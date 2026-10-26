//! Tests of the Garmin commands. Jobs run on the inline spawner of `testing` unless a test needs a job in flight
//! (`deferred`); either way a result is applied only by `Session::poll_jobs`. The tests that check where a secret store
//! is touched run the deferred jobs on another thread, as the app's runtime does.

use super::*;
use crate::testing::{
    DeferredSpawner, RecordingSleep, SecretOp, TEST_FILE_STORE_LOCATION, TestConfig, connection_at, fixed_clock, poll_finished, save_connection,
    saved_connection, stored_key, test_session, test_session_on, test_session_with,
};
use crate::{Caller, JobFinished, Services, SyncStatusView, commands};
use chrono::Duration;
use gymgar_store::Store;
use secrecy::SecretString;
use serde_json::json;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread::{self, ThreadId};
use tempfile::TempDir;

fn read_status(session: &mut Session, caller: Caller) -> GarminStatusView {
    match session.execute("garmin.status", Value::Null, caller) {
        Ok(Outcome::Done(value)) => serde_json::from_value(value).unwrap(),
        other => panic!("garmin.status: {other:?}"),
    }
}

/// The services of `gymgar-cli`: no secret stores, no Garmin, no spawner.
fn headless_session() -> Session {
    let services = Services {
        secrets: None,
        garmin: None,
        spawner: None,
        notify: Arc::new(|| {}),
        clock: fixed_clock(TestConfig::default().now),
        sleep: RecordingSleep::new().sleep_fn(),
    };
    Session::new(Store::open_in_memory().unwrap(), services)
}

#[test]
fn status_not_connected_by_default() {
    let mut session = test_session();
    for caller in [Caller::User, Caller::Agent] {
        assert_eq!(
            read_status(&mut session, caller),
            GarminStatusView {
                status: ConnectionStatus::NotConnected,
                key_expires_at: None,
                last_verified_at: None,
                expiry_warning: false,
                secret_backend: None,
                system_store_available: true,
                new_key_backend: Some(SecretBackend::System),
                file_store_needs_consent: false,
                file_store_location: Some(TEST_FILE_STORE_LOCATION.to_owned()),
                check_running: false,
                sync: SyncStatusView::default(),
            }
        );
    }
    let Ok(Outcome::Done(raw)) = session.execute("garmin.status", json!({}), Caller::Agent) else { panic!("garmin.status failed") };
    assert_eq!(raw.get("status"), Some(&json!("notConnected")));
    assert_eq!(raw.get("expiryWarning"), Some(&json!(false)));
    assert_eq!(raw.get("keyExpiresAt"), Some(&Value::Null));
    assert_eq!(raw.get("newKeyBackend"), Some(&json!("system")));
    assert_eq!(raw.get("fileStoreNeedsConsent"), Some(&json!(false)));
    assert_eq!(raw.get("fileStoreLocation"), Some(&json!(TEST_FILE_STORE_LOCATION)));
}

#[test]
fn status_reports_the_saved_connection() {
    let cfg = TestConfig::default();
    let (mut session, _) = test_session_with(cfg.clone());
    let connection = connection_at(cfg.now - Duration::days(10), SecretBackend::File);
    save_connection(&session, &connection);

    let view = read_status(&mut session, Caller::Agent);
    assert_eq!(view.status, ConnectionStatus::Connected);
    assert_eq!(view.key_expires_at, Some(connection.key_expires_at));
    assert_eq!(view.last_verified_at, connection.last_verified_at);
    assert_eq!(view.secret_backend, Some(SecretBackend::File));
    assert!(!view.expiry_warning);

    let Ok(Outcome::Done(raw)) = session.execute("garmin.status", Value::Null, Caller::User) else { panic!("garmin.status failed") };
    assert_eq!(raw.get("keyExpiresAt"), Some(&json!("2027-10-13T12:00:00Z")));
    assert_eq!(raw.get("secretBackend"), Some(&json!("file")));
}

#[test]
fn status_reports_expiry_warning() {
    let cfg = TestConfig::default();
    let (mut session, _) = test_session_with(cfg.clone());
    let mut connection = connection_at(cfg.now - Duration::days(360), SecretBackend::System);
    connection.key_expires_at = cfg.now + Duration::days(5);
    save_connection(&session, &connection);
    assert!(read_status(&mut session, Caller::Agent).expiry_warning);

    // The warning follows the session's clock, not the wall clock.
    connection.key_expires_at = cfg.now + Duration::days(8);
    save_connection(&session, &connection);
    assert!(!read_status(&mut session, Caller::Agent).expiry_warning);
}

#[test]
fn status_reports_where_a_new_key_goes() {
    let with_stores = |system_store, prefer_file| test_session_with(TestConfig { system_store, prefer_file, ..TestConfig::default() }).0;
    let file = Some(TEST_FILE_STORE_LOCATION.to_owned());
    // (case, session, systemStoreAvailable, newKeyBackend, fileStoreNeedsConsent, fileStoreLocation)
    let cases = [
        ("CLI without secret stores", headless_session(), false, None, false, None),
        ("debug build, system store present", with_stores(true, true), true, Some(SecretBackend::File), false, file.clone()),
        ("release build with a system store", with_stores(true, false), true, Some(SecretBackend::System), false, file.clone()),
        ("release build without a system store", with_stores(false, false), false, Some(SecretBackend::File), true, file),
    ];
    for (case, mut session, available, backend, consent, location) in cases {
        let view = read_status(&mut session, Caller::Agent);
        assert_eq!(
            (view.system_store_available, view.new_key_backend, view.file_store_needs_consent, view.file_store_location),
            (available, backend, consent, location),
            "{case}"
        );
    }
}

#[test]
fn status_has_no_file_location_when_the_file_store_does_not_know_it() {
    // A store that does not override `SecretStore::location` has none to report; the status then says so instead of guessing.
    let (mut session, _) = test_session_with(TestConfig { file_store_location: None, ..TestConfig::default() });
    assert_eq!(read_status(&mut session, Caller::Agent).file_store_location, None);
    let Ok(Outcome::Done(raw)) = session.execute("garmin.status", Value::Null, Caller::Agent) else { panic!("garmin.status failed") };
    assert_eq!(raw.get("fileStoreLocation"), Some(&Value::Null));
}

