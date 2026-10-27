//! Tests of `garmin.sync` and `activity.reparse`. Garmin is a `FakeGarminSession` that answers from bytes the test
//! gives it; jobs run on the inline spawner unless a test needs one in flight (`deferred`), and pauses go to the
//! session's `RecordingSleep`, so nothing waits for real.

use std::sync::Arc;

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use gymgar_domain::{ActivityId, ConnectionStatus, FileState, RecordedSet, Reps, SecretBackend, SetRef, SyncErrorKind};
use gymgar_garmin::fit::testing::{FitWriter, SetSpec, zip_single};
use gymgar_store::{SetChange, Store};
use gymgar_sync::policy::{FILE_PAUSE, RETRY_PAUSES};
use gymgar_sync::strength::PARSER_VERSION;
use secrecy::SecretString;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::testing::{
    FakeGarminSession, FakeRequest, RecordingSleep, RequestKind, TestConfig, TestHandles, connection_at, fixed_clock, poll_all, save_connection,
    saved_connection, seed_fixture_activities, seed_strength, test_session, test_session_on, test_session_with,
};
use crate::{Caller, GarminAuthError, GarminStatusView, JobEvent, JobFinished, SecretStore, Services, commands};

const KEY: &str = "k-sync";

const A: i64 = 101;
const B: i64 = 102;
const C: i64 = 103;
const D: i64 = 104;
const O: i64 = 100;

/// The newest first, as Garmin lists them: C, then A, then B and D, and O, the oldest.
const C_START: &str = "2026-10-22 18:00:00";
const A_START: &str = "2026-10-20 10:00:00";
const B_START: &str = "2026-10-19 08:00:00";
const D_START: &str = "2026-10-18 07:00:00";
const O_START: &str = "2026-10-01 07:00:00";

fn at(text: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap().and_utc()
}

/// An element of Garmin's activity list.
fn listed(id: i64, type_key: &str, start: &str, manual: bool) -> Value {
    json!({
        "activityId": id,
        "activityName": format!("{type_key} {id}"),
        "activityType": { "typeKey": type_key },
        "startTimeGMT": start,
        "duration": 3600.0,
        "manualActivity": manual,
        "ownerFullName": "Not The Owner",
    })
}

fn strength(id: i64, start: &str) -> Value {
    listed(id, "strength_training", start, false)
}

fn running(id: i64, start: &str) -> Value {
    listed(id, "running", start, false)
}

fn page(items: &[Value]) -> Vec<u8> {
    Value::Array(items.to_vec()).to_string().into_bytes()
}

fn set_spec(message_index: u16, start: DateTime<Utc>, reps: u16, weight_kg: f64) -> SetSpec {
    SetSpec {
        message_index,
        active: true,
        start,
        duration_s: 30.0,
        reps: Some(reps),
        weight_kg: Some(weight_kg),
        category: Some(8),
        subtype: Some(23),
        step_index: None,
        pounds: false,
    }
}

/// A strength file with two sets, as Garmin sends it: a ZIP with one `.fit`.
fn strength_zip(start: &str) -> Vec<u8> {
    let start = at(start);
    let mut fit = FitWriter::new(start);
    fit.set(set_spec(0, start, 10, 60.0));
    fit.set(set_spec(1, start + TimeDelta::minutes(2), 8, 62.5));
    zip_single("activity.fit", &fit.finish())
}

/// A Garmin whose list is `pages` and that has a good file for every strength activity on them.
fn garmin_with(pages: &[&[Value]]) -> Arc<FakeGarminSession> {
    let garmin = Arc::new(FakeGarminSession::new());
    serve(&garmin, pages);
    garmin
}

/// Makes `garmin` list `pages` and serve a good file for every strength activity on them.
fn serve(garmin: &FakeGarminSession, pages: &[&[Value]]) {
    garmin.set_pages(pages.iter().map(|items| page(items)).collect());
    for item in pages.iter().flat_map(|items| items.iter()) {
        if item["activityType"]["typeKey"] == "strength_training" {
            let start = item["startTimeGMT"].as_str().unwrap();
            garmin.set_file(ActivityId(item["activityId"].as_i64().unwrap()), strength_zip(start));
        }
    }
}

/// A session connected ten days ago, its key in the system store, whose Garmin is `garmin`.
fn connected(garmin: &Arc<FakeGarminSession>, deferred: bool) -> (Session, TestHandles) {
    connected_on(Store::open_in_memory().unwrap(), garmin, deferred)
}

fn connected_on(store: Store, garmin: &Arc<FakeGarminSession>, deferred: bool) -> (Session, TestHandles) {
    let cfg = TestConfig { session: Some(garmin.clone()), deferred, ..TestConfig::default() };
    let now = cfg.now;
    let (session, handles) = test_session_on(store, cfg);
    save_connection(&session, &connection_at(now - TimeDelta::days(10), SecretBackend::System));
    handles.system.as_ref().unwrap().set(&SecretString::from(KEY)).unwrap();
    (session, handles)
}

/// Runs `garmin.sync` to its end and returns how the job ended. Every partial result must have been applied.
fn run_sync(session: &mut Session) -> JobFinished {
    run_job(session, SYNC)
}

fn run_job(session: &mut Session, id: &str) -> JobFinished {
    let job = match session.execute(id, Value::Null, Caller::User) {
        Ok(Outcome::Started(job)) => job,
        other => panic!("{id} did not start: {other:?}"),
    };
    let mut finished = None;
    for event in poll_all(session) {
        match event {
            JobEvent::Partial { id: from, result, .. } => {
                assert_eq!(from, job);
                assert_eq!(result, Ok(()), "a partial result of {id} failed to apply");
            }
            JobEvent::Finished(end) => {
                assert_eq!(end.id, job);
                assert!(finished.replace(end).is_none(), "{id} ended twice");
            }
        }
    }
    finished.unwrap_or_else(|| panic!("{id} did not end"))
}

fn status(session: &mut Session) -> GarminStatusView {
    match session.execute("garmin.status", Value::Null, Caller::User) {
        Ok(Outcome::Done(value)) => serde_json::from_value(value).unwrap(),
        other => panic!("garmin.status: {other:?}"),
    }
}

