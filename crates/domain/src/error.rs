//! Errors of the domain layer.
//!
//! Domain values validate themselves in their constructors; a value that is outside its allowed range is the only
//! way the domain can fail. Callers (sync, store, engine) wrap this into their own error enums.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    /// A number is outside the range the domain type accepts (or is not a finite number at all).
    #[error("{what} is out of range: {value}")]
    OutOfRange { what: &'static str, value: String },
}