#[test]
fn agent_cannot_disconnect() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    let connection = connection_at(cfg.now, SecretBackend::System);
    save_connection(&session, &connection);
    let system = handles.system.unwrap();
    system.set(&SecretString::from("k-123")).unwrap();

    assert_eq!(session.execute("garmin.disconnect", Value::Null, Caller::Agent), Err(EngineError::AgentNotAllowed("garmin.disconnect".to_owned())));
    assert_eq!(saved_connection(&session), Some(connection));
    assert_eq!(stored_key(&system).as_deref(), Some("k-123"));
}

#[test]
fn disconnect_removes_key_and_row() {
    for backend in [SecretBackend::System, SecretBackend::File] {
        let cfg = TestConfig::default();
        let (mut session, handles) = test_session_with(cfg.clone());
        let system = handles.system.unwrap();
        let (recorded, other) = match backend {
            SecretBackend::System => (system, handles.file),
            SecretBackend::File => (handles.file, system),
        };
        save_connection(&session, &connection_at(cfg.now, backend));
        recorded.set(&SecretString::from("k-123")).unwrap();
        other.set(&SecretString::from("k-other")).unwrap();

        let finished = run_job(&mut session, "garmin.disconnect", Value::Null);
        let Ok(raw) = finished.result else { panic!("{backend:?}: {:?}", finished.result) };
        assert!(!raw.to_string().contains("k-123"), "{raw}");
        assert_eq!(serde_json::from_value::<GarminStatusView>(raw).unwrap().status, ConnectionStatus::NotConnected);
        assert_eq!(stored_key(&recorded), None, "{backend:?}");
        assert_eq!(saved_connection(&session), None, "{backend:?}");
        // Only the store recorded with the connection is touched.
        assert_eq!(stored_key(&other).as_deref(), Some("k-other"), "{backend:?}");
    }
}

#[test]
fn disconnect_keeps_the_connection_when_the_key_cannot_be_deleted() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    let connection = connection_at(cfg.now, SecretBackend::File);
    save_connection(&session, &connection);
    handles.file.set(&SecretString::from("k-123")).unwrap();
    handles.file.set_failing(true);

    let finished = run_job(&mut session, "garmin.disconnect", Value::Null);
    assert!(matches!(finished.result, Err(EngineError::Secrets(_))), "{:?}", finished.result);
    assert_eq!(saved_connection(&session), Some(connection));
    assert_eq!(stored_key(&handles.file).as_deref(), Some("k-123"));
    // Nothing holds the connection any more: the user can try again.
    handles.file.set_failing(false);
    assert_eq!((commands::find("garmin.disconnect").unwrap().enabled)(&session), Ok(()));
    view_of(run_job(&mut session, "garmin.disconnect", Value::Null));
    assert_eq!((saved_connection(&session), stored_key(&handles.file)), (None, None));
}

#[test]
fn disconnect_when_not_connected_is_disabled() {
    let mut session = test_session();
    match session.execute("garmin.disconnect", Value::Null, Caller::User) {
        Err(EngineError::Disabled { cmd, reason }) => {
            assert_eq!(cmd, "garmin.disconnect");
            assert!(reason.contains("not connected"), "{reason}");
        }
        other => panic!("expected Disabled, got {other:?}"),
    }
}

#[test]
fn disconnect_without_the_recorded_store_is_disabled() {
    let cfg = TestConfig { system_store: false, ..TestConfig::default() };
    let (mut session, _) = test_session_with(cfg.clone());
    let connection = connection_at(cfg.now, SecretBackend::System);
    save_connection(&session, &connection);
    match session.execute("garmin.disconnect", Value::Null, Caller::User) {
        Err(EngineError::Disabled { reason, .. }) => assert!(reason.contains("system secret store"), "{reason}"),
        other => panic!("expected Disabled, got {other:?}"),
    }
    assert_eq!(saved_connection(&session), Some(connection));

    let mut headless = headless_session();
    save_connection(&headless, &connection_at(cfg.now, SecretBackend::File));
    match headless.execute("garmin.disconnect", Value::Null, Caller::User) {
        Err(EngineError::Disabled { reason, .. }) => assert!(reason.contains("secret store"), "{reason}"),
        other => panic!("expected Disabled, got {other:?}"),
    }
    assert!(saved_connection(&headless).is_some());
}

#[test]
fn bad_params_are_errors() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    save_connection(&session, &connection_at(cfg.now, SecretBackend::File));
    handles.file.set(&SecretString::from("k-123")).unwrap();
    for id in ["garmin.status", "garmin.disconnect"] {
        for params in [json!({"force": true}), json!(1), json!("x"), json!([])] {
            let result = session.execute(id, params.clone(), Caller::User);
            assert!(matches!(result, Err(EngineError::BadParams { .. })), "{id} {params}: {result:?}");
        }
    }
    // A rejected disconnect changed nothing.
    assert!(saved_connection(&session).is_some());
    assert_eq!(stored_key(&handles.file).as_deref(), Some("k-123"));
}

// --- garmin.connect and garmin.verify ---

const KEY: &str = "k-123";
const RUNNING: &str = "connection check already running";

fn connect_params(key: &str) -> Value {
    json!({"apiKey": key, "keyTerm": 365})
}

/// Runs job command `id` and applies its result, expecting exactly that job to finish.
fn run_job(session: &mut Session, id: &str, params: Value) -> JobFinished {
    let job = match session.execute(id, params, Caller::User) {
        Ok(Outcome::Started(job)) => job,
        other => panic!("{id}: expected a started job, got {other:?}"),
    };
    let mut finished = poll_finished(session);
    assert_eq!(finished.len(), 1, "{id}: {finished:?}");
    let finished = finished.pop().unwrap();
    assert_eq!((finished.id, finished.command), (job, id));
    finished
}

fn view_of(finished: JobFinished) -> GarminStatusView {
    match finished.result {
        Ok(value) => serde_json::from_value(value).unwrap(),
        Err(error) => panic!("{}: {error:?}", finished.command),
    }
}

