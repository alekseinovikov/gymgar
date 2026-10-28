//! SQLite persistence of the domain model: schema, migrations, transactions.
//!
//! L1: may depend on `gymgar-domain` only. Every schema change ships with a migration and a test
//! that opens a database written by the previous version. One mutating command = one transaction.

mod activities;
mod activity_queries;
mod corrections;
mod error;
mod garmin_connection;
mod migrations;
pub mod paths;
mod settings;
#[cfg(test)]
mod test_support;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, Transaction, TransactionBehavior};

pub use activities::{FileOutcome, NewActivity, SyncSnapshot, SyncState};
pub use activity_queries::{ActivityDetail, ActivityFilter, ActivityRow, HistorySet};
pub use corrections::SetChange;
pub use error::StoreError;

/// How long a statement waits for another connection's write lock before failing with [`StoreError::Busy`].
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// The Gymgar database. One `Store` is one SQLite connection; it is `Send` but not `Sync`, so share it behind a mutex.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating it and its parent directories if needed) the database at `path` and brings the schema up to date.
    pub fn open(path: &Path) -> Result<Store, StoreError> {
        Store::open_at(path, BUSY_TIMEOUT)
    }

    /// A private in-memory database with the current schema, for tests of crates above the store.
    pub fn open_in_memory() -> Result<Store, StoreError> {
        Store::init(Connection::open_in_memory()?, BUSY_TIMEOUT, false)
    }

    #[cfg(test)]
    fn open_with_timeout(path: &Path, timeout: Duration) -> Result<Store, StoreError> {
        Store::open_at(path, timeout)
    }

    fn open_at(path: &Path, busy_timeout: Duration) -> Result<Store, StoreError> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError::Open { path: path.to_path_buf(), reason: format!("cannot create {}: {e}", parent.display()) })?;
        }
        // SQLite opens the file lazily: a file that is not a database only fails on the first real statement.
        // Every step up to a usable connection is therefore mapped the same way: `Sql` becomes `Open { path, .. }`
        // and `Corrupt` gets the path prefixed to its text. `Busy` has no field for a path and is left as is.
        Connection::open(path).map_err(StoreError::from).and_then(|conn| Store::init(conn, busy_timeout, true)).map_err(|error| match error {
            StoreError::Sql(reason) => StoreError::Open { path: path.to_path_buf(), reason },
            StoreError::Corrupt(reason) => StoreError::Corrupt(format!("{}: {reason}", path.display())),
            other => other,
        })
    }

    fn init(mut conn: Connection, busy_timeout: Duration, wal: bool) -> Result<Store, StoreError> {
        // The timeout goes first so that switching to WAL and migrating wait for a writer instead of failing at once.
        conn.busy_timeout(busy_timeout)?;
        if wal {
            conn.pragma_update(None, "journal_mode", "WAL")?;
        }
        migrations::migrate(&mut conn)?;
        Ok(Store { conn })
    }

    /// Starts a transaction that takes the write lock at once. A deferred one that reads first and writes later can
    /// fail with `Busy` at the upgrade without waiting for `busy_timeout`; this one waits its turn up front.
    /// Dropping it without `commit` rolls everything back.
    pub(crate) fn write_tx(&self) -> Result<Transaction<'_>, StoreError> {
        Ok(Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?)
    }

    /// SQLite's `data_version`: changes when *another* connection commits to the database file, never for our own writes.
    /// The UI polls it to notice that another process (the CLI, an agent) changed the data.
    pub fn data_version(&self) -> Result<i64, StoreError> {
        Ok(self.conn.pragma_query_value(None, "data_version", |row| row.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use gymgar_domain::{ConnectionStatus, GarminConnection, KeyTerm, Language, SecretBackend};
    use tempfile::TempDir;

    fn db_in(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join("gymgar.sqlite3")
    }

    fn user_version(store: &Store) -> i64 {
        store.conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap()
    }

    #[test]
    fn store_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Store>();
    }

    #[test]
    fn migrates_fresh_db_and_reopens() {
        let dir = TempDir::new().unwrap();
        let path = db_in(&dir);
        let store = Store::open(&path).unwrap();
        assert_eq!(user_version(&store), 2);
        let journal_mode: String = store.conn.pragma_query_value(None, "journal_mode", |row| row.get(0)).unwrap();
        assert_eq!(journal_mode, "wal");
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(user_version(&reopened), 2);
    }

    #[test]
    fn migrates_v1_database_with_connection() {
        let dir = TempDir::new().unwrap();
        let path = db_in(&dir);
        {
            // A database exactly as the first release left it: schema 1 with a chosen language and a saved connection.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(migrations::MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute("INSERT INTO app_settings (key, value) VALUES ('language', 'ru')", []).unwrap();
            conn.execute(
                "INSERT INTO garmin_connection (id, connected_at, key_term_days, key_expires_at, last_verified_at, last_status, secret_backend)
                 VALUES (1, '2026-10-23T12:30:15Z', 90, '2027-01-21T12:30:15Z', '2026-10-23T13:30:15Z', 'connected', 'file')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(user_version(&store), 2);
        assert_eq!(store.language().unwrap(), Language::Ru);
        let expected = GarminConnection {
            connected_at: Utc.with_ymd_and_hms(2026, 10, 23, 12, 30, 15).unwrap(),
            key_term: KeyTerm::Days90,
            key_expires_at: Utc.with_ymd_and_hms(2027, 1, 21, 12, 30, 15).unwrap(),
            last_verified_at: Some(Utc.with_ymd_and_hms(2026, 10, 23, 13, 30, 15).unwrap()),
            last_status: ConnectionStatus::Connected,
            secret_backend: SecretBackend::File,
        };
        assert_eq!(store.garmin_connection().unwrap(), Some(expected));
        // The new tables are there, empty and usable.
        assert_eq!(store.sync_snapshot().unwrap().known.len(), 0);
        assert_eq!(store.sync_state().unwrap(), SyncState::default());
    }

    #[test]
    fn open_creates_missing_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a").join("b").join("gymgar.sqlite3");
        Store::open(&path).unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn open_in_memory_has_the_schema() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(user_version(&store), 2);
        assert_eq!(store.language().unwrap(), Language::System);
    }

    #[test]
    fn second_connection_sees_writes_and_version_changes() {
        let dir = TempDir::new().unwrap();
        let path = db_in(&dir);
        let first = Store::open(&path).unwrap();
        let second = Store::open(&path).unwrap();
        let before = second.data_version().unwrap();
        first.set_language(Language::Ru).unwrap();
        assert_ne!(second.data_version().unwrap(), before);
        assert_eq!(second.language().unwrap(), Language::Ru);
    }

    #[test]
    fn own_writes_do_not_change_data_version() {
        let store = Store::open_in_memory().unwrap();
        let before = store.data_version().unwrap();
        store.set_language(Language::En).unwrap();
        assert_eq!(store.data_version().unwrap(), before);
    }

    #[test]
    fn busy_is_reported_not_panicked() {
        let dir = TempDir::new().unwrap();
        let path = db_in(&dir);
        let holder = Store::open(&path).unwrap();
        holder.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let waiter = Store::open_with_timeout(&path, Duration::from_millis(100)).unwrap();
        assert_eq!(waiter.set_language(Language::Ru), Err(StoreError::Busy));
        // The holder's lock is released, so the same write now goes through.
        holder.conn.execute_batch("ROLLBACK").unwrap();
        waiter.set_language(Language::Ru).unwrap();
    }

    #[test]
    fn corrupt_file_is_error() {
        let dir = TempDir::new().unwrap();
        for (name, bytes) in
            [("garbage", vec![0xA5_u8; 4096]), ("text", b"this is certainly not a database file, but it is long enough to have a header".to_vec())]
        {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let result = Store::open(&path);
            assert!(matches!(result, Err(StoreError::Corrupt(_) | StoreError::Open { .. })), "{name}: {result:?}");
            // Whichever variant it is, the message must say which file is damaged.
            let message = result.err().map(|error| error.to_string()).unwrap_or_default();
            assert!(message.contains(&path.display().to_string()), "{name}: {message}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_dir_is_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        // root ignores directory permissions, so there is nothing to test when the probe file can be created.
        let is_root = std::fs::File::create(locked.join("probe")).is_ok();
        let path = locked.join("gymgar.sqlite3");
        let result = Store::open(&path);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        if is_root {
            return;
        }
        match result {
            Err(StoreError::Open { path: reported, .. }) => assert_eq!(reported, path),
            other => panic!("expected Open error, got {other:?}"),
        }
    }

    #[test]
    fn schema_from_a_newer_version_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = db_in(&dir);
        drop(Store::open(&path).unwrap());
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        match Store::open(&path) {
            Err(StoreError::Open { path: reported, reason }) => {
                assert_eq!(reported, path);
                assert!(reason.contains("99"), "{reason}");
            }
            other => panic!("expected Open error, got {other:?}"),
        }
    }
}
