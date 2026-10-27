//! Garmin Connect commands: `garmin.status`, `garmin.connect`, `garmin.verify` and `garmin.disconnect`.
//!
//! `garmin.status` reads the database only: it never opens a secret store or contacts Garmin, so agents and the
//! CLI can call it freely. Only a key check (`garmin.verify`, or the one a sync starts with) sets statuses such as
//! `keyMissing`.
//!
//! `garmin.connect`, `garmin.verify` and `garmin.disconnect` are jobs: the Garmin request and every call of a secret
//! store run on the injected spawner, because the system store may hold a call while it asks the user for access, and
//! that must not freeze the window. Their results are applied (the connection record written or removed, the status
//! recorded) by `Session::poll_jobs` through [`apply_connect`], [`apply_verify`] and [`apply_disconnect`], on the
//! session's thread, which touches the database only. What is left to do with a store after that (the key a failed
//! connect replaced put back, the key of a replaced connection removed from its other store) goes to the spawner as well,
//! without a job of its own ([`jobs::spawn_tail`]).
//!
//! One of these jobs runs at a time; while one does, or while a sync runs, every command that changes the connection is
//! disabled. A new key drops the Garmin session the last sync kept, and so does any disconnect.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use gymgar_domain::{ConnectionStatus, GarminConnection, KeyTerm, SecretBackend, expiry_warning, key_expires_at};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::{always, done, to_json};
use crate::jobs::{self, JobOutput};
use crate::params::{self, NoParams};
use crate::sync_cmds::{self, SyncStatusView};
use crate::{CommandKind, CommandSpec, EngineError, GarminAuth, GarminAuthError, Outcome, SecretStore, SecretStores, Session, Spawner};

const STATUS: &str = "garmin.status";
const CONNECT: &str = "garmin.connect";
const VERIFY: &str = "garmin.verify";
const DISCONNECT: &str = "garmin.disconnect";

/// The jobs that check a key with Garmin. While one runs, connect, verify and disconnect are disabled, and so is a sync.
const KEY_CHECKS: [&str; 2] = [CONNECT, VERIFY];

/// `enabled` reason of commands that need a connection.
pub(crate) const NOT_CONNECTED: &str = "Garmin Connect is not connected";
/// `enabled` reason while a key check runs (Review Focus 3: a second click must not start a second check).
const CHECK_RUNNING: &str = "connection check already running";
/// `enabled` reason while a disconnect deletes the key.
const DISCONNECT_RUNNING: &str = "disconnect already running";
/// `enabled` reason in a session without secret stores (the CLI).
const NO_SECRET_STORES: &str = "this session has no access to the secret stores";
/// `enabled` reason in a session without a spawner (the CLI).
const NO_SPAWNER: &str = "this session cannot run background jobs";

/// The answer of `garmin.status` (and of `garmin.connect`, `garmin.verify` and `garmin.disconnect`): the last known
/// state of the connection.
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
    /// Session-scoped, not a fact about the machine: always `false` in the CLI, whose session has no secret stores at
    /// all, and in debug builds of the app, which never open the system store.
    pub system_store_available: bool,
    /// Where a key connected now would be saved; `None` when this session has no secret stores (the CLI).
    pub new_key_backend: Option<SecretBackend>,
    /// A key connected now could only go to a file, which needs the user's consent (`allowFileStore`).
    pub file_store_needs_consent: bool,
    /// Where the file store keeps the key, for the consent warning ("Save the key to ‹path›?"); `None` when this
    /// session has no secret stores (the CLI).
    pub file_store_location: Option<String>,
    /// A key check (connect or verify) is in flight.
    pub check_running: bool,
    /// The activity import: whether it runs, how far it got, and how the last one ended.
    pub sync: SyncStatusView,
}

/// The params of `garmin.connect`. The key never leaves this struct in readable form: its `Debug` prints a
/// placeholder, and nothing serializes it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConnectParams {
    /// The Personal API key from Garmin Connect, as pasted. Spaces and line breaks around it are ignored.
    #[schemars(with = "String")]
    pub api_key: SecretString,
    /// The key's lifetime in days, as chosen on the Garmin page when the key was issued: 30, 90, 180 or 365.
    #[schemars(schema_with = "key_term_schema")]
    pub key_term: KeyTerm,
    /// The user agreed to keep the key in a file because the system secret store is unavailable.
    #[serde(default)]
    pub allow_file_store: bool,
}

