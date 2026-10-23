//! Garmin connection data: the Personal API key's lifetime and the last known state of the connection.
//!
//! Pure data and date arithmetic. The key itself is a secret and never appears here: the domain only knows
//! when the key was connected, how long it lives, where it is stored and whether it was last seen working.

use chrono::{DateTime, Days, TimeDelta, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The key warning starts when this much time (or less) is left before the key expires.
const EXPIRY_WARNING_WINDOW: TimeDelta = TimeDelta::days(7);

/// Lifetime the user chose when issuing the Personal API key in Garmin Connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum KeyTerm {
    Days30,
    Days90,
    Days180,
    #[default]
    Days365,
}

impl KeyTerm {
    pub const ALL: [KeyTerm; 4] = [KeyTerm::Days30, KeyTerm::Days90, KeyTerm::Days180, KeyTerm::Days365];

    /// Length of the term in days.
    pub fn days(&self) -> u16 {
        match self {
            KeyTerm::Days30 => 30,
            KeyTerm::Days90 => 90,
            KeyTerm::Days180 => 180,
            KeyTerm::Days365 => 365,
        }
    }

    /// Inverse of [`KeyTerm::days`]; `None` unless `days` is one of the offered terms.
    pub fn from_days(days: u16) -> Option<KeyTerm> {
        KeyTerm::ALL.into_iter().find(|term| term.days() == days)
    }
}

// Serialized as the number of days (`365`), the form used in command parameters and storage, not as a variant name.
impl Serialize for KeyTerm {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.days())
    }
}

impl<'de> Deserialize<'de> for KeyTerm {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<KeyTerm, D::Error> {
        let days = u16::deserialize(deserializer)?;
        KeyTerm::from_days(days).ok_or_else(|| D::Error::custom(format!("unsupported key term: {days} days (expected 30, 90, 180 or 365)")))
    }
}

/// Last known state of the Garmin connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionStatus {
    NotConnected,
    Connected,
    /// Garmin refused the key (expired or deleted on the Garmin side).
    KeyRejected,
    /// The key is gone from the secret store while the connection record still exists.
    KeyMissing,
    /// The key could not be checked, for example because the network was down.
    Unverified,
}

impl ConnectionStatus {
    /// Stable machine name, used for storage and in serialized form.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionStatus::NotConnected => "notConnected",
            ConnectionStatus::Connected => "connected",
            ConnectionStatus::KeyRejected => "keyRejected",
            ConnectionStatus::KeyMissing => "keyMissing",
            ConnectionStatus::Unverified => "unverified",
        }
    }

    /// Inverse of [`ConnectionStatus::as_str`]; `None` for anything else.
    pub fn parse(s: &str) -> Option<ConnectionStatus> {
        match s {
            "notConnected" => Some(ConnectionStatus::NotConnected),
            "connected" => Some(ConnectionStatus::Connected),
            "keyRejected" => Some(ConnectionStatus::KeyRejected),
            "keyMissing" => Some(ConnectionStatus::KeyMissing),
            "unverified" => Some(ConnectionStatus::Unverified),
            _ => None,
        }
    }
}

/// Where the Personal API key is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretBackend {
    /// The operating system's secret store (keychain).
    System,
    /// A `0600` file in the data directory, used only when the user allows it.
    File,
}

impl SecretBackend {
    /// Stable machine name, used for storage and in serialized form.
    pub fn as_str(&self) -> &'static str {
        match self {
            SecretBackend::System => "system",
            SecretBackend::File => "file",
        }
    }

    /// Inverse of [`SecretBackend::as_str`]; `None` for anything else.
    pub fn parse(s: &str) -> Option<SecretBackend> {
        match s {
            "system" => Some(SecretBackend::System),
            "file" => Some(SecretBackend::File),
            _ => None,
        }
    }
}

/// The single Garmin connection record (there is one Garmin account per Gymgar installation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GarminConnection {
    pub connected_at: DateTime<Utc>,
    pub key_term: KeyTerm,
    pub key_expires_at: DateTime<Utc>,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub last_status: ConnectionStatus,
    pub secret_backend: SecretBackend,
}

