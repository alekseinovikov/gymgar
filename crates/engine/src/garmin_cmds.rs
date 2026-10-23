//! Garmin Connect commands: `garmin.status`, `garmin.connect`, `garmin.verify` and `garmin.disconnect`.
//!
//! `garmin.status` reads the database only: it never opens a secret store or contacts Garmin, so agents and the
//! CLI can call it freely. Only the key check (`garmin.verify`) sets statuses such as `keyMissing`.
//!
//! `garmin.connect` and `garmin.verify` are jobs: the Garmin request runs on the injected spawner, and the result is
//! applied (key saved, status recorded) by `Session::poll_jobs` through [`apply_connect`] and [`apply_verify`]. One
//! key check runs at a time; while it does, every command that changes the connection is disabled.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use gymgar_domain::{ConnectionStatus, GarminConnection, KeyTerm, SecretBackend, expiry_warning, key_expires_at};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::{always, done, to_json};
use crate::jobs::JobOutput;
use crate::params::{self, NoParams};
use crate::{CommandKind, CommandSpec, EngineError, GarminAuth, GarminAuthError, Outcome, SecretStore, SecretStores, Session, Spawner};

const STATUS: &str = "garmin.status";
const CONNECT: &str = "garmin.connect";
const VERIFY: &str = "garmin.verify";
const DISCONNECT: &str = "garmin.disconnect";

/// The jobs that check a key with Garmin. While one runs, connect, verify and disconnect are disabled.
const KEY_CHECKS: [&str; 2] = [CONNECT, VERIFY];

/// `enabled` reason of commands that need a connection.
const NOT_CONNECTED: &str = "Garmin Connect is not connected";
/// `enabled` reason while a key check runs (Review Focus 3: a second click must not start a second check).
const CHECK_RUNNING: &str = "connection check already running";
/// `enabled` reason in a session without secret stores (the CLI).
const NO_SECRET_STORES: &str = "this session has no access to the secret stores";

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
                          stored. Reads the database only; it never contacts Garmin.",
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
                          could not be asked). Answers with a job id; the job's result is the new status.",
            kind: CommandKind::Job,
            agent: false,
            destructive: false,
            params_schema: params::schema::<NoParams>,
            enabled: verify_enabled,
            run: verify,
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
        file_store_location: secrets.and_then(|s| s.file.location()),
        check_running: check_running(session),
    })
}

/// A key check has started and its result has not been applied yet.
fn check_running(session: &Session) -> bool {
    session.running_jobs().any(|command| KEY_CHECKS.contains(&command))
}

fn no_check_running(session: &Session) -> Result<(), String> {
    if check_running(session) { Err(CHECK_RUNNING.to_owned()) } else { Ok(()) }
}

/// What a key check needs from the session's services.
struct CheckServices {
    secrets: SecretStores,
    garmin: Arc<dyn GarminAuth>,
    spawner: Arc<dyn Spawner>,
}

/// The services a key check needs, or why this session cannot run one.
fn check_services(session: &Session) -> Result<CheckServices, String> {
    let services = session.services();
    Ok(CheckServices {
        secrets: services.secrets.clone().ok_or(NO_SECRET_STORES)?,
        garmin: services.garmin.clone().ok_or("this session cannot reach Garmin")?,
        spawner: services.spawner.clone().ok_or("this session cannot run background jobs")?,
    })
}

/// [`check_services`] inside `run`, where `enabled` has already passed.
fn check_services_for(cmd: &str, session: &Session) -> Result<CheckServices, EngineError> {
    check_services(session).map_err(|reason| EngineError::Disabled { cmd: cmd.to_owned(), reason })
}

fn connect_enabled(session: &Session) -> Result<(), String> {
    no_check_running(session)?;
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
    let job = session.start_job(CONNECT, &spawner, async move {
        let result = garmin.check_key(key.clone()).await;
        JobOutput::ConnectChecked { key, term: key_term, backend, result }
    });
    Ok(Outcome::Started(job))
}

