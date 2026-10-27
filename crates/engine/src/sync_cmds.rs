//! The activity import's commands: `garmin.sync` downloads from Garmin, `activity.reparse` reads the stored files
//! again with a newer parser. Both are jobs, closed to agents.
//!
//! This module decides when they may start, hands their jobs (`sync_job`) what they need read on the session's thread
//! (a snapshot of the database, the store holding the key, the Garmin session the last sync kept), and applies what
//! the jobs send, each piece in its own transaction, so whatever was applied stays when a sync stops half-way. It
//! never talks to Garmin and never parses a file itself.
//!
//! The two jobs never run together, and a sync never runs with a key check: they would write the same rows, and a sync
//! records a key check of its own. While a sync runs, the commands that change the connection are disabled
//! (`garmin_cmds` asks [`no_sync_running`]).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use gymgar_domain::{ActivityId, SyncErrorKind};
use gymgar_store::{FileOutcome, NewActivity};
use gymgar_sync::strength::PARSER_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::to_json;
use crate::garmin_cmds::{self, CheckServices};
use crate::params::{self, NoParams};
use crate::sync_job::{self, SyncInput};
use crate::{CommandKind, CommandSpec, EngineError, GarminAuthError, GarminSession, JobId, Outcome, Session};

pub(crate) const SYNC: &str = "garmin.sync";
pub(crate) const REPARSE: &str = "activity.reparse";

/// `enabled` reason while a sync runs.
pub(crate) const SYNC_RUNNING: &str = "Garmin sync already running";
/// `enabled` reason while a re-parse runs.
pub(crate) const REPARSE_RUNNING: &str = "activity re-parse already running";
/// `enabled` reason in a session without a spawner (the CLI).
const NO_SPAWNER: &str = "this session cannot run background jobs";

/// The activity import as `garmin.status` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusView {
    /// A sync has started and its end has not been applied yet.
    pub running: bool,
    /// Strength files the running sync is done with (downloaded, missing, unreadable, or left for the next sync after an
    /// answer not understood); 0 when none runs.
    pub files_done: u32,
    /// Strength files the running sync has to fetch; 0 when none runs or it is still reading the list.
    pub files_total: u32,
    /// When a sync last ran to its end.
    pub last_success_at: Option<DateTime<Utc>>,
    /// Why the last sync stopped early, or that part of what it got could not be saved; `None` after one that ran to its
    /// end and saved everything.
    pub last_error: Option<SyncErrorKind>,
    /// Activities the user has not opened yet.
    pub new_count: u32,
}

/// The answer of `activity.reparse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReparseView {
    /// Stored files read again.
    files: u32,
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: SYNC,
            description: "Download from Garmin Connect in the background: every activity of the list (newest first, a summary each) and \
                          the original file of each strength activity, one request at a time with pauses. Starts with a key check, \
                          recorded like garmin.verify. Answers with a job id; the job's result is the new status.",
            kind: CommandKind::Job,
            agent: false,
            destructive: false,
            params_schema: params::schema::<NoParams>,
            enabled: sync_enabled,
            run: sync,
        },
        CommandSpec {
            id: REPARSE,
            description: "Read the stored strength files that an older version of Gymgar read again, in the background and without \
                          the network. The user's corrections are kept. Answers with a job id; the job's result is the number of \
                          files read.",
            kind: CommandKind::Job,
            agent: false,
            destructive: false,
            params_schema: params::schema::<NoParams>,
            enabled: reparse_enabled,
            run: reparse,
        },
    ]
}

fn is_running(session: &Session, command: &str) -> bool {
    session.running_jobs().any(|running| running == command)
}

/// `Err` with the reason while a sync runs.
pub(crate) fn no_sync_running(session: &Session) -> Result<(), String> {
    if is_running(session, SYNC) { Err(SYNC_RUNNING.to_owned()) } else { Ok(()) }
}

fn no_reparse_running(session: &Session) -> Result<(), String> {
    if is_running(session, REPARSE) { Err(REPARSE_RUNNING.to_owned()) } else { Ok(()) }
}

/// The import part of `garmin.status`. The progress is shown only while a sync runs: a job that never ended (the
/// runtime dropped it) leaves no stale numbers behind.
pub(crate) fn sync_status(session: &Session) -> Result<SyncStatusView, EngineError> {
    let state = session.store().sync_state()?;
    let running = is_running(session, SYNC);
    let (files_done, files_total) = if running { session.sync_progress().unwrap_or_default() } else { (0, 0) };
    Ok(SyncStatusView {
        running,
        files_done,
        files_total,
        last_success_at: state.last_success_at,
        last_error: state.last_error,
        new_count: session.store().new_count()?,
    })
}

fn sync_enabled(session: &Session) -> Result<(), String> {
    garmin_cmds::check_services(session)?;
    no_sync_running(session)?;
    no_reparse_running(session)?;
    garmin_cmds::no_key_job_running(session)?;
    // A database failure does not grey the command out: `run` reports it as the store error it is.
    let Ok(connection) = session.store().garmin_connection() else { return Ok(()) };
    connection.map(|_| ()).ok_or_else(|| garmin_cmds::NOT_CONNECTED.to_owned())
}

