//! Typed units: weight (the load lifted, not body weight) and repetition counts.
//!
//! Weight is stored as whole grams so that sums and comparisons are exact; the user-facing unit (kg or lb) is a
//! display setting and is not decided here. Both types validate in their constructors and in `Deserialize`, so
//! a hostile value from Garmin, a file or an MCP call can never produce an out-of-range instance.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::DomainError;

const MAX_GRAMS: i64 = 1_000_000;
const MAX_KG: f64 = 1000.0;
const MAX_REPS: u16 = 1000;

/// A weight in whole grams, `0..=1000` kg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mass(i64);

impl Mass {
    /// From kilograms; must be finite and within `0.0..=1000.0`. Rounds to the nearest gram.
    pub fn from_kg(kg: f64) -> Result<Mass, DomainError> {
        if !(0.0..=MAX_KG).contains(&kg) {
            // Also catches NaN: it is in no range.
            return Err(DomainError::OutOfRange { what: "mass (kg)", value: kg.to_string() });
        }
        // In range after the check above, so the cast cannot overflow, and the value is never negative.
        let grams = (kg * 1000.0).round() as i64;
        Mass::from_grams(grams)
    }

    /// From whole grams; must be within `0..=1_000_000`.
    pub fn from_grams(grams: i64) -> Result<Mass, DomainError> {
        if (0..=MAX_GRAMS).contains(&grams) { Ok(Mass(grams)) } else { Err(DomainError::OutOfRange { what: "mass (g)", value: grams.to_string() }) }
    }

    pub fn grams(self) -> i64 {
        self.0
    }

    pub fn kg(self) -> f64 {
        // At most 1_000_000, far below 2^53, so the conversion is exact.
        let grams = self.0 as f64;
        grams / 1000.0
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

// Serialized as a number of kilograms (`12.5`), the form used in command parameters and views.
impl Serialize for Mass {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.kg())
    }
}

impl<'de> Deserialize<'de> for Mass {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Mass, D::Error> {
        Mass::from_kg(f64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A repetition count, `0..=1000`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reps(u16);

impl Reps {
    pub fn new(n: u32) -> Result<Reps, DomainError> {
        match u16::try_from(n) {
            Ok(value) if value <= MAX_REPS => Ok(Reps(value)),
            _ => Err(DomainError::OutOfRange { what: "reps", value: n.to_string() }),
        }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl Serialize for Reps {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.0)
    }
}

impl<'de> Deserialize<'de> for Reps {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Reps, D::Error> {
        Reps::new(u32::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mass_bounds() {
        assert_eq!(Mass::from_kg(77.5).unwrap().grams(), 77_500);
        assert_eq!(Mass::from_kg(1000.0).unwrap().grams(), 1_000_000);
        assert!(Mass::from_kg(-0.1).is_err());
        assert!(Mass::from_kg(1000.001).is_err());
        assert!(Mass::from_kg(f64::NAN).is_err());
        assert!(Mass::from_kg(f64::INFINITY).is_err());
        assert!(Mass::from_kg(f64::NEG_INFINITY).is_err());
        assert!(Mass::from_kg(0.0).unwrap().is_zero());
        assert!(!Mass::from_kg(0.001).unwrap().is_zero());
    }

    #[test]
    fn mass_rounds_to_the_gram() {
        assert_eq!(Mass::from_kg(0.0004).unwrap().grams(), 0);
        assert_eq!(Mass::from_kg(0.0006).unwrap().grams(), 1);
        assert_eq!(Mass::from_kg(12.3456).unwrap().grams(), 12_346);
    }

    #[test]
    fn mass_from_grams_bounds() {
        assert_eq!(Mass::from_grams(0).unwrap().grams(), 0);
        assert_eq!(Mass::from_grams(1_000_000).unwrap().kg(), 1000.0);
        assert!(Mass::from_grams(-1).is_err());
        assert!(Mass::from_grams(1_000_001).is_err());
        assert!(Mass::from_grams(i64::MAX).is_err());
        assert!(Mass::from_grams(i64::MIN).is_err());
    }

    #[test]
    fn mass_serde_is_kg() {
        assert_eq!(serde_json::to_value(Mass::from_kg(12.5).unwrap()).unwrap(), json!(12.5));
        assert_eq!(serde_json::from_value::<Mass>(json!(12.5)).unwrap().grams(), 12_500);
        assert_eq!(serde_json::from_value::<Mass>(json!(80)).unwrap().grams(), 80_000);
        assert!(serde_json::from_value::<Mass>(json!(1001)).is_err());
        assert!(serde_json::from_value::<Mass>(json!(-1)).is_err());
        assert!(serde_json::from_value::<Mass>(json!("12.5")).is_err());
    }

    #[test]
    fn reps_bounds() {
        assert_eq!(Reps::new(0).unwrap().get(), 0);
        assert_eq!(Reps::new(1000).unwrap().get(), 1000);
        assert!(Reps::new(1001).is_err());
        assert!(Reps::new(u32::MAX).is_err());
    }

    #[test]
    fn reps_serde_validates() {
        assert_eq!(serde_json::to_value(Reps::new(8).unwrap()).unwrap(), json!(8));
        assert_eq!(serde_json::from_value::<Reps>(json!(12)).unwrap().get(), 12);
        assert!(serde_json::from_value::<Reps>(json!(1001)).is_err());
        assert!(serde_json::from_value::<Reps>(json!(-1)).is_err());
        assert!(serde_json::from_value::<Reps>(json!(1.5)).is_err());
    }
}
