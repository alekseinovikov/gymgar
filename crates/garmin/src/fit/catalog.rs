//! Garmin's exercise catalog: which exercises a watch can record, under stable text keys.
//!
//! FIT numbers an exercise by `(category, name)`: `8, 23` is the Romanian deadlift. The numbers are Garmin's and fitparser's profile
//! names the known ones, but a number is a poor key for storage and for people: [`exercise_key`] turns the pair into
//! `"deadlift/romanian_deadlift"`, and [`parse_exercise_key`] turns it back. A number the profile does not name (a newer watch) keeps
//! working in the numeric forms `"deadlift/77"` and `"c900/3"`. Nothing here touches the network or the database.

use std::collections::HashMap;
use std::sync::OnceLock;

use fitparser::profile::field_types::ExerciseCategory;

/// FIT's "unknown" category (and exercise): a watch writes it when it did not recognise the exercise.
const UNKNOWN_CATEGORY: u16 = 65534;
/// The scan covers numbers `0..=MAX_NUMBER`. The profile's longest list ends near 135, so this leaves room for years of additions.
const MAX_NUMBER: u16 = 1023;

/// One exercise Garmin's profile names, or a whole category (`name` is `None`: "some deadlift", which is all a watch knows when the
/// user picked no exercise inside the category).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub category: u16,
    pub name: Option<u16>,
    /// `deadlift/romanian_deadlift`, or `deadlift` for the category alone. Unique within the catalog.
    pub key: String,
    /// `Romanian deadlift`, or `Deadlift`.
    pub english: String,
}

/// Every category the profile names (without `unknown`), each followed by the exercises it names, all in number order. Built once,
/// on first use.
pub fn all() -> &'static [CatalogEntry] {
    static CATALOG: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    CATALOG.get_or_init(build)
}

/// The stable text key of an exercise: `"deadlift/romanian_deadlift"`, `"cardio"` for a category alone. A number the profile does not
/// name stays a number: `"deadlift/77"`, `"c900/3"` (an unknown category is `c` and its number).
pub fn exercise_key(category: u16, name: Option<u16>) -> String {
    let named = ExerciseCategory::from(category);
    let category_text = category_text(named);
    match name {
        None => category_text,
        Some(number) => match profile_name(named, number) {
            Some(text) => format!("{category_text}/{text}"),
            None => format!("{category_text}/{number}"),
        },
    }
}

/// The inverse of [`exercise_key`]: the catalog's keys and the numeric forms (`"deadlift/77"`, `"c900/3"`, `"c900"`). Anything else,
/// including a number that does not fit FIT's 16 bits, is `None`.
pub fn parse_exercise_key(key: &str) -> Option<(u16, Option<u16>)> {
    if let Some(&found) = index().get(key) {
        return Some(found);
    }
    let (category, name) = match key.split_once('/') {
        Some((category, name)) => (category, Some(name)),
        None => (key, None),
    };
    let category = category_number(category)?;
    let name = match name {
        Some(name) => Some(number(name)?),
        None => None,
    };
    Some((category, name))
}

/// `"Romanian deadlift"` for `(8, Some(23))`, `"Deadlift"` for the category alone. For anything the profile does not name the key
/// itself, which is unambiguous and still readable.
pub fn english_name(category: u16, name: Option<u16>) -> String {
    let named = ExerciseCategory::from(category);
    let text = match (named, name) {
        (ExerciseCategory::Value(_) | ExerciseCategory::Unknown, _) => None,
        (named, None) => Some(named.to_string()),
        (named, Some(number)) => profile_name(named, number),
    };
    text.map_or_else(|| exercise_key(category, name), |text| sentence(&text))
}

fn build() -> Vec<CatalogEntry> {
    let mut entries = Vec::new();
    for category in 0..=MAX_NUMBER {
        let named = ExerciseCategory::from(category);
        if matches!(named, ExerciseCategory::Value(_) | ExerciseCategory::Unknown) {
            continue;
        }
        entries.push(entry(category, None));
        for number in 0..=MAX_NUMBER {
            if profile_name(named, number).is_some() {
                entries.push(entry(category, Some(number)));
            }
        }
    }
    entries
}

