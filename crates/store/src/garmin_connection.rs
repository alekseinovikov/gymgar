//! The single Garmin connection record (`garmin_connection`, always `id = 1`).
//!
//! Stores exactly what the domain hands over, including `key_expires_at`; it never recomputes anything.
//! The key itself is never here: it lives in the secret store.

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use gymgar_domain::{ConnectionStatus, GarminConnection, KeyTerm, SecretBackend};
use rusqlite::OptionalExtension;

use crate::{Store, StoreError};

/// The row as stored, before its strings and integers are turned back into domain values.
struct Row {
    connected_at: String,
    key_term_days: i64,
    key_expires_at: String,
    last_verified_at: Option<String>,
    last_status: String,
    secret_backend: String,
}

impl Row {
    fn into_connection(self) -> Result<GarminConnection, StoreError> {
        let key_term =
            u16::try_from(self.key_term_days).ok().and_then(KeyTerm::from_days).ok_or_else(|| corrupt("key_term_days", &self.key_term_days.to_string()))?;
        Ok(GarminConnection {
            connected_at: parse_time("connected_at", &self.connected_at)?,
            key_term,
            key_expires_at: parse_time("key_expires_at", &self.key_expires_at)?,
            last_verified_at: self.last_verified_at.as_deref().map(|value| parse_time("last_verified_at", value)).transpose()?,
            last_status: ConnectionStatus::parse(&self.last_status).ok_or_else(|| corrupt("last_status", &self.last_status))?,
            secret_backend: SecretBackend::parse(&self.secret_backend).ok_or_else(|| corrupt("secret_backend", &self.secret_backend))?,
        })
    }
}

fn corrupt(column: &str, value: &str) -> StoreError {
    StoreError::Corrupt(format!("garmin_connection.{column} holds an unusable value: {value:?}"))
}

fn parse_time(column: &str, value: &str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(value).map(|time| time.with_timezone(&Utc)).map_err(|_| corrupt(column, value))
}

