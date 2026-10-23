//! Secret storage for Gymgar (the Garmin Personal API key).
//!
//! Two stores with the same `get` / `set` / `delete` shape:
//!
//! - [`SystemStore`]: the OS secret store (Keychain, Credential Manager, Secret Service).
//! - [`FileStore`]: a plain file with `0600` permissions on Unix, for machines without a usable system store.
//!
//! Standalone: depends on no workspace crate. Choosing between the stores and adapting them to the engine's
//! traits is the app's job. Secrets are always [`secrecy::SecretString`]; they never appear in `Debug`,
//! `Display` or error text, so [`SecretsError`] carries only failure descriptions.

mod file;
mod system;

pub use file::FileStore;
pub use system::SystemStore;

/// Service name of every Gymgar entry in the system secret store.
pub const SERVICE: &str = "gymgar";

/// Account name of the Garmin Personal API key (system store account and file name).
pub const GARMIN_ACCOUNT: &str = "garmin-personal-api-key";

/// Why a secret could not be read, written or deleted. The text never contains a secret value.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// The system secret store is missing, locked, or refused the request.
    #[error("system secret store unavailable: {0}")]
    Unavailable(String),
    /// A secret file could not be read, written or deleted, or its content is unusable.
    #[error("secret file error: {0}")]
    Io(String),
}