/// A saved connection made ten days before `now` whose key `key` is in `backend`'s store.
fn connected(session: &Session, handles: &crate::testing::TestHandles, now: DateTime<Utc>, backend: SecretBackend, key: &str) -> GarminConnection {
    let connection = connection_at(now - Duration::days(10), backend);
    save_connection(session, &connection);
    let store = match backend {
        SecretBackend::System => handles.system.as_ref().unwrap(),
        SecretBackend::File => &handles.file,
    };
    store.set(&SecretString::from(key)).unwrap();
    connection
}

#[test]
fn connect_success_stores_key_and_row() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    let view = view_of(run_job(&mut session, "garmin.connect", connect_params(KEY)));

    assert_eq!(view.status, ConnectionStatus::Connected);
    assert_eq!(view.key_expires_at, Some(cfg.now + Duration::days(365)));
    assert_eq!(view.last_verified_at, Some(cfg.now));
    assert_eq!(view.secret_backend, Some(SecretBackend::System));
    assert!(!view.check_running);
    assert_eq!(read_status(&mut session, Caller::Agent), view);
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY));
    assert_eq!(stored_key(&handles.file), None);
    assert_eq!(
        saved_connection(&session),
        Some(GarminConnection {
            connected_at: cfg.now,
            key_term: KeyTerm::Days365,
            key_expires_at: cfg.now + Duration::days(365),
            last_verified_at: Some(cfg.now),
            last_status: ConnectionStatus::Connected,
            secret_backend: SecretBackend::System,
        })
    );
    assert_eq!(handles.garmin.calls(), 1);
    // The finished job woke the frontend up once.
    assert_eq!(handles.notifications.load(Ordering::SeqCst), 1);
    assert!(session.poll_jobs().is_empty(), "a result is handed out once");
}

#[test]
fn connect_records_the_chosen_key_term() {
    let cfg = TestConfig::default();
    let (mut session, _) = test_session_with(cfg.clone());
    let view = view_of(run_job(&mut session, "garmin.connect", json!({"apiKey": KEY, "keyTerm": 90})));
    assert_eq!(view.key_expires_at, Some(cfg.now + Duration::days(90)));
    assert_eq!(saved_connection(&session).unwrap().key_term, KeyTerm::Days90);
}

#[test]
fn connect_rejected_saves_nothing() {
    for error in
        [GarminAuthError::KeyRejected, GarminAuthError::RateLimited, GarminAuthError::Unavailable, GarminAuthError::Network, GarminAuthError::Unexpected]
    {
        let (mut session, handles) = test_session_with(TestConfig { garmin: Err(error), ..TestConfig::default() });
        let finished = run_job(&mut session, "garmin.connect", connect_params(KEY));
        assert_eq!(finished.result, Err(EngineError::Garmin(error)));
        assert_eq!(stored_key(handles.system.as_ref().unwrap()), None, "{error:?}");
        assert_eq!(stored_key(&handles.file), None, "{error:?}");
        assert_eq!(saved_connection(&session), None, "{error:?}");
        assert_eq!(read_status(&mut session, Caller::User).status, ConnectionStatus::NotConnected);
    }
}

#[test]
fn connect_rejected_keeps_the_old_key() {
    // "Replace key": the old key stays until the new one has passed the check (design spec §2, item 4).
    let cfg = TestConfig { garmin: Err(GarminAuthError::KeyRejected), ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    let old = connected(&session, &handles, cfg.now, SecretBackend::System, "k-old");

    let finished = run_job(&mut session, "garmin.connect", connect_params("k-new"));
    assert_eq!(finished.result, Err(EngineError::Garmin(GarminAuthError::KeyRejected)));
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some("k-old"));
    assert_eq!(saved_connection(&session), Some(old));
}

#[test]
fn connect_replaces_the_old_key() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    connected(&session, &handles, cfg.now, SecretBackend::System, "k-old");

    let view = view_of(run_job(&mut session, "garmin.connect", json!({"apiKey": "k-new", "keyTerm": 30})));
    assert_eq!(view.key_expires_at, Some(cfg.now + Duration::days(30)));
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some("k-new"));
    let saved = saved_connection(&session).unwrap();
    assert_eq!((saved.connected_at, saved.key_term), (cfg.now, KeyTerm::Days30));
}

#[test]
fn reconnect_into_another_store_removes_the_key_from_the_old_one() {
    // The connection was made by a build that kept keys in a file; this one keeps them in the system store. Only the
    // recorded store is ever read or cleared again, so a key left in the old one would stay there for good.
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    connected(&session, &handles, cfg.now, SecretBackend::File, "k-old");

    let view = view_of(run_job(&mut session, "garmin.connect", connect_params("k-new")));
    assert_eq!(view.secret_backend, Some(SecretBackend::System));
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some("k-new"));
    assert_eq!(stored_key(&handles.file), None, "the old key is gone from the old store");

    // And disconnecting now leaves no key anywhere.
    view_of(run_job(&mut session, "garmin.disconnect", Value::Null));
    assert_eq!((stored_key(handles.system.as_ref().unwrap()), stored_key(&handles.file)), (None, None));
}

#[test]
fn reconnect_succeeds_when_the_old_store_cannot_be_cleared() {
    // Clearing the old store is housekeeping: the new key is saved and recorded all the same.
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    connected(&session, &handles, cfg.now, SecretBackend::File, "k-old");
    handles.file.set_failing(true);

    let view = view_of(run_job(&mut session, "garmin.connect", connect_params("k-new")));
    assert_eq!((view.status, view.secret_backend), (ConnectionStatus::Connected, Some(SecretBackend::System)));
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some("k-new"));
    handles.file.set_failing(false);
    assert_eq!(stored_key(&handles.file).as_deref(), Some("k-old"), "left behind, and logged");
}