/// When a key connected at `connected_at` with the given `term` stops working.
pub fn key_expires_at(connected_at: DateTime<Utc>, term: KeyTerm) -> DateTime<Utc> {
    // Overflow needs a date around year 262000. Saturating to the latest representable moment ("never expires")
    // is the only honest answer there, and it keeps this function total.
    connected_at.checked_add_days(Days::new(u64::from(term.days()))).unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// Whether the user should be warned that the key is about to expire: 7 days or less remain, or it has already expired.
pub fn expiry_warning(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    expires_at.signed_duration_since(now) <= EXPIRY_WARNING_WINDOW
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Language;
    use chrono::{Duration, TimeZone};

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
    }

    #[test]
    fn key_term_days_and_default() {
        assert_eq!(KeyTerm::default(), KeyTerm::Days365);
        assert_eq!(KeyTerm::Days30.days(), 30);
        assert_eq!(KeyTerm::from_days(90), Some(KeyTerm::Days90));
        assert_eq!(KeyTerm::from_days(31), None);
        for term in KeyTerm::ALL {
            assert_eq!(KeyTerm::from_days(term.days()), Some(term));
        }
    }

    #[test]
    fn expires_after_term() {
        assert_eq!(key_expires_at(at(2026, 10, 23, 12), KeyTerm::Days365), at(2027, 10, 23, 12));
        assert_eq!(key_expires_at(at(2026, 10, 23, 12), KeyTerm::Days30), at(2026, 11, 22, 12));
    }

    #[test]
    fn expiry_saturates_instead_of_panicking() {
        assert_eq!(key_expires_at(DateTime::<Utc>::MAX_UTC, KeyTerm::Days365), DateTime::<Utc>::MAX_UTC);
    }

    #[test]
    fn warning_threshold() {
        let now = at(2026, 10, 23, 12);
        assert!(!expiry_warning(now + Duration::days(8), now));
        assert!(expiry_warning(now + Duration::days(7), now));
        assert!(expiry_warning(now + Duration::hours(1), now));
        assert!(expiry_warning(now - Duration::days(1), now));
    }

    #[test]
    fn language_and_status_round_trip() {
        for language in [Language::System, Language::En, Language::Ru] {
            assert_eq!(Language::parse(language.as_str()), Some(language));
        }
        assert_eq!(Language::parse("de"), None);
        for status in [
            ConnectionStatus::NotConnected,
            ConnectionStatus::Connected,
            ConnectionStatus::KeyRejected,
            ConnectionStatus::KeyMissing,
            ConnectionStatus::Unverified,
        ] {
            assert_eq!(ConnectionStatus::parse(status.as_str()), Some(status));
        }
        for backend in [SecretBackend::System, SecretBackend::File] {
            assert_eq!(SecretBackend::parse(backend.as_str()), Some(backend));
        }
        assert_eq!(ConnectionStatus::parse("bogus"), None);
        assert_eq!(SecretBackend::parse("bogus"), None);
    }

    #[test]
    fn status_and_backend_strings_are_stable() {
        let statuses = [
            (ConnectionStatus::NotConnected, "notConnected"),
            (ConnectionStatus::Connected, "connected"),
            (ConnectionStatus::KeyRejected, "keyRejected"),
            (ConnectionStatus::KeyMissing, "keyMissing"),
            (ConnectionStatus::Unverified, "unverified"),
        ];
        for (status, name) in statuses {
            assert_eq!(status.as_str(), name);
            assert_eq!(serde_json::to_string(&status).unwrap(), format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<ConnectionStatus>(&format!("\"{name}\"")).unwrap(), status);
        }
        assert_eq!(SecretBackend::System.as_str(), "system");
        assert_eq!(SecretBackend::File.as_str(), "file");
        assert_eq!(serde_json::to_string(&SecretBackend::File).unwrap(), "\"file\"");
    }

    #[test]
    fn key_term_serializes_as_days() {
        for term in KeyTerm::ALL {
            let json = serde_json::to_string(&term).unwrap();
            assert_eq!(json, term.days().to_string());
            assert_eq!(serde_json::from_str::<KeyTerm>(&json).unwrap(), term);
        }
        assert!(serde_json::from_str::<KeyTerm>("31").is_err());
        assert!(serde_json::from_str::<KeyTerm>("-1").is_err());
        assert!(serde_json::from_str::<KeyTerm>("\"Days365\"").is_err());
    }
}