fn row(session: &Session, id: i64) -> gymgar_store::ActivityRow {
    session.store().activity_detail(ActivityId(id)).unwrap().unwrap_or_else(|| panic!("activity {id} is not stored")).row
}

fn file_state(session: &Session, id: i64) -> FileState {
    row(session, id).file_state
}

fn stored(session: &Session) -> usize {
    session.store().sync_snapshot().unwrap().known.len()
}

fn files_asked(garmin: &FakeGarminSession) -> Vec<i64> {
    garmin.requests().into_iter().filter_map(|request| if let FakeRequest::File(id) = request { Some(id.0) } else { None }).collect()
}

fn enabled(session: &Session, id: &str) -> Result<(), String> {
    (commands::find(id).unwrap().enabled)(session)
}

#[test]
fn first_sync_imports_all_without_new() {
    let garmin = garmin_with(&[&[strength(A, A_START), running(B, B_START)]]);
    let (mut session, handles) = connected(&garmin, false);
    let now = TestConfig::default().now;

    let finished = run_sync(&mut session);

    let Ok(answer) = &finished.result else { panic!("the sync failed: {:?}", finished.result) };
    assert_eq!(answer["status"], "connected");
    assert_eq!(answer["sync"]["lastSuccessAt"], "2026-10-23T12:00:00Z");
    assert_eq!(stored(&session), 2);
    assert!(!row(&session, A).is_new && !row(&session, B).is_new, "the first sync marks nothing new");
    assert_eq!(file_state(&session, A), FileState::Downloaded);
    assert_eq!(file_state(&session, B), FileState::NotNeeded);
    assert_eq!(session.store().activity_detail(ActivityId(A)).unwrap().unwrap().recorded.len(), 2);
    let state = session.store().sync_state().unwrap();
    assert!(state.history_complete);
    assert_eq!((state.last_success_at, state.last_error), (Some(now), None));
    // The second page starts after the two activities of the first.
    assert_eq!(garmin.requests(), [FakeRequest::Verify, FakeRequest::Page(0), FakeRequest::Page(2), FakeRequest::File(ActivityId(A))]);
    assert_eq!(handles.garmin.opened_key().as_deref(), Some(KEY), "the session was opened with the stored key");

    let view = status(&mut session);
    assert_eq!(view.status, ConnectionStatus::Connected);
    assert_eq!(view.last_verified_at, Some(now), "the sync's key check is recorded like garmin.verify");
    assert_eq!(view.sync, SyncStatusView { running: false, files_done: 0, files_total: 0, last_success_at: Some(now), last_error: None, new_count: 0 });
}

#[test]
fn second_sync_marks_only_newer() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, _) = connected(&garmin, false);
    assert!(run_sync(&mut session).result.is_ok());
    garmin.take_requests();

    serve(&garmin, &[&[strength(C, C_START), strength(A, A_START)], &[strength(O, O_START)]]);
    assert!(run_sync(&mut session).result.is_ok());

    assert!(row(&session, C).is_new);
    assert!(!row(&session, A).is_new);
    assert_eq!(file_state(&session, C), FileState::Downloaded);
    assert_eq!(status(&mut session).sync.new_count, 1);
    // The history was complete and the first page met a known activity: no second page, and A's file is not fetched again.
    assert_eq!(garmin.take_requests(), [FakeRequest::Page(0), FakeRequest::File(ActivityId(C))]);
    assert_eq!(stored(&session), 2, "O, behind the known activity, was never asked for");
}

#[test]
fn duplicate_across_pages() {
    // Review Focus 1: a new activity appeared while the list was paged, so A slid from the first page onto the second.
    let garmin = garmin_with(&[&[running(C, C_START), strength(A, A_START)], &[strength(A, A_START), running(B, B_START)]]);
    let (mut session, _) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(stored(&session), 3);
    assert_eq!(files_asked(&garmin), [A], "the file of A is fetched once");
    assert_eq!(file_state(&session, A), FileState::Downloaded);
}

#[test]
fn duplicate_across_pages_is_new_once() {
    let garmin = garmin_with(&[&[running(O, O_START)]]);
    let (mut session, _) = connected(&garmin, false);
    assert!(run_sync(&mut session).result.is_ok());
    garmin.take_requests();

    serve(&garmin, &[&[running(C, C_START), strength(A, A_START)], &[strength(A, A_START), running(B, B_START)], &[running(O, O_START)]]);
    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(stored(&session), 4);
    assert_eq!(status(&mut session).sync.new_count, 3, "C, A and B are new, A once");
    assert_eq!(files_asked(&garmin), [A]);
}

#[test]
fn interrupted_sync_resumes() {
    let garmin = garmin_with(&[&[strength(C, C_START), strength(A, A_START), strength(B, B_START)]]);
    // The second file fails four times: the first try and every retry.
    garmin.fail(FakeRequest::File(ActivityId(A)), [GarminAuthError::Network; 4]);
    let (mut session, _) = connected(&garmin, false);

    let first = run_sync(&mut session);
    assert_eq!(first.result, Err(EngineError::Garmin(GarminAuthError::Network)));
    assert_eq!(file_state(&session, C), FileState::Downloaded);
    assert_eq!(file_state(&session, A), FileState::Pending);
    assert_eq!(file_state(&session, B), FileState::Pending, "the sync stopped before the third file");
    let state = session.store().sync_state().unwrap();
    assert_eq!((state.last_success_at, state.last_error), (None, Some(SyncErrorKind::Network)));
    assert_eq!(files_asked(&garmin), [C, A, A, A, A]);
    garmin.take_requests();

    let second = run_sync(&mut session);
    assert!(second.result.is_ok(), "{:?}", second.result);
    // Only what is left: the list once (the history is complete), then the two files still waiting.
    assert_eq!(garmin.take_requests(), [FakeRequest::Page(0), FakeRequest::File(ActivityId(A)), FakeRequest::File(ActivityId(B))]);
    for id in [C, A, B] {
        assert_eq!(file_state(&session, id), FileState::Downloaded, "activity {id}");
    }
    assert_eq!(session.store().sync_state().unwrap().last_error, None, "a good run clears the error");
}

