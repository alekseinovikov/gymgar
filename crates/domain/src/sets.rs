//! Strength sets: what the watch recorded, what the user corrected, and whether a set counts.
//!
//! A recorded set is keyed by `(activity id, message_index)`; corrections and added sets live apart from the
//! recorded data, so re-parsing an activity file never touches what the user changed. This module is pure data and
//! rules: it knows nothing about where sets are stored or how they are shown.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ExerciseId, Mass, Reps};

/// A recorded set shorter than this, with no reps and no weight, is an artifact of the watch rather than work done.
pub const EMPTY_SET_MAX_MS: u32 = 10_000;

/// A set the user can point at: one the watch recorded (by its `.FIT` message index) or one the user added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetRef {
    Recorded(u16),
    Added(i64),
}

/// What the workout plan on the watch asked for in this set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedTarget {
    #[serde(default)]
    pub reps: Option<Reps>,
    #[serde(default)]
    pub weight: Option<Mass>,
}

/// One `set` message of a strength activity file, as recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedSet {
    pub message_index: u16,
    pub start: DateTime<Utc>,
    pub duration_ms: u32,
    pub reps: Option<Reps>,
    pub weight: Option<Mass>,
    /// `None` is "no exercise": the watch did not say what this set was.
    pub exercise: Option<ExerciseId>,
    pub step_index: Option<u16>,
    pub plan: Option<PlannedTarget>,
    pub hr_avg: Option<u16>,
    pub hr_max: Option<u16>,
    pub rest_after_ms: Option<u32>,
}

/// The user's corrections to a recorded set. A `None` field means "as recorded".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCorrection {
    #[serde(default)]
    pub weight: Option<Mass>,
    #[serde(default)]
    pub reps: Option<Reps>,
    #[serde(default)]
    pub exercise: Option<ExerciseId>,
    /// `Some(true)` or `Some(false)` overrides the automatic "is this set counted" decision; `None` leaves it automatic.
    #[serde(default)]
    pub counted: Option<bool>,
}

/// A set the user added by hand after the fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedSet {
    pub id: i64,
    /// The set this one follows in the list; `None` places it at the end.
    #[serde(default)]
    pub after: Option<SetRef>,
    pub exercise: ExerciseId,
    #[serde(default)]
    pub weight: Option<Mass>,
    #[serde(default)]
    pub reps: Option<Reps>,
    #[serde(default)]
    pub counted: Option<bool>,
}

/// Why a set does or does not count towards statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CountReason {
    Counted,
    ExcludedByUser,
    /// No reps, no weight and over within seconds: nothing was done (see [`is_empty_set`]).
    Empty,
    /// The whole activity is excluded.
    ActivityExcluded,
}

/// Whether a recorded set is empty: no reps (or zero), no weight (or zero) and shorter than [`EMPTY_SET_MAX_MS`].
/// Warm-ups and cardio are long enough to stay out of this.
pub fn is_empty_set(reps: Option<Reps>, weight: Option<Mass>, duration_ms: u32) -> bool {
    reps.is_none_or(|r| r.get() == 0) && weight.is_none_or(Mass::is_zero) && duration_ms < EMPTY_SET_MAX_MS
}

/// Decides whether a set counts. The order matters: an excluded activity beats everything, then the user's own
/// choice (either way) beats the automatic empty-set rule. `empty` is meaningless for added sets, so callers pass
/// `false` for them.
pub fn count_reason(activity_excluded: bool, user: Option<bool>, empty: bool) -> CountReason {
    if activity_excluded {
        return CountReason::ActivityExcluded;
    }
    match user {
        Some(false) => CountReason::ExcludedByUser,
        Some(true) => CountReason::Counted,
        None if empty => CountReason::Empty,
        None => CountReason::Counted,
    }
}

