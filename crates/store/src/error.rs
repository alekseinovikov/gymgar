//! Store errors. Every failure of the database becomes a value of [`StoreError`]; nothing in this crate panics.
//!
//! The variants are what callers can act on: `Busy` means "try again", `Corrupt` means "this file cannot be
//! trusted", `Open` means "the database could not be brought up at this path", `NotFound` means "the record you
//! named does not exist", `Sql` is everything else.

use std::path::PathBuf;

use rusqlite::ErrorCode;

/// Why a store operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The database could not be opened or prepared (missing permissions, a schema from a newer Gymgar, ...).
    #[error("cannot open the database at {}: {reason}", path.display())]
    Open { path: PathBuf, reason: String },
    /// Another connection (usually another Gymgar process) holds the write lock and did not release it in time.
    #[error("the database is busy: another Gymgar process is writing to it, try again")]
    Busy,
    /// The file is not a SQLite database, is damaged, or holds a value this version of Gymgar cannot read.
    #[error("the database is corrupt: {0}")]
    Corrupt(String),
    /// The record a write names does not exist (an unknown activity or set); nothing was changed. The text says which.
    #[error("not found: {0}")]
    NotFound(String),
    /// Any other SQLite failure.
    #[error("database error: {0}")]
    Sql(String),
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> StoreError {
        match &error {
            rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => StoreError::Busy,
                ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt => StoreError::Corrupt(error.to_string()),
                _ => StoreError::Sql(error.to_string()),
            },
            // A stored value that does not have the column's expected type means the file was not written by us.
            rusqlite::Error::InvalidColumnType(..) | rusqlite::Error::FromSqlConversionFailure(..) | rusqlite::Error::IntegralValueOutOfRange(..) => {
                StoreError::Corrupt(error.to_string())
            }
            _ => StoreError::Sql(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::ffi;

    fn failure(code: std::os::raw::c_int) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(ffi::Error::new(code), Some("detail".to_owned()))
    }

    #[test]
    fn sqlite_codes_map_to_variants() {
        assert_eq!(StoreError::from(failure(ffi::SQLITE_BUSY)), StoreError::Busy);
        assert_eq!(StoreError::from(failure(ffi::SQLITE_LOCKED)), StoreError::Busy);
        assert!(matches!(StoreError::from(failure(ffi::SQLITE_NOTADB)), StoreError::Corrupt(_)));
        assert!(matches!(StoreError::from(failure(ffi::SQLITE_CORRUPT)), StoreError::Corrupt(_)));
        assert!(matches!(StoreError::from(failure(ffi::SQLITE_FULL)), StoreError::Sql(_)));
        assert!(matches!(StoreError::from(rusqlite::Error::QueryReturnedNoRows), StoreError::Sql(_)));
        assert!(matches!(StoreError::from(rusqlite::Error::IntegralValueOutOfRange(0, 5)), StoreError::Corrupt(_)));
    }
}