#[test]
fn retries_pause_then_stop() {
    let garmin = garmin_with(&[&[strength(A, A_START), strength(B, B_START)]]);
    garmin.fail(FakeRequest::File(ActivityId(A)), [GarminAuthError::RateLimited; 4]);
    let (mut session, handles) = connected(&garmin, false);

    let finished = run_sync(&mut session);

    assert_eq!(finished.result, Err(EngineError::Garmin(GarminAuthError::RateLimited)));
    assert_eq!(handles.sleep.slept(), RETRY_PAUSES.to_vec(), "30 s, 2 min and 5 min between the tries, and no more");
    assert_eq!(files_asked(&garmin), [A, A, A, A], "B is not asked for once the sync has stopped");
    let state = session.store().sync_state().unwrap();
    assert_eq!((state.last_success_at, state.last_error), (None, Some(SyncErrorKind::RateLimited)));
    // What was applied before the stop stays.
    assert_eq!(stored(&session), 2);
    assert!(state.history_complete);
    assert_eq!(file_state(&session, A), FileState::Pending);
    let view = status(&mut session);
    assert_eq!(view.status, ConnectionStatus::Connected, "Garmin accepted the key; only the requests were limited");
    assert_eq!(view.sync.last_error, Some(SyncErrorKind::RateLimited));
}

#[test]
fn a_request_that_recovers_goes_on() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    garmin.fail(FakeRequest::Page(0), [GarminAuthError::Unavailable]);
    garmin.fail(FakeRequest::File(ActivityId(A)), [GarminAuthError::Unavailable, GarminAuthError::Network]);
    let (mut session, handles) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(handles.sleep.slept(), [RETRY_PAUSES[0], RETRY_PAUSES[0], RETRY_PAUSES[1]], "each request has its own retries");
    assert_eq!(file_state(&session, A), FileState::Downloaded);
}

#[test]
fn an_unexpected_file_answer_is_left_for_the_next_sync() {
    // Garmin answers B with something the client does not understand (a body over the size cap, say). Retrying it now
    // would most likely get the same, and stopping would hold up every older file behind it on every sync.
    let garmin = garmin_with(&[&[strength(C, C_START), strength(B, "2026-10-21 09:00:00"), strength(A, A_START)]]);
    garmin.fail(FakeRequest::File(ActivityId(B)), [GarminAuthError::Unexpected]);
    let (mut session, handles) = connected(&garmin, false);

    let finished = run_sync(&mut session);

    assert!(finished.result.is_ok(), "{:?}", finished.result);
    assert_eq!(files_asked(&garmin), [C, B, A], "asked once, and the sync went on");
    assert_eq!(file_state(&session, C), FileState::Downloaded);
    assert_eq!(file_state(&session, A), FileState::Downloaded);
    assert_eq!(file_state(&session, B), FileState::Pending);
    assert_eq!(handles.sleep.slept(), [FILE_PAUSE, FILE_PAUSE], "the pauses between files, and no retry pause for B");
    assert_eq!(session.store().sync_state().unwrap().last_error, None);
    // B still waits, so the next sync asks for it again.
    garmin.take_requests();
    assert!(run_sync(&mut session).result.is_ok());
    assert_eq!(files_asked(&garmin), [B]);
    assert_eq!(file_state(&session, B), FileState::Downloaded);
}

/// Makes the database refuse the rows `when` matches in `table`, as a write that fails half-way would: every
/// connection to the file sees the trigger.
fn refuse_rows(path: &std::path::Path, table: &str, when: &str) {
    let sql = format!("CREATE TRIGGER refuse_{table} BEFORE INSERT ON {table} WHEN {when} BEGIN SELECT RAISE(ABORT, 'refused by the test'); END;");
    rusqlite::Connection::open(path).unwrap().execute_batch(&sql).unwrap();
}

fn accept_rows(path: &std::path::Path, table: &str) {
    rusqlite::Connection::open(path).unwrap().execute_batch(&format!("DROP TRIGGER refuse_{table};")).unwrap();
}

/// Runs `garmin.sync` to its end and returns how many of its partial results failed to apply, and its end.
fn run_sync_counting_failures(session: &mut Session) -> (usize, JobFinished) {
    let Ok(Outcome::Started(_)) = session.execute(SYNC, Value::Null, Caller::User) else { panic!("garmin.sync did not start") };
    let mut failed = 0;
    let mut finished = None;
    for event in poll_all(session) {
        match event {
            JobEvent::Partial { result, .. } => failed += usize::from(result.is_err()),
            JobEvent::Finished(end) => finished = Some(end),
        }
    }
    (failed, finished.expect("the sync ended"))
}

#[test]
fn a_page_that_cannot_be_stored_does_not_seal_the_history() {
    let dir = TempDir::new().unwrap();
    let garmin = garmin_with(&[&[running(A, A_START)], &[running(B, B_START)]]);
    let (mut session, _, path) = on_file(&dir, &garmin);
    let earlier = TestConfig::default().now - TimeDelta::days(1);
    session.store().record_sync(Some(earlier), None).unwrap();
    // The second page cannot be written; the job, which never learns it, goes on to the end of the list.
    refuse_rows(&path, "activity", &format!("NEW.id = {B}"));

    let (failed, finished) = run_sync_counting_failures(&mut session);

    assert_eq!(failed, 1, "the page that could not be written is reported");
    assert!(matches!(finished.result, Err(EngineError::Store(_))), "{:?}", finished.result);
    assert_eq!(stored(&session), 1, "the first page is stored, nothing of the second");
    let state = session.store().sync_state().unwrap();
    assert!(!state.history_complete, "a list with a gap is not the whole history");
    assert_eq!((state.last_success_at, state.last_error), (Some(earlier), Some(SyncErrorKind::Storage)));
    assert_eq!(status(&mut session).sync.last_error, Some(SyncErrorKind::Storage));

    // Once the database takes it, the next sync pages the whole list again and fills the gap.
    accept_rows(&path, "activity");
    garmin.take_requests();
    let (failed, finished) = run_sync_counting_failures(&mut session);
    assert_eq!(failed, 0);
    assert!(finished.result.is_ok(), "{:?}", finished.result);
    assert_eq!(garmin.count(RequestKind::Page), 3, "past the known activity, to the end of the list");
    assert_eq!(stored(&session), 2);
    let state = session.store().sync_state().unwrap();
    assert!(state.history_complete);
    assert_eq!((state.last_success_at, state.last_error), (Some(TestConfig::default().now), None));
}