#[test]
fn reconnect_into_the_same_store_keeps_only_the_new_key() {
    let cfg = TestConfig { prefer_file: true, ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    connected(&session, &handles, cfg.now, SecretBackend::File, "k-old");
    view_of(run_job(&mut session, "garmin.connect", connect_params("k-new")));
    assert_eq!(stored_key(&handles.file).as_deref(), Some("k-new"), "replaced in place, not deleted");
    assert_eq!(stored_key(handles.system.as_ref().unwrap()), None);
}

#[test]
fn connect_trims_whitespace() {
    // Review Focus 1: a key pasted with spaces or a line break around it is checked and saved without them.
    for pasted in ["  k-123\n", "\tk-123 \r\n", "k-123"] {
        let (mut session, handles) = test_session_with(TestConfig::default());
        view_of(run_job(&mut session, "garmin.connect", connect_params(pasted)));
        assert_eq!(handles.garmin.last_key().as_deref(), Some(KEY), "{pasted:?}");
        assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY), "{pasted:?}");
    }
}

#[test]
fn connect_keeps_long_keys_whole() {
    // Real keys are longer than 128 characters (the one seen was 132); nothing may cut them.
    let key = "k".repeat(300);
    let (mut session, handles) = test_session_with(TestConfig::default());
    view_of(run_job(&mut session, "garmin.connect", connect_params(&key)));
    assert_eq!(handles.garmin.last_key(), Some(key.clone()));
    assert_eq!(stored_key(handles.system.as_ref().unwrap()), Some(key));
}

#[test]
fn connect_empty_key_is_bad_params() {
    // An empty key is refused here, before any request (it would only come back as a rejected key).
    let (mut session, handles) = test_session_with(TestConfig::default());
    for key in ["", "   ", " \n\t\r\n"] {
        match session.execute("garmin.connect", connect_params(key), Caller::User) {
            Err(EngineError::BadParams { cmd, msg }) => {
                assert_eq!(cmd, "garmin.connect");
                assert!(msg.contains("apiKey"), "{msg}");
            }
            other => panic!("{key:?}: expected BadParams, got {other:?}"),
        }
    }
    assert_eq!(handles.garmin.calls(), 0);
    assert!(session.poll_jobs().is_empty());
    assert!(!read_status(&mut session, Caller::User).check_running);
}

#[test]
fn connect_bad_params_are_errors() {
    let (mut session, handles) = test_session_with(TestConfig::default());
    let cases = [
        json!({}),
        json!({"apiKey": KEY}),
        json!({"keyTerm": 365}),
        json!({"apiKey": KEY, "keyTerm": 31}),
        json!({"apiKey": KEY, "keyTerm": "365"}),
        json!({"apiKey": 42, "keyTerm": 365}),
        json!({"apiKey": KEY, "keyTerm": 365, "allowFileStore": "yes"}),
        json!({"apiKey": KEY, "keyTerm": 365, "extra": 1}),
        json!({"api_key": KEY, "keyTerm": 365}),
        json!([KEY, 365]),
        json!(KEY),
        Value::Null,
    ];
    for params in cases {
        match session.execute("garmin.connect", params.clone(), Caller::User) {
            Err(error @ EngineError::BadParams { .. }) => {
                assert!(!format!("{error:?} {error}").contains(KEY), "{params}: the error repeats the key: {error}");
            }
            other => panic!("{params}: expected BadParams, got {other:?}"),
        }
    }
    assert_eq!(handles.garmin.calls(), 0);
    assert!(session.poll_jobs().is_empty());
    assert_eq!(stored_key(handles.system.as_ref().unwrap()), None);
    assert_eq!(saved_connection(&session), None);
}

#[test]
fn connect_twice_is_disabled() {
    // Review Focus 3. The inline spawner has already finished the check, but its result waits in the channel until
    // `poll_jobs` applies it: until then the check counts as running.
    let (mut session, handles) = test_session_with(TestConfig::default());
    let Ok(Outcome::Started(first)) = session.execute("garmin.connect", connect_params(KEY), Caller::User) else { panic!("connect did not start") };
    for (id, params) in [("garmin.connect", connect_params("k-456")), ("garmin.verify", Value::Null), ("garmin.disconnect", Value::Null)] {
        assert_eq!(session.execute(id, params, Caller::User), Err(EngineError::Disabled { cmd: id.to_owned(), reason: RUNNING.to_owned() }), "{id}");
        assert_eq!((commands::find(id).unwrap().enabled)(&session), Err(RUNNING.to_owned()), "{id}");
    }
    assert!(read_status(&mut session, Caller::Agent).check_running);
    assert_eq!(handles.garmin.calls(), 1);

    let finished = poll_finished(&mut session);
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].id, first);
    assert!(finished[0].result.is_ok());
    assert!(!read_status(&mut session, Caller::Agent).check_running);
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY));

    // Once applied, the next check may start.
    let second = run_job(&mut session, "garmin.connect", connect_params("k-456"));
    assert_ne!(second.id, first);
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some("k-456"));
}

#[test]
fn check_in_flight_is_reported_until_applied() {
    let (mut session, handles) = test_session_with(TestConfig { deferred: true, ..TestConfig::default() });
    let spawner = handles.spawner.clone().unwrap();
    let Ok(Outcome::Started(job)) = session.execute("garmin.connect", connect_params(KEY), Caller::User) else { panic!("connect did not start") };
    // Nothing has run yet: no request, nothing to apply, but the check is running.
    assert_eq!((handles.garmin.calls(), spawner.pending()), (0, 1));
    assert!(session.poll_jobs().is_empty());
    assert!(read_status(&mut session, Caller::User).check_running);
    assert_eq!(handles.notifications.load(Ordering::SeqCst), 0);

    spawner.run_pending();
    assert_eq!(handles.notifications.load(Ordering::SeqCst), 1);
    assert!(read_status(&mut session, Caller::User).check_running, "finished but not applied yet");
    // The job has saved the key (a secret store may make the user wait); the record is written on the session's thread.
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY));
    assert_eq!(saved_connection(&session), None, "the record waits for the session's thread");

    let finished = poll_finished(&mut session);
    assert_eq!(finished.iter().map(|f| (f.id, f.command)).collect::<Vec<_>>(), [(job, "garmin.connect")]);
    assert!(!read_status(&mut session, Caller::User).check_running);
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY));
}

