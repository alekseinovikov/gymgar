//! System secret store (macOS Keychain, Windows Credential Manager, Linux Secret Service) through `keyring`.
//!
//! Not unit-tested against the real store: the only test touching it is `#[ignore]` and uses a dummy secret.

use keyring::{Entry, Error as KeyringError};
use secrecy::{ExposeSecret, SecretString};

use crate::{SERVICE, SecretsError};

/// Account of the throwaway entry `is_available` writes and deletes.
const PROBE_ACCOUNT: &str = "availability-probe";
const PROBE_VALUE: &str = "gymgar-availability-probe";

/// A secret kept in the system secret store under `service` / `account`.
#[derive(Debug, Clone)]
pub struct SystemStore {
    service: String,
    account: String,
}

impl SystemStore {
    /// A handle for the entry `service` / `account`. Nothing is touched until `get`, `set` or `delete`.
    pub fn new(service: &str, account: &str) -> SystemStore {
        SystemStore { service: service.to_owned(), account: account.to_owned() }
    }

    /// Whether the system store really works here: it initialized, and a probe entry can be written, read
    /// back and deleted. Merely having a store is not enough (a locked or absent Secret Service passes
    /// initialization on some setups). On a headless Linux box this is `false`.
    pub fn is_available() -> bool {
        probe(SERVICE)
    }

    /// The stored secret, or `None` when the entry does not exist.
    pub fn get(&self) -> Result<Option<SecretString>, SecretsError> {
        match self.entry()?.get_password() {
            Ok(password) => Ok(Some(SecretString::from(password))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(unavailable(&e)),
        }
    }

    /// Stores `secret`, replacing any previous one.
    pub fn set(&self, secret: &SecretString) -> Result<(), SecretsError> {
        self.entry()?.set_password(secret.expose_secret()).map_err(|e| unavailable(&e))
    }

    /// Removes the entry. A missing entry is not an error.
    pub fn delete(&self) -> Result<(), SecretsError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(unavailable(&e)),
        }
    }

    fn entry(&self) -> Result<Entry, SecretsError> {
        Entry::new(&self.service, &self.account).map_err(|e| unavailable(&e))
    }
}

/// Keyring errors are reported through their `Display`, which names the failure but, unlike `Debug`, never
/// includes the secret bytes carried by the encoding errors.
fn unavailable(error: &KeyringError) -> SecretsError {
    SecretsError::Unavailable(error.to_string())
}

/// Writes, reads back and deletes a throwaway entry under `service`.
fn probe(service: &str) -> bool {
    if Entry::store_status().is_err() {
        return false;
    }
    let Ok(entry) = Entry::new(service, PROBE_ACCOUNT) else {
        return false;
    };
    if entry.set_password(PROBE_VALUE).is_err() {
        return false;
    }
    let read_back = matches!(entry.get_password(), Ok(value) if value == PROBE_VALUE);
    // Clean up even when the read failed, so the probe never lingers in the user's keychain.
    let deleted = entry.delete_credential().is_ok();
    read_back && deleted
}

#[cfg(test)]
mod tests {
    use secrecy::{ExposeSecret, SecretString};

    use super::*;

    /// Manual: talks to the real system store (may show an OS access prompt). Dummy value, separate service.
    #[test]
    #[ignore = "uses the real system secret store; run manually"]
    fn system_store_round_trip_with_dummy() {
        let store = SystemStore::new("gymgar-test", "dummy");
        let value = format!("dummy-{}", "x".repeat(140));
        let secret = SecretString::from(value.clone());

        store.set(&secret).unwrap();
        let read = store.get().unwrap().unwrap();
        assert_eq!(read.expose_secret(), value);

        store.delete().unwrap();
        assert!(store.get().unwrap().is_none());
        // Deleting again is fine.
        store.delete().unwrap();
    }

    /// Manual: the availability probe against a separate service, so the real `gymgar` one stays untouched.
    #[test]
    #[ignore = "uses the real system secret store; run manually"]
    fn system_store_probe_with_test_service() {
        assert!(probe("gymgar-test"));
    }
}
