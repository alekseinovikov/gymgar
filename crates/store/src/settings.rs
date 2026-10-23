//! Application settings stored in the `app_settings` key/value table.
//!
//! One method pair per setting, so the stored string never leaks out of this module.

use gymgar_domain::Language;
use rusqlite::OptionalExtension;

use crate::{Store, StoreError};

const LANGUAGE_KEY: &str = "language";

impl Store {
    /// The interface language; `System` until the user picks one.
    pub fn language(&self) -> Result<Language, StoreError> {
        match self.setting(LANGUAGE_KEY)? {
            None => Ok(Language::System),
            Some(value) => Language::parse(&value).ok_or_else(|| StoreError::Corrupt(format!("unknown language setting {value:?}"))),
        }
    }

    pub fn set_language(&self, language: Language) -> Result<(), StoreError> {
        self.set_setting(LANGUAGE_KEY, language.as_str())
    }

    fn setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self.conn.query_row("SELECT value FROM app_settings WHERE key = ?1", [key], |row| row.get(0)).optional()?)
    }

    fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn.execute("INSERT INTO app_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [key, value])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn language_defaults_to_system_and_persists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("gymgar.sqlite3");
        let store = Store::open(&path).unwrap();
        assert_eq!(store.language().unwrap(), Language::System);
        store.set_language(Language::Ru).unwrap();
        assert_eq!(store.language().unwrap(), Language::Ru);
        store.set_language(Language::En).unwrap();
        assert_eq!(store.language().unwrap(), Language::En);
        drop(store);
        assert_eq!(Store::open(&path).unwrap().language().unwrap(), Language::En);
    }

    #[test]
    fn unknown_stored_language_is_an_error() {
        let store = Store::open_in_memory().unwrap();
        store.conn.execute("INSERT INTO app_settings (key, value) VALUES ('language', 'klingon')", []).unwrap();
        assert!(matches!(store.language(), Err(StoreError::Corrupt(_))));
        // Choosing a language repairs it.
        store.set_language(Language::En).unwrap();
        assert_eq!(store.language().unwrap(), Language::En);
    }
}