#[test]
fn connect_requires_file_consent() {
    let cfg = TestConfig { system_store: false, ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg);
    for params in [connect_params(KEY), json!({"apiKey": KEY, "keyTerm": 365, "allowFileStore": false})] {
        assert_eq!(session.execute("garmin.connect", params, Caller::User), Err(EngineError::FileStoreConsentRequired));
    }
    // Refused before any request.
    assert_eq!(handles.garmin.calls(), 0);
    assert!(session.poll_jobs().is_empty());
    assert_eq!(stored_key(&handles.file), None);
    assert_eq!(saved_connection(&session), None);

    let view = view_of(run_job(&mut session, "garmin.connect", json!({"apiKey": KEY, "keyTerm": 365, "allowFileStore": true})));
    assert_eq!(view.secret_backend, Some(SecretBackend::File));
    assert_eq!(stored_key(&handles.file).as_deref(), Some(KEY));
    assert_eq!(saved_connection(&session).unwrap().secret_backend, SecretBackend::File);
}

#[test]
fn connect_saves_the_key_where_the_storage_rule_says() {
    // (system store present, prefer_file, allowFileStore) -> where the key goes
    let cases = [
        (true, false, false, SecretBackend::System),
        // Consent to a file does not move the key out of a working system store.
        (true, false, true, SecretBackend::System),
        // Debug builds keep keys in files without asking.
        (true, true, false, SecretBackend::File),
        (false, true, false, SecretBackend::File),
        (false, false, true, SecretBackend::File),
    ];
    for (system_store, prefer_file, allow, backend) in cases {
        let case = format!("system {system_store}, prefer_file {prefer_file}, allowFileStore {allow}");
        let (mut session, handles) = test_session_with(TestConfig { system_store, prefer_file, ..TestConfig::default() });
        let view = view_of(run_job(&mut session, "garmin.connect", json!({"apiKey": KEY, "keyTerm": 365, "allowFileStore": allow})));
        assert_eq!(view.secret_backend, Some(backend), "{case}");
        let (used, other) = match backend {
            SecretBackend::System => (stored_key(handles.system.as_ref().unwrap()), Some(stored_key(&handles.file))),
            SecretBackend::File => (stored_key(&handles.file), handles.system.as_ref().map(|system| stored_key(system))),
        };
        assert_eq!(used.as_deref(), Some(KEY), "{case}");
        assert_eq!(other.flatten(), None, "{case}: the other store is untouched");
    }
}

#[test]
fn connect_keeps_everything_when_the_key_cannot_be_saved() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    let old = connected(&session, &handles, cfg.now, SecretBackend::System, "k-old");
    let system = handles.system.clone().unwrap();
    system.set_failing(true);

    let finished = run_job(&mut session, "garmin.connect", connect_params("k-new"));
    assert!(matches!(finished.result, Err(EngineError::Secrets(_))), "{:?}", finished.result);
    system.set_failing(false);
    assert_eq!(stored_key(&system).as_deref(), Some("k-old"));
    assert_eq!(saved_connection(&session), Some(old));
}

/// Makes every write of the connection record to the database at `path` fail, as a full disk would.
fn block_saving(path: &Path) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_insert BEFORE INSERT ON garmin_connection BEGIN SELECT RAISE(ABORT, 'test: saving fails'); END;
             CREATE TRIGGER no_update BEFORE UPDATE ON garmin_connection BEGIN SELECT RAISE(ABORT, 'test: saving fails'); END;",
        )
        .unwrap();
}

#[test]
fn connect_restores_the_old_key_when_the_connection_cannot_be_saved() {
    for old_key in [Some("k-old"), None] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("gymgar.sqlite3");
        let (mut session, handles) = test_session_on(Store::open(&path).unwrap(), TestConfig::default());
        let system = handles.system.clone().unwrap();
        if let Some(key) = old_key {
            system.set(&SecretString::from(key)).unwrap();
        }
        block_saving(&path);

        let finished = run_job(&mut session, "garmin.connect", connect_params("k-new"));
        assert!(matches!(finished.result, Err(EngineError::Store(_))), "{:?}", finished.result);
        assert_eq!(stored_key(&system).as_deref(), old_key);
        assert_eq!(saved_connection(&session), None);
    }
}

#[test]
fn connect_result_has_no_key() {
    let params: ConnectParams = params::parse("garmin.connect", connect_params(KEY)).unwrap();
    assert!(!format!("{params:?}").contains(KEY), "{params:?}");

    let (mut session, _) = test_session_with(TestConfig::default());
    let outcome = session.execute("garmin.connect", connect_params(KEY), Caller::User).unwrap();
    assert!(!format!("{outcome:?}").contains(KEY));
    let finished = poll_finished(&mut session);
    let shown = format!("{finished:?}");
    assert!(!shown.contains(KEY), "{shown}");
    let value = finished[0].result.as_ref().unwrap();
    assert!(!serde_json::to_string(value).unwrap().contains(KEY), "{value}");

    let (mut session, _) = test_session_with(TestConfig { garmin: Err(GarminAuthError::KeyRejected), ..TestConfig::default() });
    let error = run_job(&mut session, "garmin.connect", connect_params(KEY)).result.unwrap_err();
    assert!(!format!("{error:?} {error}").contains(KEY));
}

#[test]
fn connect_schema_describes_the_params() {
    let schema = (commands::find("garmin.connect").unwrap().params_schema)();
    assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
    let properties = schema.get("properties").unwrap();
    assert_eq!(properties.pointer("/apiKey/type"), Some(&json!("string")));
    assert_eq!(properties.pointer("/keyTerm/enum"), Some(&json!([30, 90, 180, 365])));
    assert_eq!(properties.pointer("/allowFileStore/type"), Some(&json!("boolean")));
    let mut required: Vec<_> = schema.get("required").unwrap().as_array().unwrap().iter().filter_map(Value::as_str).collect();
    required.sort_unstable();
    assert_eq!(required, ["apiKey", "keyTerm"]);
}