/// `KeyTerm` lives in the domain, which knows nothing of JSON Schema; its serialized form (days) is spelled out here.
fn key_term_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "enum": KeyTerm::ALL.map(|term| term.days()) })
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: STATUS,
            description: "Report the last known Garmin Connect status (notConnected, connected, keyRejected, keyMissing or unverified), \
                          when the key expires, when Garmin last accepted it, whether the expiry warning is due and where the key is \
                          stored, plus the activity import: whether it runs, files done of files to fetch, when it last succeeded, \
                          why the last one failed and how many activities are new. Reads the database only; it never contacts Garmin.",
            kind: CommandKind::Query,
            agent: true,
            destructive: false,
            params_schema: params::schema::<NoParams>,
            enabled: always,
            run: status,
        },
        CommandSpec {
            id: CONNECT,
            description: "Connect Garmin Connect with a Personal API key: exchange the key for a token in the background and, if Garmin \
                          accepts it, save the key to the secret store and the connection to the database. A rejected key saves \
                          nothing. Answers with a job id; the job's result is the new status.",
            kind: CommandKind::Job,
            agent: false,
            destructive: false,
            params_schema: params::schema::<ConnectParams>,
            enabled: connect_enabled,
            run: connect,
        },
        CommandSpec {
            id: VERIFY,
            description: "Check the saved Personal API key with Garmin in the background (one key-to-token exchange) and record the \
                          outcome: connected, keyRejected, keyMissing (the key is not in its secret store) or unverified (Garmin \
                          could not be asked; a key already marked keyRejected stays so). Answers with a job id; the job's result \
                          is the new status.",
            kind: CommandKind::Job,
            agent: false,
            destructive: false,
            params_schema: params::schema::<NoParams>,
            enabled: verify_enabled,
            run: verify,
        },
        CommandSpec {
            id: DISCONNECT,
            description: "Disconnect Garmin Connect in the background: delete the Personal API key from its secret store, then the \
                          connection record from the database. If the key cannot be deleted, the connection stays. Answers with a \
                          job id; the job's result is the new status.",
            kind: CommandKind::Job,
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
pub(crate) fn status_view(session: &Session) -> Result<GarminStatusView, EngineError> {
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
        file_store_location: secrets.and_then(|s| s.file.location()),
        check_running: check_running(session),
        sync: sync_cmds::sync_status(session)?,
    })
}

/// A key check has started and its result has not been applied yet.
fn check_running(session: &Session) -> bool {
    session.running_jobs().any(|command| KEY_CHECKS.contains(&command))
}

/// `Err` with the reason while a job that uses or changes the key runs: a key check, or a disconnect.
pub(crate) fn no_key_job_running(session: &Session) -> Result<(), String> {
    if check_running(session) {
        return Err(CHECK_RUNNING.to_owned());
    }
    if session.running_jobs().any(|command| command == DISCONNECT) {
        return Err(DISCONNECT_RUNNING.to_owned());
    }
    Ok(())
}

/// What a key check (or a sync) needs from the session's services.
pub(crate) struct CheckServices {
    pub(crate) secrets: SecretStores,
    pub(crate) garmin: Arc<dyn GarminAuth>,
    pub(crate) spawner: Arc<dyn Spawner>,
}

/// The services a key check (or a sync) needs, or why this session cannot run one.
pub(crate) fn check_services(session: &Session) -> Result<CheckServices, String> {
    let services = session.services();
    Ok(CheckServices {
        secrets: services.secrets.clone().ok_or(NO_SECRET_STORES)?,
        garmin: services.garmin.clone().ok_or("this session cannot reach Garmin")?,
        spawner: services.spawner.clone().ok_or(NO_SPAWNER)?,
    })
}

/// [`check_services`] inside `run`, where `enabled` has already passed.
pub(crate) fn check_services_for(cmd: &str, session: &Session) -> Result<CheckServices, EngineError> {
    check_services(session).map_err(|reason| EngineError::Disabled { cmd: cmd.to_owned(), reason })
}

