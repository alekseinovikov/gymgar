//! The crate's error type. No variant quotes Garmin's data: names and notes in a list or a file are the user's own text, so the
//! messages only say what was wrong, never what was in it.

use gymgar_garmin::GarminError;
use gymgar_garmin::fit::FitError;

/// Why Garmin's bytes could not be turned into domain types.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    /// A response from the Personal API is not what the mapping expects.
    #[error("unexpected data from Garmin: {0}")]
    Garmin(String),
    /// The activity file (the ZIP or the `.FIT` in it) cannot be read.
    #[error("the activity file cannot be read: {0}")]
    Fit(String),
}

impl From<GarminError> for SyncError {
    fn from(error: GarminError) -> Self {
        SyncError::Garmin(error.to_string())
    }
}

impl From<FitError> for SyncError {
    fn from(error: FitError) -> Self {
        SyncError::Fit(error.to_string())
    }
}