#[test]
fn agent_cannot_connect_or_verify() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    assert_eq!(session.execute("garmin.connect", connect_params(KEY), Caller::Agent), Err(EngineError::AgentNotAllowed("garmin.connect".to_owned())));
    let connection = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
    assert_eq!(session.execute("garmin.verify", Value::Null, Caller::Agent), Err(EngineError::AgentNotAllowed("garmin.verify".to_owned())));
    assert_eq!(handles.garmin.calls(), 0);
    assert!(session.poll_jobs().is_empty());
    assert_eq!(saved_connection(&session), Some(connection));
}

#[test]
fn headless_session_cannot_check_keys() {
    let mut session = headless_session();
    save_connection(&session, &connection_at(TestConfig::default().now, SecretBackend::File));
    for (id, params) in [("garmin.connect", connect_params(KEY)), ("garmin.verify", Value::Null)] {
        match session.execute(id, params, Caller::User) {
            Err(EngineError::Disabled { cmd, reason }) => {
                assert_eq!(cmd, id);
                assert!(reason.contains("secret store"), "{id}: {reason}");
            }
            other => panic!("{id}: expected Disabled, got {other:?}"),
        }
    }
    assert!(session.poll_jobs().is_empty());
}

#[test]
fn verify_success_records_the_check() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    let mut old = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
    old.last_status = ConnectionStatus::Unverified;
    save_connection(&session, &old);

    let view = view_of(run_job(&mut session, "garmin.verify", Value::Null));
    assert_eq!(view.status, ConnectionStatus::Connected);
    assert_eq!(view.last_verified_at, Some(cfg.now));
    assert_eq!(handles.garmin.last_key().as_deref(), Some(KEY));
    // Only the outcome of the check changes; the connection itself stays as it was.
    assert_eq!(saved_connection(&session), Some(GarminConnection { last_status: ConnectionStatus::Connected, last_verified_at: Some(cfg.now), ..old }));
}

#[test]
fn verify_key_missing() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    let connection = connection_at(cfg.now - Duration::days(10), SecretBackend::System);
    save_connection(&session, &connection);

    let finished = run_job(&mut session, "garmin.verify", Value::Null);
    assert_eq!(finished.result, Err(EngineError::KeyMissing));
    assert_eq!(handles.garmin.calls(), 0);
    assert_eq!(read_status(&mut session, Caller::Agent).status, ConnectionStatus::KeyMissing);
    assert_eq!(saved_connection(&session), Some(GarminConnection { last_status: ConnectionStatus::KeyMissing, ..connection }));
}

#[test]
fn verify_reads_recorded_backend() {
    // The key is read from the store recorded with the connection, never from where a new key would go now.
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    save_connection(&session, &connection_at(cfg.now, SecretBackend::File));
    handles.system.as_ref().unwrap().set(&SecretString::from(KEY)).unwrap();
    assert_eq!(run_job(&mut session, "garmin.verify", Value::Null).result, Err(EngineError::KeyMissing));
    assert_eq!(handles.garmin.calls(), 0);
    assert_eq!(saved_connection(&session).unwrap().last_status, ConnectionStatus::KeyMissing);

    // A debug build (new keys go to files) still reads a key recorded in the system store from there.
    let (mut session, handles) = test_session_with(TestConfig { prefer_file: true, ..cfg.clone() });
    connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
    handles.file.set(&SecretString::from("k-file")).unwrap();
    assert_eq!(view_of(run_job(&mut session, "garmin.verify", Value::Null)).status, ConnectionStatus::Connected);
    assert_eq!(handles.garmin.last_key().as_deref(), Some(KEY));
}

#[test]
fn verify_without_the_recorded_store_is_key_missing() {
    // The store changed (no system store in this session): the key is gone as far as this session can tell.
    let cfg = TestConfig { system_store: false, ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    save_connection(&session, &connection_at(cfg.now, SecretBackend::System));
    handles.file.set(&SecretString::from(KEY)).unwrap();
    assert_eq!(run_job(&mut session, "garmin.verify", Value::Null).result, Err(EngineError::KeyMissing));
    assert_eq!(handles.garmin.calls(), 0);
    assert_eq!(saved_connection(&session).unwrap().last_status, ConnectionStatus::KeyMissing);
}

#[test]
fn verify_unreadable_key_is_key_missing() {
    // A store that cannot be read (a locked keychain) is reported, and the key counts as missing (design spec §2, item 7).
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    connected(&session, &handles, cfg.now, SecretBackend::File, KEY);
    handles.file.set_failing(true);
    let finished = run_job(&mut session, "garmin.verify", Value::Null);
    assert!(matches!(finished.result, Err(EngineError::Secrets(_))), "{:?}", finished.result);
    assert_eq!(handles.garmin.calls(), 0);
    assert_eq!(saved_connection(&session).unwrap().last_status, ConnectionStatus::KeyMissing);
    handles.file.set_failing(false);
    assert_eq!(stored_key(&handles.file).as_deref(), Some(KEY), "nothing is deleted");
}

#[test]
fn verify_network_is_unverified() {
    for error in [GarminAuthError::Network, GarminAuthError::Unavailable, GarminAuthError::RateLimited, GarminAuthError::Unexpected] {
        let cfg = TestConfig { garmin: Err(error), ..TestConfig::default() };
        let (mut session, handles) = test_session_with(cfg.clone());
        let connection = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);

        let finished = run_job(&mut session, "garmin.verify", Value::Null);
        assert_eq!(finished.result, Err(EngineError::Garmin(error)));
        let view = read_status(&mut session, Caller::Agent);
        assert_eq!(view.status, ConnectionStatus::Unverified, "{error:?}");
        assert_eq!(view.last_verified_at, connection.last_verified_at, "{error:?}: the last successful check stays");
        assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY));
    }
}

#[test]
fn verify_network_keeps_a_rejected_key_rejected() {
    // Design spec §2, item 8: with no answer from Garmin the last known status stays. A key Garmin refused is not
    // "connected, could not verify" just because the next check went unanswered.
    for error in [GarminAuthError::Network, GarminAuthError::Unavailable, GarminAuthError::RateLimited, GarminAuthError::Unexpected] {
        let cfg = TestConfig { garmin: Err(error), ..TestConfig::default() };
        let (mut session, handles) = test_session_with(cfg.clone());
        let mut rejected = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
        rejected.last_status = ConnectionStatus::KeyRejected;
        save_connection(&session, &rejected);

        let finished = run_job(&mut session, "garmin.verify", Value::Null);
        assert_eq!(finished.result, Err(EngineError::Garmin(error)));
        assert_eq!(read_status(&mut session, Caller::Agent).status, ConnectionStatus::KeyRejected, "{error:?}");
        assert_eq!(saved_connection(&session), Some(rejected), "{error:?}: nothing else changes either");
        assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY));
    }
}

