//! Garmin Connect commands: `garmin.status` and `garmin.disconnect`.
//!
//! `garmin.status` reads the database only: it never opens a secret store or contacts Garmin, so agents and the
//! CLI can call it freely. Only the key check sets statuses such as `keyMissing`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use gymgar_domain::{ConnectionStatus, SecretBackend, expiry_warning};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::{always, done};
use crate::params::{self, NoParams};
use crate::{CommandKind, CommandSpec, EngineError, Outcome, SecretStore, SecretStores, Session};

const STATUS: &str = "garmin.status";
const DISCONNECT: &str = "garmin.disconnect";

/// `enabled` reason of commands that need a connection.
const NOT_CONNECTED: &str = "Garmin Connect is not connected";

/// The answer of `garmin.status` (and of `garmin.disconnect`): the last known state of the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GarminStatusView {
    pub status: ConnectionStatus,
    /// Connection date plus the key term the user chose; `None` when not connected.
    pub key_expires_at: Option<DateTime<Utc>>,
    /// The last time Garmin accepted the key.
    pub last_verified_at: Option<DateTime<Utc>>,
    /// The key expires within 7 days, or has expired.
    pub expiry_warning: bool,
    /// Where the connected key is kept; `None` when not connected.
    pub secret_backend: Option<SecretBackend>,
    /// The system secret store exists in this session (debug builds may still prefer files: see `new_key_backend`).
    pub system_store_available: bool,
    /// Where a key connected now would be saved; `None` when this session has no secret stores (the CLI).
    pub new_key_backend: Option<SecretBackend>,
    /// A key connected now could only go to a file, which needs the user's consent (`allowFileStore`).
    pub file_store_needs_consent: bool,
    /// A key check (connect or verify) is in flight.
    pub check_running: bool,
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: STATUS,
            description: "Report the last known Garmin Connect status (notConnected, connected, keyRejected, keyMissing or unverified), \
                          when the key expires, when Garmin last accepted it, whether the expiry warning is due and where the key is \
                          stored. Reads the database only; it never contacts Garmin.",
            kind: CommandKind::Query,
            agent: true,
            destructive: false,
            params_schema: params::schema::<NoParams>,
            enabled: always,
            run: status,
        },
        CommandSpec {
            id: DISCONNECT,
            description: "Disconnect Garmin Connect: delete the Personal API key from its secret store and the connection record from \
                          the database.",
            kind: CommandKind::Mutation,
            agent: false,
            destructive: true,
            params_schema: params::schema::<NoParams>,
            enabled: disconnect_enabled,
            run: disconnect,
        },
    ]
}

fn status(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    params::parse::<NoParams>(STATUS, params)?;
    done(STATUS, &status_view(session)?)
}

/// The connection state as the database records it, judged by the session's clock.
fn status_view(session: &Session) -> Result<GarminStatusView, EngineError> {
    let connection = session.store().garmin_connection()?;
    let now = session.now();
    let secrets = session.services().secrets.as_ref();
    Ok(GarminStatusView {
        status: connection.as_ref().map_or(ConnectionStatus::NotConnected, |c| c.last_status),
        key_expires_at: connection.as_ref().map(|c| c.key_expires_at),
        last_verified_at: connection.as_ref().and_then(|c| c.last_verified_at),
        expiry_warning: connection.as_ref().is_some_and(|c| expiry_warning(c.key_expires_at, now)),
        secret_backend: connection.as_ref().map(|c| c.secret_backend),
        system_store_available: secrets.is_some_and(|s| s.system.is_some()),
        new_key_backend: secrets.map(SecretStores::new_key_backend),
        file_store_needs_consent: secrets.is_some_and(SecretStores::needs_file_consent),
        check_running: false,
    })
}