/// RFC 3339 in UTC, keeping sub-second digits only when there are any so that a round trip is exact.
fn format_time(time: DateTime<Utc>) -> Result<String, StoreError> {
    // RFC 3339 has four-digit years only. Writing anything else would store a row that can never be read back
    // (the domain's expiry saturates to `MAX_UTC`), so refuse it here while nothing has been written yet.
    if !(0..=9999).contains(&time.year()) {
        return Err(StoreError::Sql(format!("the time {time} cannot be stored: its year is outside 0..=9999")));
    }
    Ok(time.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

impl Store {
    /// The connection record, or `None` when Garmin was never connected (or was disconnected).
    pub fn garmin_connection(&self) -> Result<Option<GarminConnection>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT connected_at, key_term_days, key_expires_at, last_verified_at, last_status, secret_backend FROM garmin_connection WHERE id = 1",
                [],
                |row| {
                    Ok(Row {
                        connected_at: row.get(0)?,
                        key_term_days: row.get(1)?,
                        key_expires_at: row.get(2)?,
                        last_verified_at: row.get(3)?,
                        last_status: row.get(4)?,
                        secret_backend: row.get(5)?,
                    })
                },
            )
            .optional()?;
        row.map(Row::into_connection).transpose()
    }

    /// Inserts or replaces the connection record.
    pub fn save_garmin_connection(&self, connection: &GarminConnection) -> Result<(), StoreError> {
        let connected_at = format_time(connection.connected_at)?;
        let key_expires_at = format_time(connection.key_expires_at)?;
        let last_verified_at = connection.last_verified_at.map(format_time).transpose()?;
        self.conn.execute(
            "INSERT INTO garmin_connection (id, connected_at, key_term_days, key_expires_at, last_verified_at, last_status, secret_backend)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 connected_at = excluded.connected_at,
                 key_term_days = excluded.key_term_days,
                 key_expires_at = excluded.key_expires_at,
                 last_verified_at = excluded.last_verified_at,
                 last_status = excluded.last_status,
                 secret_backend = excluded.secret_backend",
            (connected_at, connection.key_term.days(), key_expires_at, last_verified_at, connection.last_status.as_str(), connection.secret_backend.as_str()),
        )?;
        Ok(())
    }

    /// Removes the connection record; fine when there is none.
    pub fn delete_garmin_connection(&self) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM garmin_connection WHERE id = 1", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use gymgar_domain::{ConnectionStatus, KeyTerm, SecretBackend};
    use tempfile::TempDir;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 30, 15).unwrap()
    }

    fn sample() -> GarminConnection {
        GarminConnection {
            connected_at: at(23, 12),
            key_term: KeyTerm::Days90,
            // Deliberately not connected_at + 90 days: the store must keep what it is given.
            key_expires_at: at(24, 8),
            last_verified_at: Some(at(23, 13)),
            last_status: ConnectionStatus::Connected,
            secret_backend: SecretBackend::File,
        }
    }

    #[test]
    fn garmin_connection_round_trip_and_delete() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("gymgar.sqlite3")).unwrap();
        assert_eq!(store.garmin_connection().unwrap(), None);

        let connection = sample();
        store.save_garmin_connection(&connection).unwrap();
        assert_eq!(store.garmin_connection().unwrap(), Some(connection));

        store.delete_garmin_connection().unwrap();
        assert_eq!(store.garmin_connection().unwrap(), None);
        // Deleting when nothing is stored is not an error.
        store.delete_garmin_connection().unwrap();
    }

    #[test]
    fn save_replaces_the_single_row() {
        let store = Store::open_in_memory().unwrap();
        store.save_garmin_connection(&sample()).unwrap();
        let updated = GarminConnection {
            key_term: KeyTerm::Days365,
            last_verified_at: None,
            last_status: ConnectionStatus::KeyRejected,
            secret_backend: SecretBackend::System,
            ..sample()
        };
        store.save_garmin_connection(&updated).unwrap();
        assert_eq!(store.garmin_connection().unwrap(), Some(updated));
        let rows: i64 = store.conn.query_row("SELECT COUNT(*) FROM garmin_connection", [], |row| row.get(0)).unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn every_term_status_and_backend_round_trips() {
        let store = Store::open_in_memory().unwrap();
        let statuses = [
            ConnectionStatus::NotConnected,
            ConnectionStatus::Connected,
            ConnectionStatus::KeyRejected,
            ConnectionStatus::KeyMissing,
            ConnectionStatus::Unverified,
        ];
        for key_term in KeyTerm::ALL {
            for last_status in statuses {
                for secret_backend in [SecretBackend::System, SecretBackend::File] {
                    let connection = GarminConnection { key_term, last_status, secret_backend, ..sample() };
                    store.save_garmin_connection(&connection).unwrap();
                    assert_eq!(store.garmin_connection().unwrap(), Some(connection));
                }
            }
        }
    }

    #[test]
    fn sub_second_precision_survives() {
        let store = Store::open_in_memory().unwrap();
        let precise = at(23, 12) + Duration::nanoseconds(123_456_789);
        let connection = GarminConnection { connected_at: precise, last_verified_at: Some(precise), ..sample() };
        store.save_garmin_connection(&connection).unwrap();
        assert_eq!(store.garmin_connection().unwrap(), Some(connection));
    }

    #[test]
    fn times_are_stored_as_rfc3339_utc() {
        let store = Store::open_in_memory().unwrap();
        store.save_garmin_connection(&sample()).unwrap();
        let (connected_at, expires_at): (String, String) =
            store.conn.query_row("SELECT connected_at, key_expires_at FROM garmin_connection", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(connected_at, "2026-10-23T12:30:15Z");
        assert_eq!(expires_at, "2026-10-24T08:30:15Z");
    }

    #[test]
    fn unrepresentable_time_is_rejected_and_nothing_is_written() {
        let store = Store::open_in_memory().unwrap();
        // `key_expires_at` saturates to MAX_UTC in the domain; RFC 3339 cannot spell a five-digit year.
        let connection = GarminConnection { key_expires_at: DateTime::<Utc>::MAX_UTC, ..sample() };
        assert!(matches!(store.save_garmin_connection(&connection), Err(StoreError::Sql(_))));
        assert_eq!(store.garmin_connection().unwrap(), None);
    }

    #[test]
    fn bad_stored_values_are_errors() {
        let store = Store::open_in_memory().unwrap();
        // (column, bad value): each replaces one good column of an otherwise valid row.
        let bad: [(&str, &str); 7] = [
            ("last_status", "'xyz'"),
            ("secret_backend", "'cloud'"),
            ("key_term_days", "31"),
            ("key_term_days", "70000"),
            ("key_term_days", "'soon'"),
            ("connected_at", "'yesterday'"),
            ("last_verified_at", "'2026-13-45T00:00:00Z'"),
        ];
        for (column, value) in bad {
            store.save_garmin_connection(&sample()).unwrap();
            store.conn.execute(&format!("UPDATE garmin_connection SET {column} = {value}"), []).unwrap();
            let result = store.garmin_connection();
            assert!(matches!(result, Err(StoreError::Corrupt(_))), "{column} = {value}: {result:?}");
        }
    }
}
