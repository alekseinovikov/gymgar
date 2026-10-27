//! `fixtures/garmin/strength-plan.zip`: a real strength workout ("FB2", a program followed on the watch), scrubbed by
//! `cargo xtask scrub-fit`. The expected values were read from the original file with a raw fitparser dump, independently of
//! `fit::parse`, so these tests check the parser against the file rather than against itself.

use std::collections::HashSet;

use fitparser::profile::MesgNum;
use gymgar_garmin::fit::{self, FitActivity, FitSet, FitSetKind, catalog};

const FIXTURE: &[u8] = include_bytes!("../../../../fixtures/garmin/strength-plan.zip");

fn fixture_fit() -> Vec<u8> {
    fit::unzip_activity(FIXTURE).unwrap()
}

fn set(activity: &FitActivity, message_index: u16) -> &FitSet {
    activity.sets.iter().find(|set| set.message_index == message_index).unwrap_or_else(|| panic!("no set {message_index}"))
}

#[test]
fn parse_fixture() {
    let activity = fit::parse(&fixture_fit()).unwrap();

    assert_eq!(activity.workout_name.as_deref(), Some("FB2"));
    assert_eq!(activity.sets.iter().filter(|set| set.kind == FitSetKind::Active).count(), 31);

    // The first Romanian deadlift: 10 reps, 25 kg, step 4 of the plan.
    let deadlift = set(&activity, 4);
    assert_eq!(deadlift.kind, FitSetKind::Active);
    assert_eq!(deadlift.repetitions, Some(10));
    assert_eq!(deadlift.weight_kg, Some(25.0));
    assert_eq!(deadlift.category, Some(8));
    assert_eq!(deadlift.category_subtype, Some(23));
    assert_eq!(deadlift.wkt_step_index, Some(4));

    // The rest after the first warm-up: its duration is all a rest carries.
    let rest = set(&activity, 1);
    assert_eq!(rest.kind, FitSetKind::Rest);
    assert!((rest.duration_s.unwrap() - 31.765).abs() < 1e-9, "{:?}", rest.duration_s);
    assert_eq!((rest.repetitions, rest.weight_kg, rest.category), (None, None, None));

    // A warm-up has a category and no exercise inside it.
    let warm_up = set(&activity, 2);
    assert_eq!((warm_up.category, warm_up.category_subtype), (Some(31), None));

    // The watch's own name (Russian here) for that exercise.
    let title = activity.titles.iter().find(|title| (title.category, title.name) == (Some(8), Some(23))).unwrap();
    assert_eq!(title.title, "Румынская становая тяга");

    assert!(!activity.steps.is_empty() && !activity.heart_rate.is_empty());

    // The real file dates and times every set, so the scrub tool's fallbacks (the activity's start, 0 s) did not touch it.
    assert!(activity.sets.iter().all(|set| set.start_time.is_some() && set.duration_s.is_some()));
}

/// Every exercise the real workout used is one the catalog names, so the keys the app stores are readable ones.
#[test]
fn fixture_exercises_are_in_the_catalog() {
    let activity = fit::parse(&fixture_fit()).unwrap();
    assert_eq!(catalog::exercise_key(8, Some(23)), "deadlift/romanian_deadlift");
    assert_eq!(catalog::english_name(8, Some(23)), "Romanian deadlift");

    let known: HashSet<_> = catalog::all().iter().map(|entry| (entry.category, entry.name)).collect();
    let used = activity
        .sets
        .iter()
        .filter_map(|set| Some((set.category?, set.category_subtype)))
        .chain(activity.titles.iter().filter_map(|title| Some((title.category?, title.name))));
    for (category, name) in used {
        assert!(known.contains(&(category, name)), "{}", catalog::exercise_key(category, name));
    }
}

/// The fixture is public: nothing but the messages the app reads may be in it, however it was made.
#[test]
fn fixture_holds_only_what_the_app_reads() {
    let records = fitparser::from_bytes(&fixture_fit()).unwrap();
    let kept = [MesgNum::FileId, MesgNum::Workout, MesgNum::WorkoutStep, MesgNum::ExerciseTitle, MesgNum::Set, MesgNum::Record];
    for record in &records {
        assert!(kept.contains(&record.kind()), "{:?}", record.kind());
        let fields: Vec<_> = record.fields().iter().map(|field| field.name()).collect();
        match record.kind() {
            MesgNum::Record => assert!(fields.iter().all(|&name| name == "timestamp" || name == "heart_rate"), "{fields:?}"),
            MesgNum::FileId => assert_eq!(fields, ["type", "manufacturer", "time_created"]),
            _ => {}
        }
    }
    // The archive's one entry has a neutral name; the real one is `<activity id>_ACTIVITY.fit`, and entry names are stored in the clear.
    assert!(FIXTURE.windows(12).any(|window| window == b"activity.fit"));
    assert!(!FIXTURE.windows(13).any(|window| window == b"_ACTIVITY.fit"));
}