fn connect_enabled(session: &Session) -> Result<(), String> {
    no_key_job_running(session)?;
    sync_cmds::no_sync_running(session)?;
    check_services(session).map(|_| ())
}

fn connect(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let ConnectParams { api_key, key_term, allow_file_store } = params::parse(CONNECT, params)?;
    // Review Focus 1: a key pasted with spaces or a line break around it is the same key. The trimmed key is the one
    // checked and saved; an empty one is refused here instead of being sent to Garmin.
    let key = SecretString::from(api_key.expose_secret().trim());
    if key.expose_secret().is_empty() {
        return Err(EngineError::BadParams { cmd: CONNECT.to_owned(), msg: "apiKey is empty".to_owned() });
    }
    let CheckServices { secrets, garmin, spawner } = check_services_for(CONNECT, session)?;
    // The store is chosen before the request, so a key that may not be saved is never sent.
    if secrets.needs_file_consent() && !allow_file_store {
        return Err(EngineError::FileStoreConsentRequired);
    }
    let backend = secrets.new_key_backend();
    let store = secrets.store(backend).cloned().ok_or_else(|| store_unavailable(backend))?;
    let job = session.start_job(CONNECT, &spawner, move |_| async move {
        let saved = match garmin.check_key(key.clone()).await {
            // Saved here, off the session's thread: the system store may wait for the user to allow access.
            Ok(()) => save_key(store.as_ref(), &key),
            Err(error) => Err(error.into()),
        };
        JobOutput::ConnectChecked { term: key_term, backend, saved }
    });
    Ok(Outcome::Started(job))
}

/// The error of a command that needs `backend`'s store when this session has none.
fn store_unavailable(backend: SecretBackend) -> EngineError {
    EngineError::Secrets(format!("the {} secret store is unavailable", backend.as_str()))
}

/// Saves `key`, which Garmin has accepted, to `store`, and returns the key it replaced there, to put back should the
/// connection record not be saved. Runs inside the job of `garmin.connect`.
fn save_key(store: &dyn SecretStore, key: &SecretString) -> Result<Option<SecretString>, EngineError> {
    let previous = match store.get() {
        Ok(previous) => previous,
        Err(error) => {
            // A key that cannot be read is no key at all: connecting is how the user replaces it.
            log::warn!("cannot read the key being replaced in the {} secret store: {error}", store.backend().as_str());
            None
        }
    };
    store.set(key)?;
    Ok(previous)
}

/// Applies a finished `garmin.connect`. `saved` is what the job did: the key Garmin accepted is in `backend`'s store, and
/// it replaced the key inside `Ok` there; or why there is nothing to apply (Garmin refused the key, the store failed).
/// Writes the connection record. Either the key and its record are both saved or neither is: when the record cannot be
/// saved, the replaced key is put back. A key the replaced connection kept in another store is then removed from there,
/// and the Garmin session the last sync kept, which holds the old key's token, is dropped. Every call of a secret store
/// here goes to the spawner ([`jobs::spawn_tail`]), so this thread only touches the database.
pub(crate) fn apply_connect(
    session: &mut Session,
    term: KeyTerm,
    backend: SecretBackend,
    saved: Result<Option<SecretString>, EngineError>,
) -> Result<Value, EngineError> {
    let previous = saved?;
    // Read before the new record overwrites it: where the key of the connection being replaced lives.
    let replaced_backend = match session.store().garmin_connection() {
        Ok(connection) => connection.map(|connection| connection.secret_backend),
        Err(error) => {
            log::warn!("cannot read the Garmin connection being replaced: {error}");
            None
        }
    };
    let now = session.now();
    let connection = GarminConnection {
        connected_at: now,
        key_term: term,
        key_expires_at: key_expires_at(now, term),
        last_verified_at: Some(now),
        last_status: ConnectionStatus::Connected,
        secret_backend: backend,
    };
    if let Err(error) = session.store().save_garmin_connection(&connection) {
        restore_key(session, backend, previous);
        return Err(error.into());
    }
    log::info!("Garmin Connect connected for {} days; the key is kept in the {} secret store", term.days(), backend.as_str());
    if let Some(replaced) = replaced_backend.filter(|replaced| *replaced != backend) {
        delete_replaced_key(session, replaced);
    }
    session.set_garmin_session(None);
    to_json(CONNECT, &status_view(session)?)
}

