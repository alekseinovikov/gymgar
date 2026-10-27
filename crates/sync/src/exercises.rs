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