fn entry(category: u16, name: Option<u16>) -> CatalogEntry {
    CatalogEntry { category, name, key: exercise_key(category, name), english: english_name(category, name) }
}

/// Key to `(category, name)` for the catalog's own keys, so parsing a key is a lookup rather than a scan of 51 lists.
fn index() -> &'static HashMap<&'static str, (u16, Option<u16>)> {
    static INDEX: OnceLock<HashMap<&'static str, (u16, Option<u16>)>> = OnceLock::new();
    INDEX.get_or_init(|| all().iter().map(|entry| (entry.key.as_str(), (entry.category, entry.name))).collect())
}

/// `deadlift`, or `c900` for a category the profile does not name.
fn category_text(category: ExerciseCategory) -> String {
    match category {
        ExerciseCategory::Value(number) => format!("c{number}"),
        named => named.to_string(),
    }
}

/// The number of a category written as a key's first part: a catalog category, `unknown`, or `c` and digits.
fn category_number(text: &str) -> Option<u16> {
    if let Some(&(category, None)) = index().get(text) {
        return Some(category);
    }
    if text == "unknown" {
        return Some(UNKNOWN_CATEGORY);
    }
    number(text.strip_prefix('c')?)
}

/// Digits only: `u16::from_str` would also take a leading `+`, which no key has.
fn number(text: &str) -> Option<u16> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `romanian_deadlift` to `Romanian deadlift`.
fn sentence(snake_case: &str) -> String {
    let spaced = snake_case.replace('_', " ");
    let mut chars = spaced.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Builds `profile_name`: for each category fitparser has an exercise list for, the list's name of exercise number `name`. The lists
/// are separate enums with no common trait, so one macro arm per category; a category added to the profile later needs a line here
/// (`catalog_covers_every_category_with_exercises` fails until it has one).
macro_rules! exercise_lists {
    ($($category:ident => $list:ident,)*) => {
        /// The profile's snake_case name of exercise `name` within `category`. `None` if the category has no list (`cardio_sensors`,
        /// unknown categories) or the list does not name that number: fitparser hands those over as `Value(_)`.
        fn profile_name(category: ExerciseCategory, name: u16) -> Option<String> {
            use fitparser::profile::field_types::*;
            match category {
                $(ExerciseCategory::$category => match $list::from(name) {
                    $list::Value(_) => None,
                    named => Some(named.to_string()),
                },)*
                _ => None,
            }
        }
    };
}

exercise_lists! {
    BenchPress => BenchPressExerciseName,
    CalfRaise => CalfRaiseExerciseName,
    Cardio => CardioExerciseName,
    Carry => CarryExerciseName,
    Chop => ChopExerciseName,
    Core => CoreExerciseName,
    Crunch => CrunchExerciseName,
    Curl => CurlExerciseName,
    Deadlift => DeadliftExerciseName,
    Flye => FlyeExerciseName,
    HipRaise => HipRaiseExerciseName,
    HipStability => HipStabilityExerciseName,
    HipSwing => HipSwingExerciseName,
    Hyperextension => HyperextensionExerciseName,
    LateralRaise => LateralRaiseExerciseName,
    LegCurl => LegCurlExerciseName,
    LegRaise => LegRaiseExerciseName,
    Lunge => LungeExerciseName,
    OlympicLift => OlympicLiftExerciseName,
    Plank => PlankExerciseName,
    Plyo => PlyoExerciseName,
    PullUp => PullUpExerciseName,
    PushUp => PushUpExerciseName,
    Row => RowExerciseName,
    ShoulderPress => ShoulderPressExerciseName,
    ShoulderStability => ShoulderStabilityExerciseName,
    Shrug => ShrugExerciseName,
    SitUp => SitUpExerciseName,
    Squat => SquatExerciseName,
    TotalBody => TotalBodyExerciseName,
    TricepsExtension => TricepsExtensionExerciseName,
    WarmUp => WarmUpExerciseName,
    Run => RunExerciseName,
    Bike => BikeExerciseName,
    Move => MoveExerciseName,
    Pose => PoseExerciseName,
    BandedExercises => BandedExercisesExerciseName,
    BattleRope => BattleRopeExerciseName,
    Elliptical => EllipticalExerciseName,
    FloorClimb => FloorClimbExerciseName,
    IndoorBike => IndoorBikeExerciseName,
    IndoorRow => IndoorRowExerciseName,
    Ladder => LadderExerciseName,
    Sandbag => SandbagExerciseName,
    Sled => SledExerciseName,
    SledgeHammer => SledgeHammerExerciseName,
    StairStepper => StairStepperExerciseName,
    Suspension => SuspensionExerciseName,
    Tire => TireExerciseName,
    RunIndoor => RunIndoorExerciseName,
    BikeOutdoor => BikeOutdoorExerciseName,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_keys() {
        assert_eq!(exercise_key(8, Some(23)), "deadlift/romanian_deadlift");
        assert_eq!(exercise_key(2, None), "cardio");
        assert_eq!(exercise_key(8, Some(999)), "deadlift/999");
        assert_eq!(exercise_key(900, Some(3)), "c900/3");
        assert_eq!(exercise_key(900, None), "c900");
        assert_eq!(exercise_key(65534, None), "unknown");

        assert_eq!(parse_exercise_key("deadlift/77"), Some((8, Some(77))));
        assert_eq!(parse_exercise_key("deadlift/romanian_deadlift"), Some((8, Some(23))));
        assert_eq!(parse_exercise_key("cardio"), Some((2, None)));
        assert_eq!(parse_exercise_key("c900/3"), Some((900, Some(3))));
        assert_eq!(parse_exercise_key("c900"), Some((900, None)));
        assert_eq!(parse_exercise_key("unknown"), Some((65534, None)));
        assert_eq!(parse_exercise_key("nope/x"), None);
        assert_eq!(parse_exercise_key("nope"), None);
        assert_eq!(parse_exercise_key(""), None);
        assert_eq!(parse_exercise_key("deadlift/+5"), None);
        assert_eq!(parse_exercise_key("deadlift/70000"), None);
        assert_eq!(parse_exercise_key("deadlift/romanian_deadlift/x"), None);

        assert_eq!(english_name(8, Some(23)), "Romanian deadlift");
        assert_eq!(english_name(31, None), "Warm up");
        // Unknown ones answer with their key.
        assert_eq!(english_name(8, Some(999)), "deadlift/999");
        assert_eq!(english_name(900, Some(3)), "c900/3");
        assert_eq!(english_name(65534, None), "unknown");

        let all = all();
        assert!(all.len() > 1000, "{}", all.len());
        assert!(all.iter().all(|entry| entry.category != 65534));
        let keys: HashSet<_> = all.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys.len(), all.len(), "keys are unique");
        for entry in all {
            assert_eq!(parse_exercise_key(&entry.key), Some((entry.category, entry.name)), "{}", entry.key);
            assert_eq!(exercise_key(entry.category, entry.name), entry.key);
            assert_eq!(english_name(entry.category, entry.name), entry.english, "{}", entry.key);
            assert!(!entry.english.is_empty() && !entry.english.contains('_'), "{}", entry.english);
        }
    }

    /// The dispatch macro lists every per-category enum of the profile: a category fitparser names has exercises, except the one
    /// category the profile gives no list (`cardio_sensors`).
    #[test]
    fn catalog_covers_every_category_with_exercises() {
        let with_names: HashSet<u16> = all().iter().filter(|entry| entry.name.is_some()).map(|entry| entry.category).collect();
        for entry in all().iter().filter(|entry| entry.name.is_none()) {
            assert!(with_names.contains(&entry.category) || entry.key == "cardio_sensors", "{} has no exercises", entry.key);
        }
        assert!(with_names.len() >= 50, "{}", with_names.len());
    }
}
