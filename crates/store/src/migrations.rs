//! Schema migrations, tracked by `PRAGMA user_version`.
//!
//! `MIGRATIONS[i]` brings a database from `user_version = i` to `i + 1`. Entries are append-only: a released
//! migration is never edited, a schema change is a new entry plus a test that opens a database written by the
//! previous version.

use rusqlite::{Connection, TransactionBehavior};

use crate::StoreError;

pub(crate) const MIGRATIONS: &[&str] = &[
    // 1: settings and the single Garmin connection record. Times are RFC 3339 strings in UTC.
    "CREATE TABLE app_settings (
         key   TEXT PRIMARY KEY,
         value TEXT NOT NULL
     );
     CREATE TABLE garmin_connection (
         id               INTEGER PRIMARY KEY CHECK (id = 1),
         connected_at     TEXT NOT NULL,
         key_term_days    INTEGER NOT NULL,
         key_expires_at   TEXT NOT NULL,
         last_verified_at TEXT,
         last_status      TEXT NOT NULL,
         secret_backend   TEXT NOT NULL
     );",
    // 2: imported Garmin activities, their strength files, the user's corrections and the sync bookkeeping.
    // Times are RFC 3339 strings in UTC (fixed width, so that they sort as text), weights are whole grams.
    // `recorded_set` is derived from the stored file and rebuilt on every re-parse; `set_correction` and `added_set`
    // are the user's and are never rebuilt. An exercise is a category plus an optional name: NULL category is "no exercise".
    "CREATE TABLE activity (
         id             INTEGER PRIMARY KEY,
         type_key       TEXT NOT NULL,
         name           TEXT,
         start_at       TEXT NOT NULL,
         duration_ms    INTEGER,
         distance_m     REAL,
         avg_hr         INTEGER,
         calories       INTEGER,
         manual         INTEGER NOT NULL CHECK (manual IN (0, 1)),
         raw_json       TEXT NOT NULL,
         first_seen_at  TEXT NOT NULL,
         seen_at        TEXT,
         excluded       INTEGER NOT NULL DEFAULT 0 CHECK (excluded IN (0, 1)),
         file_state     TEXT NOT NULL,
         parser_version INTEGER,
         workout_name   TEXT,
         counted_sets   INTEGER NOT NULL DEFAULT 0,
         exercise_count INTEGER NOT NULL DEFAULT 0
     );
     CREATE INDEX activity_start_at ON activity (start_at);
     CREATE TABLE activity_file (
         activity_id INTEGER PRIMARY KEY REFERENCES activity (id),
         zip         BLOB NOT NULL
     );
     CREATE TABLE recorded_set (
         activity_id       INTEGER NOT NULL REFERENCES activity (id),
         message_index     INTEGER NOT NULL,
         start_at          TEXT NOT NULL,
         duration_ms       INTEGER NOT NULL,
         reps              INTEGER,
         weight_g          INTEGER,
         exercise_category INTEGER,
         exercise_name     INTEGER,
         step_index        INTEGER,
         has_plan          INTEGER NOT NULL CHECK (has_plan IN (0, 1)),
         plan_reps         INTEGER,
         plan_weight_g     INTEGER,
         hr_avg            INTEGER,
         hr_max            INTEGER,
         rest_after_ms     INTEGER,
         PRIMARY KEY (activity_id, message_index)
     ) WITHOUT ROWID;
     CREATE TABLE set_correction (
         activity_id       INTEGER NOT NULL REFERENCES activity (id),
         message_index     INTEGER NOT NULL,
         weight_g          INTEGER,
         reps              INTEGER,
         exercise_category INTEGER,
         exercise_name     INTEGER,
         counted           INTEGER CHECK (counted IN (0, 1)),
         updated_at        TEXT NOT NULL,
         PRIMARY KEY (activity_id, message_index)
     ) WITHOUT ROWID;
     CREATE TABLE added_set (
         id                INTEGER PRIMARY KEY CHECK (id > 0),
         activity_id       INTEGER NOT NULL REFERENCES activity (id),
         after_kind        TEXT CHECK (after_kind IN ('recorded', 'added')),
         after_value       INTEGER,
         exercise_category INTEGER NOT NULL,
         exercise_name     INTEGER,
         weight_g          INTEGER,
         reps              INTEGER,
         counted           INTEGER CHECK (counted IN (0, 1)),
         created_at        TEXT NOT NULL,
         deleted_at        TEXT,
         CHECK ((after_kind IS NULL) = (after_value IS NULL))
     );
     CREATE INDEX added_set_activity ON added_set (activity_id);
     CREATE TABLE exercise_title (
         category INTEGER NOT NULL,
         name     INTEGER NOT NULL,
         title    TEXT NOT NULL,
         seen_at  TEXT NOT NULL,
         PRIMARY KEY (category, name)
     ) WITHOUT ROWID;
     CREATE TABLE garmin_sync (
         id               INTEGER PRIMARY KEY CHECK (id = 1),
         last_success_at  TEXT,
         last_error       TEXT,
         history_complete INTEGER NOT NULL DEFAULT 0 CHECK (history_complete IN (0, 1))
     );",
];