/// Applies a finished `garmin.connect`: if Garmin accepted the key, saves it and the connection record. Either both
/// are saved or neither is: a key whose record cannot be saved is replaced by the key it replaced.
pub(crate) fn apply_connect(
    session: &Session,
    key: &SecretString,
    term: KeyTerm,
    backend: SecretBackend,
    result: Result<(), GarminAuthError>,
) -> Result<Value, EngineError> {
    result?;
    let store = session
        .services()
        .secrets
        .as_ref()
        .and_then(|secrets| secrets.store(backend))
        .ok_or_else(|| EngineError::Secrets(format!("the {} secret store is unavailable", backend.as_str())))?;
    let previous = match store.get() {
        Ok(previous) => previous,
        Err(error) => {
            // A key that cannot be read is no key at all: connecting is how the user replaces it.
            log::warn!("cannot read the key being replaced in the {} secret store: {error}", backend.as_str());
            None
        }
    };
    store.set(key)?;
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
        restore_key(store.as_ref(), previous);
        return Err(error.into());
    }
    log::info!("Garmin Connect connected for {} days; the key is kept in the {} secret store", term.days(), backend.as_str());
    to_json(CONNECT, &status_view(session)?)
}

/// Puts back the key a failed connect replaced, or removes the new one when there was none. The connect has already
/// failed and reports why, so a failure here can only be logged.
fn restore_key(store: &dyn SecretStore, previous: Option<SecretString>) {
    let restored = match previous {
        Some(key) => store.set(&key),
        None => store.delete(),
    };
    if let Err(error) = restored {
        log::error!("cannot restore the {} secret store after a failed connect: {error}", store.backend().as_str());
    }
}

fn verify_enabled(session: &Session) -> Result<(), String> {
    no_check_running(session)?;
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
    let job = session.start_job(VERIFY, &spawner, async move {
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
fn read_key(store: Option<Arc<dyn SecretStore>>) -> Result<SecretString, EngineError> {
    store.ok_or(EngineError::KeyMissing)?.get()?.ok_or(EngineError::KeyMissing)
}

/// Applies a finished `garmin.verify`: records what the check found out, then answers with the new status, or with
/// the reason the key did not pass.
pub(crate) fn apply_verify(session: &Session, result: Result<(), EngineError>) -> Result<Value, EngineError> {
    let mut connection = session.store().garmin_connection()?.ok_or(EngineError::NotConnected)?;
    connection.last_status = match &result {
        Ok(()) => ConnectionStatus::Connected,
        Err(EngineError::Garmin(GarminAuthError::KeyRejected)) => ConnectionStatus::KeyRejected,
        // The key is not where it was saved, or that store cannot be read (a locked keychain): design spec §2, item 7.
        Err(EngineError::KeyMissing | EngineError::Secrets(_)) => ConnectionStatus::KeyMissing,
        // Garmin could not be asked (network, 429, 5xx, a strange answer): nothing is known about the key.
        Err(_) => ConnectionStatus::Unverified,
    };
    if result.is_ok() {
        connection.last_verified_at = Some(session.now());
    }
    // The key is never deleted here, whatever the check found (design spec §2, item 6).
    session.store().save_garmin_connection(&connection)?;
    log::info!("Garmin key check finished: {}", connection.last_status.as_str());
    result?;
    to_json(VERIFY, &status_view(session)?)
}

/// The secret store holding the key of a connection recorded with `backend`, or why it cannot be reached.
fn key_store(session: &Session, backend: SecretBackend) -> Result<&Arc<dyn SecretStore>, String> {
    let secrets = session.services().secrets.as_ref().ok_or(NO_SECRET_STORES)?;
    secrets.store(backend).ok_or_else(|| format!("the {} secret store, which holds the key, is unavailable", backend.as_str()))
}

fn disconnect_enabled(session: &Session) -> Result<(), String> {
    no_check_running(session)?;
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
#[path = "garmin_cmds_tests.rs"]
mod tests;