fn sync(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    params::parse::<NoParams>(SYNC, params)?;
    let CheckServices { secrets, garmin, spawner } = garmin_cmds::check_services_for(SYNC, session)?;
    let connection = session.store().garmin_connection()?.ok_or(EngineError::NotConnected)?;
    let input = SyncInput {
        snapshot: session.store().sync_snapshot()?,
        // The key is read from the store it was saved to; it may be missing in this session, and then so is the key.
        key_store: secrets.store(connection.secret_backend).cloned(),
        cached: session.garmin_session(),
        garmin,
    };
    session.set_sync_progress(None);
    let job = session.start_job(SYNC, &spawner, move |ctx| sync_job::run(ctx, input));
    Ok(Outcome::Started(job))
}

fn reparse_enabled(session: &Session) -> Result<(), String> {
    session.services().spawner.as_ref().ok_or(NO_SPAWNER)?;
    no_sync_running(session)?;
    no_reparse_running(session)
}

fn reparse(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    params::parse::<NoParams>(REPARSE, params)?;
    let spawner = session.services().spawner.clone().ok_or_else(|| EngineError::Disabled { cmd: REPARSE.to_owned(), reason: NO_SPAWNER.to_owned() })?;
    // Read here, on the session's thread, so the job never looks at the database. Usually empty: only a new parser
    // version gives it anything to do, and then the job ends at once.
    let files = session.store().files_to_reparse(PARSER_VERSION)?;
    let job = session.start_job(REPARSE, &spawner, move |ctx| sync_job::reparse(ctx, files));
    Ok(Outcome::Started(job))
}

/// Records the key check a sync made. The check's own failure is not this result's: it is in the status now, and the
/// sync's end reports it.
pub(crate) fn apply_verified(session: &Session, result: &Result<(), EngineError>) -> Result<Value, EngineError> {
    garmin_cmds::record_verify(session, result)?;
    Ok(Value::Null)
}

/// Stores a page of the list. A page beyond the stored history first records that the history is not complete any more:
/// should the sync stop before paging reaches the stored activities again (the network, the window closed, a page that
/// cannot be written), the next one must not stop at the first activity it knows, which may be one of this page, with
/// older new ones behind it. It pages to the end instead.
pub(crate) fn apply_page(session: &Session, items: &[NewActivity], beyond_history: bool) -> Result<Value, EngineError> {
    if beyond_history {
        // Before the page, so that the page is never stored under a complete history: if this write fails, the page is
        // not stored either. Writing it again on every such page costs nothing and keeps that true page by page.
        session.store().set_history_complete(false)?;
    }
    session.store().upsert_activities(items, session.now())?;
    Ok(Value::Null)
}

/// Records that the whole history is stored, unless a partial result of this sync (`job`) could not be written: then the
/// stored list may have a gap, and a complete history would make every later sync stop before reaching it.
pub(crate) fn apply_history_complete(session: &Session, job: JobId) -> Result<Value, EngineError> {
    if session.partial_failed(job) {
        log::warn!("the end of the Garmin activity list is not recorded: part of this sync could not be saved");
        return Ok(Value::Null);
    }
    session.store().set_history_complete(true)?;
    Ok(Value::Null)
}

pub(crate) fn apply_file(session: &Session, id: ActivityId, outcome: FileOutcome) -> Result<Value, EngineError> {
    session.store().apply_file(id, outcome, session.now())?;
    Ok(Value::Null)
}

pub(crate) fn apply_progress(session: &mut Session, done: u32, total: u32) -> Result<Value, EngineError> {
    session.set_sync_progress(Some((done, total)));
    Ok(Value::Null)
}

/// Applies the end of a sync (`job`): records how it ended, keeps (or drops) the Garmin session for the next one, and
/// answers with the new status, or with why the sync stopped. A run part of which could not be written is no success,
/// whatever the job says: it is recorded as a storage error, and the time of the last success stays.
pub(crate) fn apply_sync_finished(
    session: &mut Session,
    job: JobId,
    result: Result<(), SyncErrorKind>,
    opened: Option<Arc<dyn GarminSession>>,
) -> Result<Value, EngineError> {
    let result = match result {
        Ok(()) if session.partial_failed(job) => Err(SyncErrorKind::Storage),
        result => result,
    };
    session.set_sync_progress(None);
    session.set_garmin_session(opened);
    let now = session.now();
    match result {
        Ok(()) => session.store().record_sync(Some(now), None)?,
        Err(kind) => session.store().record_sync(None, Some(kind))?,
    }
    log::info!("Garmin sync finished: {}", result.map_or_else(|kind| kind.as_str(), |()| "ok"));
    result.map_err(engine_error)?;
    to_json(SYNC, &garmin_cmds::status_view(session)?)
}

pub(crate) fn apply_reparse_finished(files: u32) -> Result<Value, EngineError> {
    to_json(REPARSE, &ReparseView { files })
}

/// The error a sync that stopped answers with: the one a key check or a Garmin request would have given.
fn engine_error(kind: SyncErrorKind) -> EngineError {
    match kind {
        SyncErrorKind::KeyRejected => EngineError::Garmin(GarminAuthError::KeyRejected),
        SyncErrorKind::KeyMissing => EngineError::KeyMissing,
        SyncErrorKind::RateLimited => EngineError::Garmin(GarminAuthError::RateLimited),
        SyncErrorKind::Network => EngineError::Garmin(GarminAuthError::Network),
        SyncErrorKind::Unavailable => EngineError::Garmin(GarminAuthError::Unavailable),
        SyncErrorKind::Storage => EngineError::Store("part of what Garmin sent could not be saved; the next sync fetches it again".to_owned()),
    }
}

#[cfg(test)]
#[path = "sync_cmds_tests.rs"]
mod tests;