/// A session whose whole history, O alone, is stored, and whose Garmin has since listed more than a page of new
/// activities: C and A on the first page, B on the second, with O.
fn catching_up_on(store: Store) -> (Session, Arc<FakeGarminSession>) {
    let garmin = garmin_with(&[&[running(O, O_START)]]);
    let (mut session, _) = connected_on(store, &garmin, false);
    assert!(run_sync(&mut session).result.is_ok());
    assert!(session.store().sync_state().unwrap().history_complete);
    serve(&garmin, &[&[running(C, C_START), running(A, A_START)], &[running(B, B_START), running(O, O_START)]]);
    garmin.take_requests();
    (session, garmin)
}

#[test]
fn a_catch_up_that_reaches_a_known_activity_keeps_the_history_complete() {
    let (mut session, garmin) = catching_up_on(Store::open_in_memory().unwrap());

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(stored(&session), 4);
    assert!(session.store().sync_state().unwrap().history_complete);
    // Paging stopped at O, the first known activity: the rest of the list was not asked for.
    assert_eq!(garmin.take_requests(), [FakeRequest::Page(0), FakeRequest::Page(2)]);
}

#[test]
fn an_interrupted_catch_up_is_finished_by_the_next_sync() {
    let (mut session, garmin) = catching_up_on(Store::open_in_memory().unwrap());
    // The second page fails, retries included, after the first was stored.
    garmin.fail(FakeRequest::Page(2), [GarminAuthError::Network; 4]);

    let stopped = run_sync(&mut session);

    assert_eq!(stopped.result, Err(EngineError::Garmin(GarminAuthError::Network)));
    assert_eq!(stored(&session), 3, "O, and C and A of the first page");
    // C and A are known now, and B lies behind them: a complete history would stop the next sync at C, before B.
    assert!(!session.store().sync_state().unwrap().history_complete, "the stored list has a gap");

    garmin.take_requests();
    assert!(run_sync(&mut session).result.is_ok());
    assert_eq!(stored(&session), 4, "B is imported");
    assert_eq!(row(&session, B).summary.id, ActivityId(B));
    assert!(session.store().sync_state().unwrap().history_complete);
    assert_eq!(garmin.count(RequestKind::Page), 3, "past the known activities, to the end of the list");
}

#[test]
fn a_catch_up_page_that_cannot_be_stored_is_fetched_again() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("gymgar.sqlite3");
    let (mut session, garmin) = catching_up_on(Store::open(&path).unwrap());
    // The second page cannot be written. The job, which never learns it, stops at O as usual.
    refuse_rows(&path, "activity", &format!("NEW.id = {B}"));

    let (failed, finished) = run_sync_counting_failures(&mut session);

    assert_eq!(failed, 1);
    assert!(matches!(finished.result, Err(EngineError::Store(_))), "{:?}", finished.result);
    assert_eq!(stored(&session), 3);
    assert!(!session.store().sync_state().unwrap().history_complete, "the stored list has a gap");

    accept_rows(&path, "activity");
    garmin.take_requests();
    let (failed, finished) = run_sync_counting_failures(&mut session);
    assert_eq!(failed, 0);
    assert!(finished.result.is_ok(), "{:?}", finished.result);
    assert_eq!(stored(&session), 4, "B is imported");
    assert!(session.store().sync_state().unwrap().history_complete);
    assert_eq!(garmin.count(RequestKind::Page), 3, "past the known activities, to the end of the list");
}

#[test]
fn a_page_beyond_the_history_is_not_stored_while_the_history_stays_complete() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("gymgar.sqlite3");
    let (mut session, garmin) = catching_up_on(Store::open(&path).unwrap());
    // Taking the completeness back cannot be written.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER keep_complete BEFORE UPDATE OF history_complete ON garmin_sync WHEN NEW.history_complete = 0
             BEGIN SELECT RAISE(ABORT, 'refused by the test'); END;",
        )
        .unwrap();

    let (failed, finished) = run_sync_counting_failures(&mut session);

    assert_eq!(failed, 1, "the first page, beyond the history");
    assert!(matches!(finished.result, Err(EngineError::Store(_))), "{:?}", finished.result);
    // C and A were not stored under a complete history; B, on the page that reached O, was. Nothing older than a stored
    // activity is missing, so the complete history is still true.
    assert_eq!(stored(&session), 2, "O and B");
    assert!(session.store().sync_state().unwrap().history_complete);

    rusqlite::Connection::open(&path).unwrap().execute_batch("DROP TRIGGER keep_complete;").unwrap();
    garmin.take_requests();
    let (failed, finished) = run_sync_counting_failures(&mut session);
    assert_eq!(failed, 0);
    assert!(finished.result.is_ok(), "{:?}", finished.result);
    assert_eq!(stored(&session), 4, "C and A are imported");
    assert!(session.store().sync_state().unwrap().history_complete);
}

#[test]
fn a_file_that_cannot_be_stored_leaves_nothing_half_written() {
    let dir = TempDir::new().unwrap();
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, _, path) = on_file(&dir, &garmin);
    // The file's second set cannot be written: its zip and first set are written by then, in the same transaction.
    refuse_rows(&path, "recorded_set", "NEW.message_index = 1");

    let (failed, finished) = run_sync_counting_failures(&mut session);

    assert_eq!(failed, 1);
    assert!(matches!(finished.result, Err(EngineError::Store(_))), "{:?}", finished.result);
    let detail = session.store().activity_detail(ActivityId(A)).unwrap().unwrap();
    assert_eq!(detail.row.file_state, FileState::Pending, "the activity waits for its file again");
    assert!(detail.recorded.is_empty(), "no set of the file was kept");
    let state = session.store().sync_state().unwrap();
    assert_eq!((state.last_success_at, state.last_error), (None, Some(SyncErrorKind::Storage)));

    accept_rows(&path, "recorded_set");
    assert!(run_sync(&mut session).result.is_ok());
    assert_eq!(file_state(&session, A), FileState::Downloaded);
    assert_eq!(session.store().activity_detail(ActivityId(A)).unwrap().unwrap().recorded.len(), 2);
}