/// The store of `backend` in this session, if it has one.
fn secret_store(session: &Session, backend: SecretBackend) -> Option<Arc<dyn SecretStore>> {
    session.services().secrets.as_ref().and_then(|secrets| secrets.store(backend)).cloned()
}

/// Removes, in the background, the key of a replaced connection from `backend`, the store it was kept in, now that the
/// new key lives in another one. Only the recorded store is ever read or cleared again, so the old key would otherwise
/// stay there for good. Best effort: the new connection is saved already, so a failure is only logged.
fn delete_replaced_key(session: &Session, backend: SecretBackend) {
    let Some(store) = secret_store(session, backend) else {
        log::warn!("the {} secret store, which holds the replaced Garmin key, is unavailable; the key stays there", backend.as_str());
        return;
    };
    jobs::spawn_tail(session.services().spawner.as_ref(), "deleting the replaced Garmin key", move || match store.delete() {
        Ok(()) => log::info!("deleted the replaced Garmin key from the {} secret store", backend.as_str()),
        Err(error) => log::warn!("cannot delete the replaced Garmin key from the {} secret store: {error}", backend.as_str()),
    });
}

/// Puts back, in the background, the key a failed connect replaced in `backend`'s store, or removes the new one when
/// there was none. The connect has already failed and reports why, so a failure here can only be logged.
fn restore_key(session: &Session, backend: SecretBackend, previous: Option<SecretString>) {
    let Some(store) = secret_store(session, backend) else {
        // The job saved the key there, and a session's stores do not change, so this does not happen.
        log::error!("cannot restore the {} secret store after a failed connect: the store is unavailable", backend.as_str());
        return;
    };
    jobs::spawn_tail(session.services().spawner.as_ref(), "restoring the Garmin key after a failed connect", move || {
        let restored = match previous {
            Some(key) => store.set(&key),
            None => store.delete(),
        };
        if let Err(error) = restored {
            log::error!("cannot restore the {} secret store after a failed connect: {error}", backend.as_str());
        }
    });
}

fn verify_enabled(session: &Session) -> Result<(), String> {
    no_key_job_running(session)?;
    sync_cmds::no_sync_running(session)?;
    check_services(session)?;
    // A database failure does not grey the command out: `run` reports it as the store error it is.
    let Ok(connection) = session.store().garmin_connection() else { return Ok(()) };
    connection.map(|_| ()).ok_or_else(|| NOT_CONNECTED.to_owned())
}

fn verify(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    params::parse::<NoParams>(VERIFY, params)?;
    let CheckServices { secrets, garmin, spawner } = check_services_for(VERIFY, session)?;
    let connection = session.store().garmin_connection()?.ok_or(EngineError::NotConnected)?;
    // The key is read from the store it was saved to, never from where a new key would go now. That store may be
    // missing in this session (the store changed): then the key is missing too.
    let store = secrets.store(connection.secret_backend).cloned();
    let job = session.start_job(VERIFY, &spawner, move |_| async move {
        // Read inside the job, off the session's thread: the system store may wait for the user to allow access.
        let result = match read_key(store) {
            Ok(key) => garmin.check_key(key).await.map_err(EngineError::from),
            Err(error) => Err(error),
        };
        JobOutput::VerifyChecked { result }
    });
    Ok(Outcome::Started(job))
}

/// The key in `store`; `KeyMissing` when there is no store or no key in it.
pub(crate) fn read_key(store: Option<Arc<dyn SecretStore>>) -> Result<SecretString, EngineError> {
    store.ok_or(EngineError::KeyMissing)?.get()?.ok_or(EngineError::KeyMissing)
}

/// Applies a finished `garmin.verify`: records what the check found out, then answers with the new status, or with
/// the reason the key did not pass.
pub(crate) fn apply_verify(session: &Session, result: Result<(), EngineError>) -> Result<Value, EngineError> {
    record_verify(session, &result)?;
    result?;
    to_json(VERIFY, &status_view(session)?)
}