/// Everything read from one strength activity file.
#[derive(Debug, Clone, PartialEq)]
pub struct StrengthRecord {
    pub workout_name: Option<String>,
    pub sets: Vec<RecordedSet>,
    /// Exercise titles the file carries, as data for display; not translated.
    pub titles: Vec<(ExerciseId, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kg(v: f64) -> Mass {
        Mass::from_kg(v).unwrap()
    }

    fn reps(n: u32) -> Reps {
        Reps::new(n).unwrap()
    }

    #[test]
    fn empty_set_rule() {
        assert!(is_empty_set(None, None, 2_100));
        assert!(is_empty_set(Some(reps(0)), Some(kg(0.0)), 9_999));
        assert!(is_empty_set(Some(reps(0)), None, 0));
        assert!(is_empty_set(None, Some(kg(0.0)), 5_000));
        // The boundary itself is not empty.
        assert!(!is_empty_set(None, None, 10_000));
        assert_eq!(EMPTY_SET_MAX_MS, 10_000);
        assert!(!is_empty_set(Some(reps(10)), None, 2_000));
        assert!(!is_empty_set(None, Some(kg(5.0)), 2_000));
        // A warm-up with no reps and no weight, but a long duration, is counted.
        assert!(!is_empty_set(None, None, 278_350));
    }

    #[test]
    fn count_reason_order() {
        use CountReason::{ActivityExcluded, Counted, Empty, ExcludedByUser};
        // (activity_excluded, user, empty) -> reason: an excluded activity wins, then the user's choice either way, then the automatic rule.
        let table = [
            (false, None, false, Counted),
            (false, None, true, Empty),
            (false, Some(false), false, ExcludedByUser),
            (false, Some(false), true, ExcludedByUser),
            (false, Some(true), false, Counted),
            (false, Some(true), true, Counted),
            (true, None, false, ActivityExcluded),
            (true, None, true, ActivityExcluded),
            (true, Some(false), false, ActivityExcluded),
            (true, Some(false), true, ActivityExcluded),
            (true, Some(true), false, ActivityExcluded),
            (true, Some(true), true, ActivityExcluded),
        ];
        for (activity_excluded, user, empty, expected) in table {
            assert_eq!(count_reason(activity_excluded, user, empty), expected, "excluded={activity_excluded} user={user:?} empty={empty}");
        }
    }

    #[test]
    fn count_reason_serializes_camel_case() {
        assert_eq!(serde_json::to_value(CountReason::Counted).unwrap(), json!("counted"));
        assert_eq!(serde_json::to_value(CountReason::ExcludedByUser).unwrap(), json!("excludedByUser"));
        assert_eq!(serde_json::to_value(CountReason::Empty).unwrap(), json!("empty"));
        assert_eq!(serde_json::to_value(CountReason::ActivityExcluded).unwrap(), json!("activityExcluded"));
        assert_eq!(serde_json::from_value::<CountReason>(json!("excludedByUser")).unwrap(), CountReason::ExcludedByUser);
    }

    #[test]
    fn set_ref_serde() {
        assert_eq!(serde_json::to_value(SetRef::Recorded(12)).unwrap(), json!({"recorded": 12}));
        assert_eq!(serde_json::to_value(SetRef::Added(3)).unwrap(), json!({"added": 3}));
        assert_eq!(serde_json::from_value::<SetRef>(json!({"recorded": 12})).unwrap(), SetRef::Recorded(12));
        assert_eq!(serde_json::from_value::<SetRef>(json!({"added": 3})).unwrap(), SetRef::Added(3));
        assert!(serde_json::from_value::<SetRef>(json!({"recorded": -1})).is_err());
        assert!(serde_json::from_value::<SetRef>(json!({"recorded": 70000})).is_err());
        assert!(serde_json::from_value::<SetRef>(json!({"other": 1})).is_err());
    }

    #[test]
    fn correction_and_added_set_serde_round_trip() {
        let correction =
            SetCorrection { weight: Some(kg(82.5)), reps: Some(reps(8)), exercise: Some(ExerciseId { category: 5, name: Some(1) }), counted: Some(false) };
        let json = serde_json::to_value(correction).unwrap();
        assert_eq!(json, json!({"weight": 82.5, "reps": 8, "exercise": {"category": 5, "name": 1}, "counted": false}));
        assert_eq!(serde_json::from_value::<SetCorrection>(json).unwrap(), correction);
        assert_eq!(serde_json::from_value::<SetCorrection>(json!({})).unwrap(), SetCorrection::default());

        let added = AddedSet {
            id: 3,
            after: Some(SetRef::Recorded(12)),
            exercise: ExerciseId { category: 0, name: None },
            weight: Some(kg(20.0)),
            reps: None,
            counted: None,
        };
        let json = serde_json::to_value(added).unwrap();
        assert_eq!(serde_json::from_value::<AddedSet>(json).unwrap(), added);
    }
}