#[test]
fn pause_between_files() {
    let garmin = garmin_with(&[&[strength(C, C_START), strength(A, A_START), strength(B, B_START)]]);
    let (mut session, handles) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(handles.sleep.slept(), [FILE_PAUSE, FILE_PAUSE], "a pause before every file but the first");
    assert_eq!(files_asked(&garmin), [C, A, B], "newest first");
}

#[test]
fn files_are_fetched_newest_first_across_runs() {
    // C and B wait from an earlier sync that stopped; A is new. The order is by start, not by where an id came from.
    let garmin = garmin_with(&[&[strength(C, C_START), strength(B, B_START)]]);
    garmin.fail(FakeRequest::File(ActivityId(C)), [GarminAuthError::Network; 4]);
    let (mut session, _) = connected(&garmin, false);
    assert!(run_sync(&mut session).result.is_err());
    garmin.take_requests();
    // D is an older activity uploaded late; it shows up under A.
    serve(&garmin, &[&[strength(C, C_START), strength(A, A_START), strength(B, B_START), strength(D, D_START)]]);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(files_asked(&garmin), [C, A, B, D]);
}

#[test]
fn key_rejected_stops_and_records_status() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    garmin.fail(FakeRequest::Verify, [GarminAuthError::KeyRejected]);
    let (mut session, handles) = connected(&garmin, false);

    let finished = run_sync(&mut session);

    assert_eq!(finished.result, Err(EngineError::Garmin(GarminAuthError::KeyRejected)));
    assert_eq!(garmin.count(RequestKind::Page), 0);
    assert_eq!(garmin.count(RequestKind::File), 0);
    assert!(handles.sleep.slept().is_empty(), "a refused key is not retried");
    let view = status(&mut session);
    assert_eq!(view.status, ConnectionStatus::KeyRejected);
    assert_eq!(view.sync.last_error, Some(SyncErrorKind::KeyRejected));
    // Nothing was kept: the next sync reads the key again and asks Garmin about it first.
    assert!(session.garmin_session().is_none());
    assert!(run_sync(&mut session).result.is_ok());
    assert_eq!(handles.garmin.opens(), 2);
    assert_eq!(status(&mut session).status, ConnectionStatus::Connected);
}

#[test]
fn key_rejected_in_the_middle_stops_and_records_status() {
    let garmin = garmin_with(&[&[strength(A, A_START), strength(B, B_START)]]);
    garmin.fail(FakeRequest::File(ActivityId(A)), [GarminAuthError::KeyRejected]);
    let (mut session, handles) = connected(&garmin, false);

    let finished = run_sync(&mut session);

    assert_eq!(finished.result, Err(EngineError::Garmin(GarminAuthError::KeyRejected)));
    assert_eq!(files_asked(&garmin), [A]);
    let view = status(&mut session);
    assert_eq!(view.status, ConnectionStatus::KeyRejected);
    assert_eq!(view.sync.last_error, Some(SyncErrorKind::KeyRejected));
    assert!(session.garmin_session().is_none(), "a session whose key Garmin refused is not kept");
    assert_eq!(handles.garmin.opens(), 1);
}

#[test]
fn verify_failures_are_retried_then_recorded() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    garmin.fail(FakeRequest::Verify, [GarminAuthError::Network; 4]);
    let (mut session, handles) = connected(&garmin, false);

    let finished = run_sync(&mut session);

    assert_eq!(finished.result, Err(EngineError::Garmin(GarminAuthError::Network)));
    assert_eq!(handles.sleep.slept(), RETRY_PAUSES.to_vec());
    assert_eq!(garmin.count(RequestKind::Verify), 4);
    assert_eq!(garmin.count(RequestKind::Page), 0);
    let view = status(&mut session);
    assert_eq!(view.status, ConnectionStatus::Unverified, "Garmin could not be asked, as garmin.verify records it");
    assert_eq!(view.sync.last_error, Some(SyncErrorKind::Network));
    assert!(session.garmin_session().is_none(), "a session Garmin never accepted is not kept");
}

#[test]
fn missing_key_records_key_missing() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, false);
    handles.system.as_ref().unwrap().delete().unwrap();

    let finished = run_sync(&mut session);

    assert_eq!(finished.result, Err(EngineError::KeyMissing));
    assert_eq!(handles.garmin.opens(), 0);
    assert!(garmin.requests().is_empty());
    let view = status(&mut session);
    assert_eq!(view.status, ConnectionStatus::KeyMissing);
    assert_eq!(view.sync.last_error, Some(SyncErrorKind::KeyMissing));
}

#[test]
fn unreadable_key_store_records_key_missing() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, false);
    handles.system.as_ref().unwrap().set_failing(true);

    let finished = run_sync(&mut session);

    assert_eq!(finished.result, Err(EngineError::KeyMissing), "a key that cannot be read is told about as a missing one");
    assert!(garmin.requests().is_empty());
    let view = status(&mut session);
    assert_eq!(view.status, ConnectionStatus::KeyMissing);
    assert_eq!(view.sync.last_error, Some(SyncErrorKind::KeyMissing));
}

#[test]
fn broken_file_does_not_stop_others() {
    let garmin = garmin_with(&[&[strength(B, A_START), strength(C, B_START)]]);
    garmin.set_file(ActivityId(B), b"this is not a zip".to_vec());
    let (mut session, _) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(file_state(&session, B), FileState::Unreadable);
    assert_eq!(file_state(&session, C), FileState::Downloaded);
    assert_eq!(files_asked(&garmin), [B, C]);
}

#[test]
fn a_file_garmin_does_not_have_is_missing() {
    let garmin = garmin_with(&[&[strength(A, A_START), strength(B, B_START)]]);
    garmin.fail(FakeRequest::File(ActivityId(A)), [GarminAuthError::NotFound]);
    let (mut session, handles) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(file_state(&session, A), FileState::Missing);
    assert_eq!(file_state(&session, B), FileState::Downloaded);
    assert_eq!(handles.sleep.slept(), [FILE_PAUSE], "a 404 is an answer, not a reason to retry");
    garmin.take_requests();
    assert!(run_sync(&mut session).result.is_ok());
    assert_eq!(files_asked(&garmin), Vec::<i64>::new(), "a missing file is not asked for again");
}

