//! Exercises in domain terms: the stable text key of an [`ExerciseId`] and Garmin's catalog of exercises.
//!
//! The numbers and names come from `gymgar-garmin`'s catalog; this module only wraps them in [`ExerciseId`], so the engine and the
//! UI never see Garmin's tuple of numbers.

use std::sync::OnceLock;

use gymgar_domain::ExerciseId;
use gymgar_garmin::fit::catalog as garmin;

/// One exercise (or a whole category) Garmin's catalog names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogExercise {
    pub id: ExerciseId,
    /// `deadlift/romanian_deadlift`, or `deadlift` for the category alone.
    pub key: String,
    /// `Romanian deadlift`, or `Deadlift`.
    pub english: String,
}

/// The stable text key of `id`: `deadlift/romanian_deadlift`, `cardio`, and for a number Garmin's catalog does not name `deadlift/77`.
pub fn exercise_key(id: ExerciseId) -> String {
    garmin::exercise_key(id.category, id.name)
}

/// The categories whose sets are not lifting: warming up, running, cycling, rowing and the other cardio or movement work.
/// Named by key, as the catalog spells them, so the list reads the same as what the user sees in an exercise's id.
const NOT_STRENGTH: [&str; 14] = [
    "warm_up",
    "cardio",
    "run",
    "bike",
    "cardio_sensors",
    "move",
    "pose",
    "elliptical",
    "floor_climb",
    "indoor_bike",
    "indoor_row",
    "stair_stepper",
    "run_indoor",
    "bike_outdoor",
];

/// Whether the exercise is strength work, the kind whose progress is worth a chart: the category decides, whatever the name
/// inside it. Garmin's "unknown" category (the watch did not know the exercise) is never strength; a category the catalog
/// does not name is, because a lift we cannot read is likelier than cardio we cannot read.
pub fn is_strength(id: ExerciseId) -> bool {
    if id.category == ExerciseId::UNKNOWN_CATEGORY {
        return false;
    }
    let category = exercise_key(ExerciseId { category: id.category, name: None });
    !NOT_STRENGTH.contains(&category.as_str())
}

/// The inverse of [`exercise_key`]; `None` for text that is not a key.
pub fn parse_exercise_key(key: &str) -> Option<ExerciseId> {
    garmin::parse_exercise_key(key).map(|(category, name)| ExerciseId { category, name })
}

/// `Romanian deadlift` for `deadlift/romanian_deadlift`; for anything the catalog does not name, the key itself.
pub fn english_name(id: ExerciseId) -> String {
    garmin::english_name(id.category, id.name)
}

/// Every exercise and category Garmin's catalog names, in number order. Built once, on first use.
pub fn catalog() -> &'static [CatalogExercise] {
    static CATALOG: OnceLock<Vec<CatalogExercise>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        garmin::all()
            .iter()
            .map(|entry| CatalogExercise {
                id: ExerciseId { category: entry.category, name: entry.name },
                key: entry.key.clone(),
                english: entry.english.clone(),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(category: u16, name: Option<u16>) -> ExerciseId {
        ExerciseId { category, name }
    }

    #[test]
    fn keys_round_trip() {
        let all = catalog();
        assert!(all.len() > 50, "the catalog has hundreds of entries, got {}", all.len());
        // 50 entries spread over the whole catalog, plus every one of them below: a key that does not come back is a bug in the catalog.
        for entry in all.iter().step_by(all.len() / 50) {
            assert_eq!(parse_exercise_key(&exercise_key(entry.id)), Some(entry.id), "{}", entry.key);
        }
        for entry in all {
            assert_eq!(exercise_key(entry.id), entry.key);
            assert_eq!(parse_exercise_key(&entry.key), Some(entry.id), "{}", entry.key);
        }
        // A number the catalog does not name keeps working in its numeric form.
        let unnamed = id(8, Some(999));
        assert_eq!(exercise_key(unnamed), "deadlift/999");
        assert_eq!(parse_exercise_key(&exercise_key(unnamed)), Some(unnamed));
        let unknown_category = id(900, Some(3));
        assert_eq!(parse_exercise_key(&exercise_key(unknown_category)), Some(unknown_category));
    }

    #[test]
    fn keys_and_names() {
        let romanian = id(8, Some(23));
        assert_eq!(exercise_key(romanian), "deadlift/romanian_deadlift");
        assert_eq!(english_name(romanian), "Romanian deadlift");
        assert_eq!(parse_exercise_key("deadlift/romanian_deadlift"), Some(romanian));
        assert_eq!(english_name(id(8, None)), "Deadlift");
        assert_eq!(exercise_key(id(8, None)), "deadlift");
        // Not a key: nothing, never a panic.
        assert_eq!(parse_exercise_key(""), None);
        assert_eq!(parse_exercise_key("not an exercise"), None);
        assert_eq!(parse_exercise_key("deadlift/99999"), None);
    }

    #[test]
    fn is_strength_drops_warm_up_cardio_and_run() {
        let by_key = |key: &str| parse_exercise_key(key).unwrap();
        // The categories that are not lifting, each by the key the catalog gives it (and with a name inside, where it has any).
        for key in [
            "warm_up",
            "cardio",
            "run",
            "bike",
            "cardio_sensors",
            "move",
            "pose",
            "elliptical",
            "floor_climb",
            "indoor_bike",
            "indoor_row",
            "stair_stepper",
            "run_indoor",
            "bike_outdoor",
        ] {
            let category = by_key(key);
            assert_eq!(category.name, None, "{key} is a category alone");
            assert!(!is_strength(category), "{key}");
            assert!(!is_strength(ExerciseId { name: Some(1), ..category }), "{key} with a name");
        }
        assert!(!is_strength(id(ExerciseId::UNKNOWN_CATEGORY, None)));
        assert!(!is_strength(id(ExerciseId::UNKNOWN_CATEGORY, Some(3))));

        assert!(is_strength(by_key("deadlift/romanian_deadlift")));
        assert!(is_strength(by_key("curl")), "a category alone");
        assert!(is_strength(by_key("plank")));
        // A number the catalog does not name is still a lift, not a guess at cardio.
        assert!(is_strength(id(900, Some(3))));
    }

    #[test]
    fn catalog_mirrors_garmins() {
        let ours = catalog();
        let theirs = garmin::all();
        assert_eq!(ours.len(), theirs.len());
        for (ours, theirs) in ours.iter().zip(theirs) {
            assert_eq!((ours.id, ours.key.as_str(), ours.english.as_str()), (id(theirs.category, theirs.name), theirs.key.as_str(), theirs.english.as_str()));
        }
        let romanian = ours.iter().find(|entry| entry.key == "deadlift/romanian_deadlift").unwrap();
        assert_eq!((romanian.id, romanian.english.as_str()), (id(8, Some(23)), "Romanian deadlift"));
        // The catalog is built once.
        assert!(std::ptr::eq(catalog(), catalog()));
    }
}