/// The secret store holding the key of a connection recorded with `backend`, or why it cannot be reached.
fn key_store(session: &Session, backend: SecretBackend) -> Result<&Arc<dyn SecretStore>, String> {
    let secrets = session.services().secrets.as_ref().ok_or("this session has no access to the secret stores")?;
    secrets.store(backend).ok_or_else(|| format!("the {} secret store, which holds the key, is unavailable", backend.as_str()))
}

fn disconnect_enabled(session: &Session) -> Result<(), String> {
    // A database failure does not grey the command out: `run` reports it as the store error it is.
    let Ok(connection) = session.store().garmin_connection() else { return Ok(()) };
    let connection = connection.ok_or(NOT_CONNECTED)?;
    key_store(session, connection.secret_backend).map(|_| ())
}

fn disconnect(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    params::parse::<NoParams>(DISCONNECT, params)?;
    let connection = session.store().garmin_connection()?.ok_or(EngineError::NotConnected)?;
    // The key goes first. If deleting it fails, the connection stays and the user can try again; the other way
    // round, a failure would leave a key behind that no command can reach any more.
    key_store(session, connection.secret_backend).map_err(EngineError::Secrets)?.delete()?;
    session.store().delete_garmin_connection()?;
    log::info!("Garmin Connect disconnected; the key was deleted from the {} secret store", connection.secret_backend.as_str());
    done(DISCONNECT, &status_view(session)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestConfig, connection_at, fixed_clock, save_connection, saved_connection, stored_key, test_session, test_session_with};
    use crate::{Caller, Services};
    use chrono::Duration;
    use gymgar_store::Store;
    use secrecy::SecretString;
    use serde_json::json;

    fn read_status(session: &mut Session, caller: Caller) -> GarminStatusView {
        match session.execute("garmin.status", Value::Null, caller) {
            Ok(Outcome::Done(value)) => serde_json::from_value(value).unwrap(),
            other => panic!("garmin.status: {other:?}"),
        }
    }

    /// The services of `gymgar-cli`: no secret stores, no Garmin, no spawner.
    fn headless_session() -> Session {
        let services = Services { secrets: None, garmin: None, spawner: None, notify: Arc::new(|| {}), clock: fixed_clock(TestConfig::default().now) };
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
                    check_running: false,
                }
            );
        }
        let Ok(Outcome::Done(raw)) = session.execute("garmin.status", json!({}), Caller::Agent) else { panic!("garmin.status failed") };
        assert_eq!(raw.get("status"), Some(&json!("notConnected")));
        assert_eq!(raw.get("expiryWarning"), Some(&json!(false)));
        assert_eq!(raw.get("keyExpiresAt"), Some(&Value::Null));
        assert_eq!(raw.get("newKeyBackend"), Some(&json!("system")));
        assert_eq!(raw.get("fileStoreNeedsConsent"), Some(&json!(false)));
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
        // (case, session, systemStoreAvailable, newKeyBackend, fileStoreNeedsConsent)
        let cases = [
            ("CLI without secret stores", headless_session(), false, None, false),
            ("debug build, system store present", with_stores(true, true), true, Some(SecretBackend::File), false),
            ("release build with a system store", with_stores(true, false), true, Some(SecretBackend::System), false),
            ("release build without a system store", with_stores(false, false), false, Some(SecretBackend::File), true),
        ];
        for (case, mut session, available, backend, consent) in cases {
            let view = read_status(&mut session, Caller::Agent);
            assert_eq!((view.system_store_available, view.new_key_backend, view.file_store_needs_consent), (available, backend, consent), "{case}");
        }
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

            let answer = session.execute("garmin.disconnect", Value::Null, Caller::User);
            let Ok(Outcome::Done(raw)) = answer else { panic!("{backend:?}: {answer:?}") };
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

        assert!(matches!(session.execute("garmin.disconnect", Value::Null, Caller::User), Err(EngineError::Secrets(_))));
        handles.file.set_failing(false);
        assert_eq!(saved_connection(&session), Some(connection));
        assert_eq!(stored_key(&handles.file).as_deref(), Some("k-123"));
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
}