#[test]
fn manual_strength_is_not_downloaded() {
    // Review Focus 3: an activity entered by hand has no device file.
    let garmin = garmin_with(&[&[listed(A, "strength_training", A_START, true)]]);
    let (mut session, _) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(file_state(&session, A), FileState::Missing);
    assert_eq!(garmin.count(RequestKind::File), 0);
}

#[test]
fn a_page_that_repeats_stops_paging() {
    // A Garmin that ignores `start` would otherwise be asked for the same page forever.
    let same: &[Value] = &[strength(A, A_START), running(B, B_START)];
    let garmin = garmin_with(&[same, same, same, same]);
    let (mut session, _) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(garmin.count(RequestKind::Page), 2);
    assert_eq!(stored(&session), 2);
    assert!(!session.store().sync_state().unwrap().history_complete, "the end of the list was never seen");
}

#[test]
fn a_page_of_unreadable_elements_is_not_the_end() {
    let unreadable = [Value::Null, json!("text"), json!({ "activityId": 9, "note": "not an activity" })];
    let garmin = garmin_with(&[&unreadable, &[strength(A, A_START)]]);
    let (mut session, _) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(stored(&session), 1);
    assert_eq!(file_state(&session, A), FileState::Downloaded);
    // Each page starts after every element of the one before, readable or not, and only the empty array ends the list.
    let pages: Vec<_> = garmin.requests().into_iter().filter(|request| request.kind() == RequestKind::Page).collect();
    assert_eq!(pages, [FakeRequest::Page(0), FakeRequest::Page(3), FakeRequest::Page(4)]);
    assert!(session.store().sync_state().unwrap().history_complete);
}

#[test]
fn a_list_cut_short_after_unreadable_elements_is_not_complete() {
    let unreadable = [Value::Null, json!("text")];
    let garmin = garmin_with(&[&unreadable, &[strength(A, A_START)]]);
    garmin.fail(FakeRequest::Page(2), [GarminAuthError::Network; 4]);
    let (mut session, _) = connected(&garmin, false);

    assert_eq!(run_sync(&mut session).result, Err(EngineError::Garmin(GarminAuthError::Network)));

    assert!(!session.store().sync_state().unwrap().history_complete, "the end of the list was never seen");
    assert_eq!(stored(&session), 0);
}

#[test]
fn pages_with_nothing_readable_stop_paging() {
    // A Garmin that ignores `start` and answers elements none of which can be read would otherwise be asked forever.
    let unreadable: &[Value] = &[Value::Null, json!("text")];
    let garmin = garmin_with(&vec![unreadable; 50]);
    let (mut session, _) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(garmin.count(RequestKind::Page), 3);
    assert!(!session.store().sync_state().unwrap().history_complete);
}

#[test]
fn a_page_garmin_garbles_is_retried_then_stops() {
    let garmin = Arc::new(FakeGarminSession::new());
    garmin.set_pages(vec![b"<html>maintenance</html>".to_vec()]);
    let (mut session, handles) = connected(&garmin, false);

    let finished = run_sync(&mut session);

    assert_eq!(finished.result, Err(EngineError::Garmin(GarminAuthError::Unavailable)));
    assert_eq!(garmin.count(RequestKind::Page), 4);
    assert_eq!(handles.sleep.slept(), RETRY_PAUSES.to_vec());
    assert_eq!(status(&mut session).sync.last_error, Some(SyncErrorKind::Unavailable));
}

#[test]
fn odd_elements_of_a_page_are_skipped() {
    let garmin = garmin_with(&[&[strength(A, A_START), json!({ "activityId": 7, "note": "not an activity" }), running(B, B_START)]]);
    let (mut session, _) = connected(&garmin, false);

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(stored(&session), 2);
}

#[test]
fn sync_reuses_the_opened_session() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, false);

    for _ in 0..3 {
        assert!(run_sync(&mut session).result.is_ok());
    }

    assert_eq!(handles.garmin.opens(), 1, "the key is read and a session opened once per run of the app");
    assert_eq!(garmin.count(RequestKind::Verify), 1, "and the key exchanged once");
}

#[test]
fn a_sync_that_stopped_on_the_network_keeps_the_session() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    garmin.fail(FakeRequest::File(ActivityId(A)), [GarminAuthError::Network; 4]);
    let (mut session, handles) = connected(&garmin, false);
    assert!(run_sync(&mut session).result.is_err());

    assert!(run_sync(&mut session).result.is_ok());

    assert_eq!(handles.garmin.opens(), 1, "Garmin accepted the key; only the network failed");
}

#[test]
fn connect_drops_the_cached_session() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, false);
    assert!(run_sync(&mut session).result.is_ok());
    assert!(session.garmin_session().is_some());

    run_job_with(&mut session, "garmin.connect", json!({ "apiKey": "k-new", "keyTerm": 365 }));
    assert!(session.garmin_session().is_none(), "a new key needs a new session");

    assert!(run_sync(&mut session).result.is_ok());
    assert_eq!(handles.garmin.opens(), 2);
    assert_eq!(handles.garmin.opened_key().as_deref(), Some("k-new"));
}

fn run_job_with(session: &mut Session, id: &str, params: Value) {
    let Ok(Outcome::Started(_)) = session.execute(id, params, Caller::User) else { panic!("{id} did not start") };
    let events = poll_all(session);
    assert!(matches!(events.as_slice(), [JobEvent::Finished(JobFinished { result: Ok(_), .. })]), "{id}: {events:?}");
}

#[test]
fn disconnect_drops_the_cached_session() {
    // However it ends: once the user has asked to disconnect, the token of the key is not used again.
    for fails in [false, true] {
        let garmin = garmin_with(&[&[strength(A, A_START)]]);
        let (mut session, handles) = connected(&garmin, false);
        assert!(run_sync(&mut session).result.is_ok());
        assert!(session.garmin_session().is_some());
        handles.system.as_ref().unwrap().set_failing(fails);

        let finished = run_job(&mut session, "garmin.disconnect");
        assert_eq!(finished.result.is_ok(), !fails, "{finished:?}");

        assert!(session.garmin_session().is_none(), "the key's deletion failed: {fails}");
    }
}