#[test]
fn verify_network_after_a_missing_key_was_found_again_is_unverified() {
    // The key is back in its store, so it is no longer missing; whether Garmin takes it is not known yet.
    let cfg = TestConfig { garmin: Err(GarminAuthError::Network), ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    let mut missing = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
    missing.last_status = ConnectionStatus::KeyMissing;
    save_connection(&session, &missing);

    run_job(&mut session, "garmin.verify", Value::Null);
    assert_eq!(saved_connection(&session), Some(GarminConnection { last_status: ConnectionStatus::Unverified, ..missing }));
}

#[test]
fn verify_rejected() {
    let cfg = TestConfig { garmin: Err(GarminAuthError::KeyRejected), ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    let connection = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);

    let finished = run_job(&mut session, "garmin.verify", Value::Null);
    assert_eq!(finished.result, Err(EngineError::Garmin(GarminAuthError::KeyRejected)));
    assert_eq!(saved_connection(&session), Some(GarminConnection { last_status: ConnectionStatus::KeyRejected, ..connection }));
    // The key is not deleted automatically (design spec §2, item 6).
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY));
}

#[test]
fn verify_when_not_connected_is_disabled() {
    let (mut session, handles) = test_session_with(TestConfig::default());
    match session.execute("garmin.verify", Value::Null, Caller::User) {
        Err(EngineError::Disabled { cmd, reason }) => {
            assert_eq!(cmd, "garmin.verify");
            assert!(reason.contains("not connected"), "{reason}");
        }
        other => panic!("expected Disabled, got {other:?}"),
    }
    assert_eq!(handles.garmin.calls(), 0);
}

#[test]
fn verify_bad_params_are_errors() {
    let cfg = TestConfig::default();
    let (mut session, handles) = test_session_with(cfg.clone());
    let connection = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
    for params in [json!({"force": true}), json!(1), json!("x"), json!([])] {
        let result = session.execute("garmin.verify", params.clone(), Caller::User);
        assert!(matches!(result, Err(EngineError::BadParams { .. })), "{params}: {result:?}");
    }
    assert_eq!(handles.garmin.calls(), 0);
    assert!(session.poll_jobs().is_empty());
    assert_eq!(saved_connection(&session), Some(connection));
}

#[test]
fn job_panic_is_error() {
    let cfg = TestConfig { garmin_panics: true, ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    let finished = run_job(&mut session, "garmin.connect", connect_params(KEY));
    assert!(matches!(finished.result, Err(EngineError::JobPanicked(_))), "{:?}", finished.result);
    assert_eq!(handles.notifications.load(Ordering::SeqCst), 1);
    assert_eq!(saved_connection(&session), None);
    assert_eq!(stored_key(handles.system.as_ref().unwrap()), None);

    // The session keeps working, and the panicked check no longer counts as running.
    let view = read_status(&mut session, Caller::User);
    assert_eq!((view.status, view.check_running), (ConnectionStatus::NotConnected, false));

    // A verify that panics leaves the recorded status alone.
    let connection = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
    let finished = run_job(&mut session, "garmin.verify", Value::Null);
    assert!(matches!(finished.result, Err(EngineError::JobPanicked(_))), "{:?}", finished.result);
    assert_eq!(saved_connection(&session), Some(connection));
}

#[test]
fn job_panic_on_another_thread_is_error() {
    // With a deferred spawner the job runs outside `execute`, as on the app's runtime; its panic stays inside the job.
    let (mut session, handles) = test_session_with(TestConfig { garmin_panics: true, deferred: true, ..TestConfig::default() });
    let Ok(Outcome::Started(job)) = session.execute("garmin.connect", connect_params(KEY), Caller::User) else { panic!("connect did not start") };
    let spawner = handles.spawner.clone().unwrap();
    std::thread::spawn(move || spawner.run_pending()).join().unwrap();
    let finished = poll_finished(&mut session);
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].id, job);
    assert!(matches!(finished[0].result, Err(EngineError::JobPanicked(_))), "{finished:?}");
}

// --- where the secret stores are touched: never on the session's thread (the window's, in the app) ---

/// Runs what waits on `spawner` on another thread, as the app's runtime does, and returns that thread.
fn run_elsewhere(spawner: &Arc<DeferredSpawner>) -> ThreadId {
    let spawner = spawner.clone();
    thread::spawn(move || {
        spawner.run_pending();
        thread::current().id()
    })
    .join()
    .unwrap()
}