/// Applies every migration the database has not seen yet.
pub(crate) fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    apply(conn, MIGRATIONS)
}

fn apply(conn: &mut Connection, migrations: &[&str]) -> Result<(), StoreError> {
    // The slice length cannot realistically exceed i64; saturating keeps this total.
    let latest = i64::try_from(migrations.len()).unwrap_or(i64::MAX);
    let current = user_version(conn)?;
    if current > latest {
        return Err(StoreError::Sql(format!(
            "the database has schema version {current}, but this Gymgar only knows up to {latest}: it was written by a newer Gymgar"
        )));
    }
    for (version, sql) in (1_i64..).zip(migrations).skip_while(|(version, _)| *version <= current) {
        // Immediate: take the write lock up front, so two processes opening the same new database take turns.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // The other process may have applied this very step between our read and getting the lock.
        if user_version(&tx)? >= version {
            continue;
        }
        tx.execute_batch(sql)?;
        // user_version lives in the database header and is transactional: a failed step leaves the old version.
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

fn user_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap()
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |row| row.get::<_, i64>(0)).unwrap() == 1
    }

    #[test]
    fn applies_every_migration_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(user_version(&conn), i64::try_from(MIGRATIONS.len()).unwrap());
        assert!(table_exists(&conn, "app_settings"));
        assert!(table_exists(&conn, "garmin_connection"));
        for table in ["activity", "activity_file", "recorded_set", "set_correction", "added_set", "exercise_title", "garmin_sync"] {
            assert!(table_exists(&conn, table), "{table}");
        }
        // Running again is a no-op rather than a "table already exists" failure.
        migrate(&mut conn).unwrap();
    }

    #[test]
    fn upgrades_a_database_written_by_an_older_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &["CREATE TABLE old (x INTEGER); INSERT INTO old VALUES (7);"]).unwrap();
        assert_eq!(user_version(&conn), 1);
        apply(&mut conn, &["CREATE TABLE old (x INTEGER);", "CREATE TABLE added (y INTEGER);"]).unwrap();
        assert_eq!(user_version(&conn), 2);
        assert!(table_exists(&conn, "added"));
        assert_eq!(conn.query_row("SELECT x FROM old", [], |row| row.get::<_, i64>(0)).unwrap(), 7);
    }

    #[test]
    fn a_failing_migration_changes_nothing() {
        let mut conn = Connection::open_in_memory().unwrap();
        let result = apply(&mut conn, &["CREATE TABLE a (x INTEGER);", "CREATE TABLE b (x INTEGER); INSERT INTO missing VALUES (1);"]);
        assert!(matches!(result, Err(StoreError::Sql(_))), "{result:?}");
        // The first migration is kept, the half-applied second one is rolled back with its version bump.
        assert_eq!(user_version(&conn), 1);
        assert!(table_exists(&conn, "a"));
        assert!(!table_exists(&conn, "b"));
    }

    #[test]
    fn newer_schema_is_an_error() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 5).unwrap();
        let result = apply(&mut conn, &["CREATE TABLE a (x INTEGER);"]);
        assert!(matches!(&result, Err(StoreError::Sql(reason)) if reason.contains('5')), "{result:?}");
        assert!(!table_exists(&conn, "a"));
    }
}