#[test]
fn status_reports_progress_while_running() {
    let items: Vec<Value> = (0..20).map(|n| strength(1000 + n, &format!("2026-09-{:02} 10:00:00", n + 1))).collect();
    let garmin = Arc::new(FakeGarminSession::new());
    garmin.set_pages(vec![page(&items)]);
    for n in 0..20 {
        garmin.set_file(ActivityId(1000 + n), b"unreadable".to_vec());
    }
    let (mut session, _) = connected(&garmin, false);
    let Ok(Outcome::Started(_)) = session.execute(SYNC, Value::Null, Caller::User) else { panic!("garmin.sync did not start") };

    // The job has run to its end on the inline spawner; one poll applies only part of what it sent.
    let first = session.poll_jobs();
    assert!(first.iter().all(|event| matches!(event, JobEvent::Partial { result: Ok(()), .. })), "{first:?}");
    let sync = status(&mut session).sync;
    assert!(sync.running);
    assert_eq!(sync.files_total, 20);
    assert!(sync.files_done > 0 && sync.files_done < 20, "{sync:?}");

    let rest = poll_all(&mut session);
    assert!(matches!(rest.last(), Some(JobEvent::Finished(JobFinished { result: Ok(_), .. }))), "{rest:?}");
    let sync = status(&mut session).sync;
    assert_eq!((sync.running, sync.files_done, sync.files_total), (false, 0, 0), "the progress goes with the run");
    assert!(sync.last_success_at.is_some());
}

#[test]
fn sync_disables_key_commands_while_running() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, true);
    let spawner = handles.spawner.expect("a deferred spawner");
    let Ok(Outcome::Started(_)) = session.execute(SYNC, Value::Null, Caller::User) else { panic!("garmin.sync did not start") };

    for id in ["garmin.connect", "garmin.verify", "garmin.disconnect", SYNC] {
        assert_eq!(enabled(&session, id), Err(SYNC_RUNNING.to_owned()), "{id}");
    }
    assert!(enabled(&session, REPARSE).is_err());
    assert!(matches!(session.execute(SYNC, Value::Null, Caller::User), Err(EngineError::Disabled { .. })), "one sync at a time");
    assert!(status(&mut session).sync.running);

    spawner.run_pending();
    poll_all(&mut session);
    for id in ["garmin.connect", "garmin.verify", "garmin.disconnect", SYNC, REPARSE] {
        assert_eq!(enabled(&session, id), Ok(()), "{id}");
    }
    assert!(!status(&mut session).sync.running);
}

#[test]
fn a_key_check_disables_sync() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, true);
    let Ok(Outcome::Started(_)) = session.execute("garmin.verify", Value::Null, Caller::User) else { panic!("garmin.verify did not start") };

    assert_eq!(enabled(&session, SYNC), Err("connection check already running".to_owned()));

    handles.spawner.expect("a deferred spawner").run_pending();
    poll_all(&mut session);
    assert_eq!(enabled(&session, SYNC), Ok(()));
}

#[test]
fn sync_and_reparse_never_overlap() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, true);
    let spawner = handles.spawner.expect("a deferred spawner");

    let Ok(Outcome::Started(_)) = session.execute(REPARSE, Value::Null, Caller::User) else { panic!("activity.reparse did not start") };
    assert_eq!(enabled(&session, SYNC), Err(REPARSE_RUNNING.to_owned()));
    assert_eq!(enabled(&session, REPARSE), Err(REPARSE_RUNNING.to_owned()));
    spawner.run_pending();
    poll_all(&mut session);

    let Ok(Outcome::Started(_)) = session.execute(SYNC, Value::Null, Caller::User) else { panic!("garmin.sync did not start") };
    assert_eq!(enabled(&session, REPARSE), Err(SYNC_RUNNING.to_owned()));
    spawner.run_pending();
    poll_all(&mut session);
    assert_eq!(enabled(&session, REPARSE), Ok(()));
}

#[test]
fn sync_when_not_connected_is_disabled() {
    let mut session = test_session();
    assert_eq!(enabled(&session, SYNC), Err("Garmin Connect is not connected".to_owned()));
    assert!(matches!(session.execute(SYNC, Value::Null, Caller::User), Err(EngineError::Disabled { .. })));
}

/// The services of `gymgar-cli`: no secret stores, no Garmin, no spawner.
fn cli_like_session() -> Session {
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
fn cli_like_session_cannot_sync() {
    let mut session = cli_like_session();
    save_connection(&session, &connection_at(TestConfig::default().now, SecretBackend::System));

    for id in [SYNC, REPARSE] {
        match session.execute(id, Value::Null, Caller::User) {
            Err(EngineError::Disabled { cmd, reason }) => {
                assert_eq!(cmd, id);
                assert!(!reason.is_empty());
            }
            other => panic!("{id}: expected Disabled, got {other:?}"),
        }
    }
    assert_eq!(enabled(&session, REPARSE), Err("this session cannot run background jobs".to_owned()));
    assert!(!status(&mut session).sync.running);
}

#[test]
fn agent_cannot_sync_or_reparse() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, _) = connected(&garmin, false);
    for id in [SYNC, REPARSE] {
        assert_eq!(session.execute(id, Value::Null, Caller::Agent), Err(EngineError::AgentNotAllowed(id.to_owned())));
    }
    assert!(garmin.requests().is_empty());
}

#[test]
fn bad_params_are_errors() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, true);
    for id in [SYNC, REPARSE] {
        for params in [json!({ "full": true }), json!([1]), json!("now"), json!(7)] {
            assert!(matches!(session.execute(id, params.clone(), Caller::User), Err(EngineError::BadParams { .. })), "{id} {params}");
        }
    }
    assert_eq!(handles.spawner.expect("a deferred spawner").pending(), 0, "no job started");
    assert_eq!(session.running_jobs().count(), 0);
}

#[test]
fn sync_answer_has_no_key_and_no_raw_data() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, _) = connected(&garmin, false);
    let finished = run_sync(&mut session);
    let text = finished.result.unwrap().to_string();
    for secret in [KEY, "Not The Owner", "ownerFullName", "strength_training"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}