#[test]
fn connect_writes_key_off_the_session_thread() {
    let cfg = TestConfig { deferred: true, ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    connected(&session, &handles, cfg.now, SecretBackend::System, "k-old");
    let system = handles.system.clone().unwrap();
    let spawner = handles.spawner.clone().unwrap();
    system.take_ops();

    let Ok(Outcome::Started(job)) = session.execute("garmin.connect", connect_params("k-new"), Caller::User) else { panic!("connect did not start") };
    assert!(system.take_ops().is_empty(), "the command itself touches no secret store");
    let worker = run_elsewhere(&spawner);
    assert_ne!(worker, thread::current().id());
    // The job read the key it replaces (to put it back if the record cannot be saved) and saved the new one.
    assert_eq!(system.take_ops(), [(SecretOp::Get, worker), (SecretOp::Set, worker)]);
    assert_eq!(stored_key(&system).as_deref(), Some("k-new"));

    let finished = poll_finished(&mut session);
    assert_eq!(finished.iter().map(|f| (f.id, f.result.is_ok())).collect::<Vec<_>>(), [(job, true)], "{finished:?}");
    // Applying the result only wrote the record.
    assert!(system.take_ops().is_empty(), "a secret store was touched on the session's thread");
    assert!(handles.file.take_ops().is_empty(), "a secret store was touched on the session's thread");
    assert_eq!(spawner.pending(), 0, "nothing is left to clean up: the old key was in the same store, and is replaced");
    assert_eq!(saved_connection(&session).map(|connection| connection.connected_at), Some(cfg.now));
}

#[test]
fn connect_db_failure_restores_key_in_background() {
    for old_key in [Some("k-old"), None] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("gymgar.sqlite3");
        let (mut session, handles) = test_session_on(Store::open(&path).unwrap(), TestConfig { deferred: true, ..TestConfig::default() });
        let system = handles.system.clone().unwrap();
        let spawner = handles.spawner.clone().unwrap();
        if let Some(key) = old_key {
            system.set(&SecretString::from(key)).unwrap();
        }
        block_saving(&path);
        let Ok(Outcome::Started(_)) = session.execute("garmin.connect", connect_params("k-new"), Caller::User) else { panic!("connect did not start") };
        run_elsewhere(&spawner);
        system.take_ops();

        let finished = poll_finished(&mut session);
        assert!(matches!(finished.as_slice(), [JobFinished { result: Err(EngineError::Store(_)), .. }]), "{old_key:?}: {finished:?}");
        // The old key is put back by work of its own on the spawner, not on the session's thread.
        assert!(system.take_ops().is_empty(), "{old_key:?}: a secret store was touched on the session's thread");
        assert_eq!(stored_key(&system).as_deref(), Some("k-new"), "{old_key:?}: not put back yet");
        assert_eq!(spawner.pending(), 1, "{old_key:?}");

        let worker = run_elsewhere(&spawner);
        let restore = if old_key.is_some() { SecretOp::Set } else { SecretOp::Delete };
        assert_eq!(system.take_ops(), [(restore, worker)], "{old_key:?}");
        assert_eq!(stored_key(&system).as_deref(), old_key);
        assert_eq!(saved_connection(&session), None);
    }
}

#[test]
fn connect_removes_the_replaced_key_in_background() {
    let cfg = TestConfig { deferred: true, ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    connected(&session, &handles, cfg.now, SecretBackend::File, "k-old");
    let spawner = handles.spawner.clone().unwrap();
    let Ok(Outcome::Started(_)) = session.execute("garmin.connect", connect_params("k-new"), Caller::User) else { panic!("connect did not start") };
    run_elsewhere(&spawner);
    handles.file.take_ops();

    let finished = poll_finished(&mut session);
    let [JobFinished { result: Ok(_), .. }] = finished.as_slice() else { panic!("{finished:?}") };
    assert_eq!(saved_connection(&session).map(|connection| connection.secret_backend), Some(SecretBackend::System));
    assert!(handles.file.take_ops().is_empty(), "a secret store was touched on the session's thread");
    assert_eq!(stored_key(&handles.file).as_deref(), Some("k-old"), "removed later, by work of its own");

    let worker = run_elsewhere(&spawner);
    assert_eq!(handles.file.take_ops(), [(SecretOp::Delete, worker)]);
    assert_eq!(stored_key(&handles.file), None);
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some("k-new"));
}

#[test]
fn disconnect_is_a_job() {
    let cfg = TestConfig { deferred: true, ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    let connection = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
    let system = handles.system.clone().unwrap();
    let spawner = handles.spawner.clone().unwrap();
    system.take_ops();

    let Ok(Outcome::Started(job)) = session.execute("garmin.disconnect", Value::Null, Caller::User) else { panic!("disconnect did not start") };
    assert!(system.take_ops().is_empty(), "the command itself touches no secret store");
    // While the key is being deleted, nothing may change the connection or use the key.
    for id in ["garmin.connect", "garmin.verify", "garmin.disconnect", "garmin.sync"] {
        assert_eq!((commands::find(id).unwrap().enabled)(&session), Err(DISCONNECT_RUNNING.to_owned()), "{id}");
    }
    assert_eq!(
        session.execute("garmin.disconnect", Value::Null, Caller::User),
        Err(EngineError::Disabled { cmd: "garmin.disconnect".to_owned(), reason: DISCONNECT_RUNNING.to_owned() })
    );
    assert!(!read_status(&mut session, Caller::User).check_running, "a disconnect is not a key check");

    let worker = run_elsewhere(&spawner);
    assert_eq!(system.take_ops(), [(SecretOp::Delete, worker)]);
    assert_eq!(stored_key(&system), None);
    assert_eq!(saved_connection(&session), Some(connection), "the record goes on the session's thread");

    let finished = poll_finished(&mut session);
    assert_eq!(finished.iter().map(|f| (f.id, f.command)).collect::<Vec<_>>(), [(job, "garmin.disconnect")]);
    let view = view_of(finished.into_iter().next().unwrap());
    assert_eq!((view.status, view.secret_backend), (ConnectionStatus::NotConnected, None));
    assert!(system.take_ops().is_empty(), "a secret store was touched on the session's thread");
    assert_eq!(saved_connection(&session), None);
    assert_eq!((commands::find("garmin.connect").unwrap().enabled)(&session), Ok(()), "connecting again is possible at once");
}

#[test]
fn a_dropped_disconnect_frees_the_connection_commands() {
    let cfg = TestConfig { deferred: true, ..TestConfig::default() };
    let (mut session, handles) = test_session_with(cfg.clone());
    let connection = connected(&session, &handles, cfg.now, SecretBackend::System, KEY);
    let spawner = handles.spawner.clone().unwrap();
    let Ok(Outcome::Started(_)) = session.execute("garmin.disconnect", Value::Null, Caller::User) else { panic!("disconnect did not start") };

    spawner.drop_pending();
    let finished = poll_finished(&mut session);
    assert!(matches!(finished.as_slice(), [JobFinished { result: Err(EngineError::JobDropped(_)), .. }]), "{finished:?}");
    assert_eq!(saved_connection(&session), Some(connection));
    assert_eq!(stored_key(handles.system.as_ref().unwrap()).as_deref(), Some(KEY));
    assert_eq!((commands::find("garmin.disconnect").unwrap().enabled)(&session), Ok(()));
}
