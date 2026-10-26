//! Activities as listed by Garmin: identity, summary numbers and the states of their download.
//!
//! Pure data. Parsing Garmin's JSON is `garmin`'s job and mapping it into these types is `sync`'s; this module
//! only defines what an imported activity looks like to the rest of the app.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Garmin's `activityType.typeKey` of a strength workout. Only these activities have a `.FIT` file worth downloading.
pub const STRENGTH_TYPE_KEY: &str = "strength_training";

/// Garmin's numeric activity id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActivityId(pub i64);

/// The list-level facts about an activity, as shown in the activity list.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivitySummary {
    pub id: ActivityId,
    pub type_key: String,
    pub name: Option<String>,
    pub start: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub distance_m: Option<f64>,
    pub avg_hr: Option<u16>,
    pub calories: Option<u32>,
    /// Entered by hand in Garmin Connect rather than recorded by a device.
    pub manual: bool,
}

impl ActivitySummary {
    pub fn is_strength(&self) -> bool {
        self.type_key == STRENGTH_TYPE_KEY
    }
}

/// Where the activity's `.FIT` file stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileState {
    /// Not a strength activity: there is nothing to download.
    NotNeeded,
    /// Waiting for download.
    Pending,
    Downloaded,
    /// Garmin has no file for this activity (for example a manual entry).
    Missing,
    /// The file was downloaded but could not be read.
    Unreadable,
}

impl FileState {
    /// Stable machine name, used for storage and in serialized form.
    pub fn as_str(&self) -> &'static str {
        match self {
            FileState::NotNeeded => "notNeeded",
            FileState::Pending => "pending",
            FileState::Downloaded => "downloaded",
            FileState::Missing => "missing",
            FileState::Unreadable => "unreadable",
        }
    }

    /// Inverse of [`FileState::as_str`]; `None` for anything else.
    pub fn parse(s: &str) -> Option<FileState> {
        match s {
            "notNeeded" => Some(FileState::NotNeeded),
            "pending" => Some(FileState::Pending),
            "downloaded" => Some(FileState::Downloaded),
            "missing" => Some(FileState::Missing),
            "unreadable" => Some(FileState::Unreadable),
            _ => None,
        }
    }
}

/// Why the last activity sync stopped, in terms the UI can explain and the user can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncErrorKind {
    /// Garmin answered 401: the key expired, was deleted or its term was chosen wrongly.
    KeyRejected,
    /// The key is gone from the secret store.
    KeyMissing,
    RateLimited,
    Network,
    /// Garmin answered with a server error or an unexpected response.
    Unavailable,
    /// Something Garmin sent could not be written to the database, so the run cannot count as complete.
    Storage,
}

impl SyncErrorKind {
    /// Stable machine name, used for storage and in serialized form.
    pub fn as_str(&self) -> &'static str {
        match self {
            SyncErrorKind::KeyRejected => "keyRejected",
            SyncErrorKind::KeyMissing => "keyMissing",
            SyncErrorKind::RateLimited => "rateLimited",
            SyncErrorKind::Network => "network",
            SyncErrorKind::Unavailable => "unavailable",
            SyncErrorKind::Storage => "storage",
        }
    }

    /// Inverse of [`SyncErrorKind::as_str`]; `None` for anything else.
    pub fn parse(s: &str) -> Option<SyncErrorKind> {
        match s {
            "keyRejected" => Some(SyncErrorKind::KeyRejected),
            "keyMissing" => Some(SyncErrorKind::KeyMissing),
            "rateLimited" => Some(SyncErrorKind::RateLimited),
            "network" => Some(SyncErrorKind::Network),
            "unavailable" => Some(SyncErrorKind::Unavailable),
            "storage" => Some(SyncErrorKind::Storage),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn summary(type_key: &str) -> ActivitySummary {
        ActivitySummary {
            id: ActivityId(1),
            type_key: type_key.to_string(),
            name: None,
            start: Utc.with_ymd_and_hms(2026, 10, 20, 7, 30, 0).unwrap(),
            duration_ms: Some(3_600_000),
            distance_m: None,
            avg_hr: None,
            calories: None,
            manual: false,
        }
    }

    #[test]
    fn file_state_and_sync_error_round_trip() {
        let states = [FileState::NotNeeded, FileState::Pending, FileState::Downloaded, FileState::Missing, FileState::Unreadable];
        for state in states {
            assert_eq!(FileState::parse(state.as_str()), Some(state));
        }
        let errors = [
            SyncErrorKind::KeyRejected,
            SyncErrorKind::KeyMissing,
            SyncErrorKind::RateLimited,
            SyncErrorKind::Network,
            SyncErrorKind::Unavailable,
            SyncErrorKind::Storage,
        ];
        for error in errors {
            assert_eq!(SyncErrorKind::parse(error.as_str()), Some(error));
        }
        assert_eq!(FileState::parse("bogus"), None);
        assert_eq!(FileState::parse("NotNeeded"), None);
        assert_eq!(SyncErrorKind::parse("bogus"), None);
    }

    #[test]
    fn file_state_and_sync_error_strings_are_stable_camel_case() {
        let states = [
            (FileState::NotNeeded, "notNeeded"),
            (FileState::Pending, "pending"),
            (FileState::Downloaded, "downloaded"),
            (FileState::Missing, "missing"),
            (FileState::Unreadable, "unreadable"),
        ];
        for (state, name) in states {
            assert_eq!(state.as_str(), name);
            assert_eq!(serde_json::to_string(&state).unwrap(), format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<FileState>(&format!("\"{name}\"")).unwrap(), state);
        }
        let errors = [
            (SyncErrorKind::KeyRejected, "keyRejected"),
            (SyncErrorKind::KeyMissing, "keyMissing"),
            (SyncErrorKind::RateLimited, "rateLimited"),
            (SyncErrorKind::Network, "network"),
            (SyncErrorKind::Unavailable, "unavailable"),
            (SyncErrorKind::Storage, "storage"),
        ];
        for (error, name) in errors {
            assert_eq!(error.as_str(), name);
            assert_eq!(serde_json::to_string(&error).unwrap(), format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<SyncErrorKind>(&format!("\"{name}\"")).unwrap(), error);
        }
    }

    #[test]
    fn activity_id_is_transparent() {
        assert_eq!(serde_json::to_string(&ActivityId(42)).unwrap(), "42");
        assert_eq!(serde_json::from_str::<ActivityId>("42").unwrap(), ActivityId(42));
    }

    #[test]
    fn strength_detection_uses_the_type_key() {
        assert_eq!(STRENGTH_TYPE_KEY, "strength_training");
        assert!(summary("strength_training").is_strength());
        assert!(!summary("running").is_strength());
        assert!(!summary("Strength_Training").is_strength());
    }
}