/// Records what a key check found out (that of `garmin.verify`, or the one a sync starts with) as the connection's
/// status. Fails only when the record cannot be read or written, or there is none.
pub(crate) fn record_verify(session: &Session, result: &Result<(), EngineError>) -> Result<(), EngineError> {
    let mut connection = session.store().garmin_connection()?.ok_or(EngineError::NotConnected)?;
    connection.last_status = match result {
        Ok(()) => ConnectionStatus::Connected,
        Err(EngineError::Garmin(GarminAuthError::KeyRejected)) => ConnectionStatus::KeyRejected,
        // The key is not where it was saved, or that store cannot be read (a locked keychain): design spec §2, item 7.
        Err(EngineError::KeyMissing | EngineError::Secrets(_)) => ConnectionStatus::KeyMissing,
        // Garmin could not be asked (network, 429, 5xx, a strange answer): nothing new is known about the key, so the
        // last known status stands (design spec §2, item 8). A key Garmin refused stays refused; it must not turn into
        // "connected, could not verify".
        Err(_) if connection.last_status == ConnectionStatus::KeyRejected => ConnectionStatus::KeyRejected,
        // Otherwise the key was there and may well work (a key found again after `keyMissing` included).
        Err(_) => ConnectionStatus::Unverified,
    };
    if result.is_ok() {
        connection.last_verified_at = Some(session.now());
    }
    // The key is never deleted here, whatever the check found (design spec §2, item 6).
    session.store().save_garmin_connection(&connection)?;
    log::info!("Garmin key check finished: {}", connection.last_status.as_str());
    Ok(())
}

/// The secret store holding the key of a connection recorded with `backend`, or why it cannot be reached.
fn key_store(session: &Session, backend: SecretBackend) -> Result<&Arc<dyn SecretStore>, String> {
    let secrets = session.services().secrets.as_ref().ok_or(NO_SECRET_STORES)?;
    secrets.store(backend).ok_or_else(|| format!("the {} secret store, which holds the key, is unavailable", backend.as_str()))
}

fn disconnect_enabled(session: &Session) -> Result<(), String> {
    no_key_job_running(session)?;
    sync_cmds::no_sync_running(session)?;
    // A database failure does not grey the command out: `run` reports it as the store error it is.
    let Ok(connection) = session.store().garmin_connection() else { return Ok(()) };
    let connection = connection.ok_or(NOT_CONNECTED)?;
    key_store(session, connection.secret_backend)?;
    session.services().spawner.as_ref().map(|_| ()).ok_or_else(|| NO_SPAWNER.to_owned())
}

fn disconnect(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    params::parse::<NoParams>(DISCONNECT, params)?;
    let connection = session.store().garmin_connection()?.ok_or(EngineError::NotConnected)?;
    let backend = connection.secret_backend;
    let store = key_store(session, backend).map_err(EngineError::Secrets)?.clone();
    let spawner = session.services().spawner.clone().ok_or_else(|| EngineError::Disabled { cmd: DISCONNECT.to_owned(), reason: NO_SPAWNER.to_owned() })?;
    // Once the user has asked to disconnect, the token of the key is not used again, however the disconnect ends.
    session.set_garmin_session(None);
    // The key goes first, in the job: the system store may wait for the user to allow access. If deleting it fails, the
    // connection stays and the user can try again; the other way round, a failure would leave a key behind that no
    // command can reach any more.
    let job =
        session.start_job(DISCONNECT, &spawner, move |_| async move { JobOutput::KeyDeleted { backend, result: store.delete().map_err(EngineError::from) } });
    Ok(Outcome::Started(job))
}

/// Applies a finished `garmin.disconnect`: the key is gone from `backend`'s store, so the connection record goes too.
/// When the key could not be deleted, the record stays, and the answer says why.
pub(crate) fn apply_disconnect(session: &Session, backend: SecretBackend, result: Result<(), EngineError>) -> Result<Value, EngineError> {
    result?;
    session.store().delete_garmin_connection()?;
    log::info!("Garmin Connect disconnected; the key was deleted from the {} secret store", backend.as_str());
    to_json(DISCONNECT, &status_view(session)?)
}

#[cfg(test)]
#[path = "garmin_cmds_tests.rs"]
mod tests;