/// A session on a database file, so a second connection can change what no command can.
fn on_file(dir: &TempDir, garmin: &Arc<FakeGarminSession>) -> (Session, TestHandles, std::path::PathBuf) {
    let path = dir.path().join("gymgar.sqlite3");
    let (session, handles) = connected_on(Store::open(&path).unwrap(), garmin, false);
    (session, handles, path)
}

fn set_parser_version(path: &std::path::Path, version: u32) {
    rusqlite::Connection::open(path).unwrap().execute("UPDATE activity SET parser_version = ?1", [version]).unwrap();
}

#[test]
fn reparse_keeps_corrections() {
    let dir = TempDir::new().unwrap();
    let (mut session, _, path) = on_file(&dir, &Arc::new(FakeGarminSession::new()));
    let ids = seed_fixture_activities(&session);
    let id = *ids.iter().find(|id| row(&session, id.0).summary.is_strength()).unwrap();
    let before = session.store().activity_detail(id).unwrap().unwrap();
    let first = before.recorded.first().unwrap().message_index;
    let change = SetChange { reps: Some(Reps::new(12).unwrap()), ..SetChange::default() };
    session.store().edit_sets(id, &[SetRef::Recorded(first)], &change, session.now()).unwrap();
    // As if the file had been read by an older parser.
    set_parser_version(&path, 0);
    assert_eq!(session.store().files_to_reparse(PARSER_VERSION).unwrap().len(), 1);

    let finished = run_job(&mut session, REPARSE);

    assert_eq!(finished.result, Ok(json!({ "files": 1 })));
    let after = session.store().activity_detail(id).unwrap().unwrap();
    assert_eq!(after.recorded, before.recorded, "the sets are read the same way again");
    assert_eq!(after.corrections.get(&first).and_then(|correction| correction.reps), Some(Reps::new(12).unwrap()), "the correction is untouched");
    assert_eq!(after.row.file_state, FileState::Downloaded);
    assert!(session.store().files_to_reparse(PARSER_VERSION).unwrap().is_empty(), "read by this parser now");
}

#[test]
fn reparse_reads_unreadable_files_again() {
    let dir = TempDir::new().unwrap();
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    garmin.set_file(ActivityId(A), b"garbled".to_vec());
    let (mut session, _, path) = on_file(&dir, &garmin);
    assert!(run_sync(&mut session).result.is_ok());
    assert_eq!(file_state(&session, A), FileState::Unreadable);
    assert!(session.store().files_to_reparse(PARSER_VERSION).unwrap().is_empty(), "this parser has tried it already");
    set_parser_version(&path, 0);

    let finished = run_job(&mut session, REPARSE);

    assert_eq!(finished.result, Ok(json!({ "files": 1 })));
    assert_eq!(file_state(&session, A), FileState::Unreadable);
    assert!(session.store().files_to_reparse(PARSER_VERSION).unwrap().is_empty());
    assert_eq!(garmin.count(RequestKind::File), 1, "a re-parse never asks Garmin");
}

#[test]
fn reparse_with_nothing_to_read_finishes_at_once() {
    let (mut session, _) = test_session_with(TestConfig::default());

    let finished = run_job(&mut session, REPARSE);

    assert_eq!(finished.result, Ok(json!({ "files": 0 })));
}

#[test]
fn seeded_fixtures_hold_one_strength_file() {
    let session = test_session();
    let ids = seed_fixture_activities(&session);

    assert_eq!(ids.len(), 8);
    assert_eq!(stored(&session), 8);
    let strength: Vec<_> = ids.iter().map(|id| row(&session, id.0)).filter(|row| row.summary.is_strength()).collect();
    assert_eq!(strength.iter().map(|row| row.file_state).collect::<Vec<_>>(), [FileState::Downloaded, FileState::Pending]);
    let detail = session.store().activity_detail(strength.first().unwrap().summary.id).unwrap().unwrap();
    assert_eq!(detail.recorded.len(), 31);
    assert!(ids.iter().all(|id| !row(&session, id.0).is_new), "seeded like a first sync: nothing new");
}

#[test]
fn seeded_strength_holds_the_given_sets() {
    let session = test_session();
    seed_fixture_activities(&session);
    let start = at(A_START);
    let set = |message_index: u16| RecordedSet {
        message_index,
        start: start + TimeDelta::minutes(i64::from(message_index)),
        duration_ms: 30_000,
        reps: None,
        weight: None,
        exercise: None,
        step_index: None,
        plan: None,
        hr_avg: None,
        hr_max: None,
        rest_after_ms: None,
    };

    let first = seed_strength(&session, vec![set(0), set(1), set(2)]);
    let second = seed_strength(&session, Vec::new());

    assert_ne!(first, second);
    let detail = session.store().activity_detail(first).unwrap().unwrap();
    assert_eq!(detail.recorded, vec![set(0), set(1), set(2)]);
    assert!(detail.row.summary.is_strength());
    assert_eq!(detail.row.file_state, FileState::Downloaded);
    assert_eq!(session.store().activity_detail(second).unwrap().unwrap().recorded, Vec::new());
}

#[test]
fn status_has_sync_fields_in_camel_case() {
    let mut session = test_session();
    let Ok(Outcome::Done(raw)) = session.execute("garmin.status", Value::Null, Caller::Agent) else { panic!("garmin.status failed") };
    assert_eq!(raw["sync"], json!({ "running": false, "filesDone": 0, "filesTotal": 0, "lastSuccessAt": null, "lastError": null, "newCount": 0 }));
}

#[test]
fn a_dropped_sync_does_not_block_the_next_one() {
    let garmin = garmin_with(&[&[strength(A, A_START)]]);
    let (mut session, handles) = connected(&garmin, true);
    let spawner = handles.spawner.expect("a deferred spawner");
    let Ok(Outcome::Started(_)) = session.execute(SYNC, Value::Null, Caller::User) else { panic!("garmin.sync did not start") };

    spawner.drop_pending();
    let events = poll_all(&mut session);

    assert!(matches!(events.as_slice(), [JobEvent::Finished(JobFinished { result: Err(EngineError::JobDropped(_)), .. })]), "{events:?}");
    assert_eq!(enabled(&session, SYNC), Ok(()));
    assert!(!status(&mut session).sync.running);
    assert_eq!(saved_connection(&session).map(|connection| connection.last_status), Some(ConnectionStatus::Connected));
}
