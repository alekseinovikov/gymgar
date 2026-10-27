//! Exercise identity as Garmin reports it.
//!
//! Garmin identifies an exercise by a category and an optional name within it. Mapping those codes to titles is
//! data (it comes from the activity file), so this module only holds the identifier.

use serde::{Deserialize, Serialize};

/// A Garmin exercise: a category code and, when the watch knows it, a name code within that category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExerciseId {
    pub category: u16,
    #[serde(default)]
    pub name: Option<u16>,
}

impl ExerciseId {
    /// The category Garmin uses for an exercise it could not classify.
    pub const UNKNOWN_CATEGORY: u16 = 65534;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exercise_id_serde_and_constant() {
        assert_eq!(ExerciseId::UNKNOWN_CATEGORY, 65534);
        let with_name = ExerciseId { category: 5, name: Some(12) };
        let json = serde_json::to_value(with_name).unwrap();
        assert_eq!(json, json!({"category": 5, "name": 12}));
        assert_eq!(serde_json::from_value::<ExerciseId>(json).unwrap(), with_name);
        let category_only = ExerciseId { category: 5, name: None };
        assert_eq!(serde_json::to_value(category_only).unwrap(), json!({"category": 5, "name": null}));
        assert_eq!(serde_json::from_value::<ExerciseId>(json!({"category": 5})).unwrap(), category_only);
        assert!(serde_json::from_value::<ExerciseId>(json!({"category": 70000})).is_err());
    }
}
