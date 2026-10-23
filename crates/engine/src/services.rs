//! What the engine needs from the outside world, as traits the apps implement and inject through [`Services`].
//!
//! The engine never talks to the keychain, the network or an async runtime itself: `gymgar-secrets`,
//! `gymgar-garmin` and tokio stay out of its dependency tree, and every command can be tested with fakes
//! (`testing`). Secrets cross these traits only as [`SecretString`]; the error types carry descriptions only.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gymgar_domain::SecretBackend;
use secrecy::SecretString;

/// A boxed future that can run on any thread: what the engine hands to a [`Spawner`].
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// One place a secret (the Garmin Personal API key) can live: the system keychain or a file.
pub trait SecretStore: Send + Sync {
    /// Which store this is; recorded with the connection so the key is later read from the same place.
    fn backend(&self) -> SecretBackend;
    /// The stored key, or `None` when there is none.
    fn get(&self) -> Result<Option<SecretString>, SecretStoreError>;
    /// Stores `key`, replacing any previous one.
    fn set(&self, key: &SecretString) -> Result<(), SecretStoreError>;
    /// Removes the key; succeeds when there is none.
    fn delete(&self) -> Result<(), SecretStoreError>;
    /// Where the key lives, as text for the user: the path of a file store. `None` for stores that have no place the
    /// user could look at, such as the system keychain.
    fn location(&self) -> Option<String> {
        None
    }
}

/// A secret store failure. The text describes the failure and never contains the secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SecretStoreError(pub String);

/// The secret stores of this session.
#[derive(Clone)]
pub struct SecretStores {
    /// The system secret store, `None` when it does not answer on this machine.
    pub system: Option<Arc<dyn SecretStore>>,
    /// The file store, always present; used without asking only when `prefer_file` is set.
    pub file: Arc<dyn SecretStore>,
    /// New keys go to the file store. Set in debug builds, so macOS does not ask for keychain access after every rebuild.
    pub prefer_file: bool,
}

impl SecretStores {
    /// The store a key recorded with `backend` lives in, `None` when that store is unavailable in this session.
    pub(crate) fn store(&self, backend: SecretBackend) -> Option<&Arc<dyn SecretStore>> {
        match backend {
            SecretBackend::System => self.system.as_ref(),
            SecretBackend::File => Some(&self.file),
        }
    }

    /// Where a key connected now is saved. This is the one place the rule lives, so `garmin.status` and
    /// `garmin.connect` cannot disagree. Files are used when preferred (debug builds); otherwise the system store is
    /// used if there is one; otherwise a file, but only after the user agrees ([`SecretStores::needs_file_consent`]).
    pub fn new_key_backend(&self) -> SecretBackend {
        if self.prefer_file || self.system.is_none() { SecretBackend::File } else { SecretBackend::System }
    }

    /// Whether a new key may go to the file store only with the user's consent: there is no system store and files
    /// are not preferred anyway. `garmin.connect` refuses (`FileStoreConsentRequired`) before any network call.
    pub fn needs_file_consent(&self) -> bool {
        self.system.is_none() && !self.prefer_file
    }
}

/// Checks a Garmin Personal API key by exchanging it for a token.
pub trait GarminAuth: Send + Sync {
    /// `Ok` when Garmin accepted the key. Runs off the calling thread, on the injected [`Spawner`].
    fn check_key(&self, key: SecretString) -> BoxFuture<Result<(), GarminAuthError>>;
}

/// Why Garmin did not accept a key. Mirrors the cases the user is told about (design spec §2, item 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum GarminAuthError {
    /// Garmin refused the key (HTTP 401): wrong, expired or deleted.
    #[error("Garmin rejected the key")]
    KeyRejected,
    /// Too many requests (HTTP 429).
    #[error("Garmin is limiting requests, try again in a few minutes")]
    RateLimited,
    /// Garmin answered with a server error (HTTP 5xx).
    #[error("Garmin is unavailable, try again later")]
    Unavailable,
    /// Garmin could not be reached (no network, DNS, timeout).
    #[error("cannot reach Garmin, check the internet connection")]
    Network,
    /// Garmin answered with something the client does not understand.
    #[error("unexpected answer from Garmin")]
    Unexpected,
}

/// Runs background futures; in the desktop app this is the tokio runtime.
pub trait Spawner: Send + Sync {
    fn spawn(&self, fut: BoxFuture<()>);
}

/// Everything the engine gets from the app that runs it.
///
/// `gymgar-cli` has no secret stores, no Garmin client and no spawner (`None`): its commands work with the
/// database only, so an agent can never make it open the keychain.
#[derive(Clone)]
pub struct Services {
    pub secrets: Option<SecretStores>,
    pub garmin: Option<Arc<dyn GarminAuth>>,
    pub spawner: Option<Arc<dyn Spawner>>,
    /// Called after a background job finished, so the UI wakes up and polls (`ctx.request_repaint()`).
    pub notify: Arc<dyn Fn() + Send + Sync>,
    /// The current time; injected so tests control expiry dates and warnings.
    pub clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MemorySecretStore;

    fn stores(system: bool, prefer_file: bool) -> SecretStores {
        SecretStores {
            system: system.then(|| Arc::new(MemorySecretStore::new(SecretBackend::System)) as Arc<dyn SecretStore>),
            file: Arc::new(MemorySecretStore::new(SecretBackend::File)),
            prefer_file,
        }
    }

    #[test]
    fn a_store_has_no_location_unless_it_says_so() {
        let plain = MemorySecretStore::new(SecretBackend::System);
        assert_eq!(plain.location(), None);
        assert_eq!(MemorySecretStore::new(SecretBackend::File).with_location("/data/secrets/key").location().as_deref(), Some("/data/secrets/key"));
    }

    #[test]
    fn new_key_storage_rule() {
        // (system store present, prefer_file) -> (new key backend, needs file consent)
        let rule = [
            (true, false, SecretBackend::System, false),
            (true, true, SecretBackend::File, false),
            (false, true, SecretBackend::File, false),
            (false, false, SecretBackend::File, true),
        ];
        for (system, prefer_file, backend, consent) in rule {
            let stores = stores(system, prefer_file);
            assert_eq!(stores.new_key_backend(), backend, "system {system}, prefer_file {prefer_file}");
            assert_eq!(stores.needs_file_consent(), consent, "system {system}, prefer_file {prefer_file}");
        }
    }
}
