//! The engine's error type: every way a command can fail, as a value the frontends can act on.
//!
//! Texts are English because agents read them; the UI translates by variant, not by text. No variant carries a
//! secret: errors from the secret stores and from Garmin are descriptions of the failure only.

use gymgar_store::StoreError;

use crate::services::{GarminAuthError, SecretStoreError};

/// Why `Session::execute` (or a background job) failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    /// The params do not match the command's schema (wrong type, unknown key, missing field, bad value).
    #[error("bad params for {cmd}: {msg}")]
    BadParams { cmd: String, msg: String },
    /// The command exists but agents may not run it.
    #[error("command {0} is not available to agents")]
    AgentNotAllowed(String),
    /// The command cannot run in the current state; `reason` says why.
    #[error("command {cmd} is not available now: {reason}")]
    Disabled { cmd: String, reason: String },
    #[error("database error: {0}")]
    Store(String),
    #[error("secret store error: {0}")]
    Secrets(String),
    #[error(transparent)]
    Garmin(#[from] GarminAuthError),
    /// The system secret store is unavailable; the key may go to a file only after the user agrees.
    #[error("the system secret store is unavailable; saving the key to a file needs the user's consent")]
    FileStoreConsentRequired,
    #[error("Garmin Connect is not connected")]
    NotConnected,
    /// A connection is recorded but its key is not in the secret store it was saved to.
    #[error("the Garmin key is missing from the secret store")]
    KeyMissing,
    /// A background job panicked; the panic was caught and turned into this error.
    #[error("background job panicked: {0}")]
    JobPanicked(String),
    /// The spawner dropped a background job before it finished (the runtime shut down, say): whatever the job had not
    /// sent yet is lost. Holds the id of the command that started the job.
    #[error("background job was dropped before it finished: {0}")]
    JobDropped(String),
    /// A command, or the step that applies a finished job's result, panicked on the session's thread; the panic was
    /// caught and turned into this error. Holds the command id only: the params, or the job's output, may hold a key.
    #[error("command {0} panicked")]
    CommandPanicked(String),
}

impl From<StoreError> for EngineError {
    fn from(error: StoreError) -> EngineError {
        EngineError::Store(error.to_string())
    }
}

impl EngineError {
    /// A store error of command `cmd`. A record the params named that does not exist (an unknown activity or set) is a
    /// mistake of the params, so it is [`EngineError::BadParams`]; every other failure is the database's.
    pub(crate) fn from_store(cmd: &str, error: StoreError) -> EngineError {
        match error {
            StoreError::NotFound(what) => EngineError::BadParams { cmd: cmd.to_owned(), msg: format!("not found: {what}") },
            other => other.into(),
        }
    }
}

impl From<SecretStoreError> for EngineError {
    fn from(error: SecretStoreError) -> EngineError {
        EngineError::Secrets(error.0)
    }
}
