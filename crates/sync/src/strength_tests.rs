use chrono::{DateTime, Duration, TimeZone, Utc};
use gymgar_domain::{ExerciseId, Mass, PlannedTarget, Reps, is_empty_set};
use gymgar_garmin::fit::testing::{FitWriter, SetSpec, StepSpec, zip_single};
use gymgar_garmin::fit::{self, FitActivity, FitSet, FitSetKind, FitStep, FitTitle};

use super::*;

const FIXTURE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/garmin/strength-plan.zip"));

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 17, 5, 9).unwrap()
}

fn at(seconds: i64) -> DateTime<Utc> {
    t0() + Duration::seconds(seconds)
}

fn kg(value: f64) -> Mass {
    Mass::from_kg(value).unwrap()
}

fn reps(n: u32) -> Reps {
    Reps::new(n).unwrap()
}

fn exercise(category: u16, name: Option<u16>) -> Option<ExerciseId> {
    Some(ExerciseId { category, name })
}

/// A synthetic active set 30 s long that starts at `seconds`, with no exercise, reps or weight; tests fill in what they probe.
fn spec(message_index: u16, seconds: i64) -> SetSpec {
    SetSpec {
        message_index,
        active: true,
        start: at(seconds),
        duration_s: 30.0,
        reps: None,
        weight_kg: None,
        category: None,
        subtype: None,
        step_index: None,
        pounds: false,
    }
}

fn rest(message_index: u16, seconds: i64, duration_s: f64) -> SetSpec {
    SetSpec { active: false, duration_s, ..spec(message_index, seconds) }
}

/// The synthetic file as the Personal API would send it.
fn zip(writer: FitWriter) -> Vec<u8> {
    zip_single("1_ACTIVITY.fit", &writer.finish())
}

fn parse(writer: FitWriter) -> StrengthRecord {
    parse_strength(&zip(writer)).unwrap()
}

fn set_at(record: &StrengthRecord, message_index: u16) -> &gymgar_domain::RecordedSet {
    record.sets.iter().find(|set| set.message_index == message_index).unwrap_or_else(|| panic!("no set {message_index}"))
}

#[test]
fn strength_fixture() {
    let record = parse_strength(FIXTURE).unwrap();

    assert_eq!(record.workout_name.as_deref(), Some("FB2"));
    // Active sets only (the file has rests between them), in the file's order.
    assert_eq!(record.sets.len(), 31);
    assert!(record.sets.windows(2).all(|pair| pair[0].message_index < pair[1].message_index));

    // The first Romanian deadlift: 10 reps at 25 kg, where the plan said 10 at 70 kg; 114.123 s of rest follow.
    let deadlift = set_at(&record, 4);
    assert_eq!(deadlift.reps, Some(reps(10)));
    assert_eq!(deadlift.weight, Some(kg(25.0)));
    assert_eq!(deadlift.exercise, exercise(8, Some(23)));
    assert_eq!(deadlift.step_index, Some(4));
    assert_eq!(deadlift.plan, Some(PlannedTarget { reps: Some(reps(10)), weight: Some(kg(70.0)) }));
    let rest_ms = i64::from(deadlift.rest_after_ms.unwrap());
    assert!((rest_ms - 114_123).abs() <= 1, "{rest_ms}");

    // A warm-up is a category without an exercise in it, and has no reps.
    let warm_up = set_at(&record, 2);
    assert_eq!(warm_up.exercise, exercise(31, None));
    assert_eq!(warm_up.reps, None);

    assert!(record.titles.contains(&(ExerciseId { category: 8, name: Some(23) }, "Румынская становая тяга".to_owned())));
}

/// The heart rate of every set, checked against the file's own records rather than against the mapping.
#[test]
fn strength_fixture_heart_rate() {
    let record = parse_strength(FIXTURE).unwrap();
    let file = fit::parse(&fit::unzip_activity(FIXTURE).unwrap()).unwrap();

    let mut with_heart_rate = 0;
    for set in &record.sets {
        let source = file.sets.iter().find(|source| source.message_index == set.message_index).unwrap();
        let start = source.start_time.unwrap();
        let end = start + Duration::milliseconds((source.duration_s.unwrap() * 1000.0).round() as i64);
        let inside: Vec<u32> = file.heart_rate.iter().filter(|(at, _)| *at >= start && *at < end).map(|(_, bpm)| u32::from(*bpm)).collect();

        assert_eq!(set.start, start);
        assert_eq!(set.hr_avg.is_some(), !inside.is_empty(), "set {}", set.message_index);
        assert_eq!(set.hr_max.is_some(), !inside.is_empty(), "set {}", set.message_index);
        if !inside.is_empty() {
            with_heart_rate += 1;
            let mean = f64::from(inside.iter().sum::<u32>()) / inside.len() as f64;
            assert_eq!(set.hr_avg, Some(mean.round() as u16), "set {}", set.message_index);
            assert_eq!(set.hr_max, inside.iter().max().map(|&bpm| bpm as u16), "set {}", set.message_index);
        }
    }
    // The real workout has a pulse reading every couple of seconds, so every set has some.
    assert_eq!(with_heart_rate, 31);
}

#[test]
fn strength_synthetic_no_exercise_and_empty() {
    let mut writer = FitWriter::new(t0());
    // No category at all, and FIT's "unknown" category: both are "no exercise".
    writer.set(SetSpec { reps: Some(10), weight_kg: Some(20.0), ..spec(0, 0) });
    writer.set(SetSpec { reps: Some(8), category: Some(65534), ..spec(1, 60) });
    // The watch's artifact: two seconds, nothing done.
    writer.set(SetSpec { duration_s: 2.0, ..spec(2, 120) });
    // A named exercise, for contrast.
    writer.set(SetSpec { reps: Some(5), weight_kg: Some(100.0), category: Some(0), subtype: Some(1), ..spec(3, 180) });

    let record = parse(writer);

    assert_eq!(record.sets.len(), 4);
    assert_eq!(set_at(&record, 0).exercise, None);
    assert_eq!(set_at(&record, 1).exercise, None);
    assert_eq!(set_at(&record, 3).exercise, exercise(0, Some(1)));
    let empty = set_at(&record, 2);
    assert_eq!((empty.reps, empty.weight, empty.duration_ms), (None, None, 2_000));
    assert!(is_empty_set(empty.reps, empty.weight, empty.duration_ms));
    assert!(!is_empty_set(set_at(&record, 0).reps, set_at(&record, 0).weight, set_at(&record, 0).duration_ms));
}

#[test]
fn strength_no_sets() {
    // A manual activity or another app's recording: a valid file without sets is a record without sets, not an error.
    let mut writer = FitWriter::new(t0());
    writer.workout("Evening");
    writer.heart_rate(at(0), 90);

    let record = parse(writer);

    assert!(record.sets.is_empty());
    assert!(record.titles.is_empty());
    assert_eq!(record.workout_name.as_deref(), Some("Evening"));
}

#[test]
fn strength_garbage() {
    assert!(matches!(parse_strength(b"not a zip at all"), Err(SyncError::Fit(_))));
    assert!(matches!(parse_strength(b""), Err(SyncError::Fit(_))));
    // A ZIP whose `.fit` is not FIT, and one that holds no `.fit`.
    assert!(matches!(parse_strength(&zip_single("1_ACTIVITY.fit", b"this is not a FIT file")), Err(SyncError::Fit(_))));
    assert!(matches!(parse_strength(&zip_single("notes.txt", b"hello")), Err(SyncError::Fit(_))));
    // A FIT file cut short.
    let mut writer = FitWriter::new(t0());
    writer.set(spec(0, 0));
    let mut bytes = writer.finish();
    bytes.truncate(bytes.len() - 5);
    assert!(matches!(parse_strength(&zip_single("1_ACTIVITY.fit", &bytes)), Err(SyncError::Fit(_))));
}

#[test]
fn strength_sets_come_in_message_index_order_without_rests_or_duplicates() {
    let mut writer = FitWriter::new(t0());
    // Written out of order, with rests between and one index twice: the first of the two stays.
    writer.set(SetSpec { reps: Some(3), ..spec(4, 400) });
    writer.set(rest(1, 40, 30.0));
    writer.set(SetSpec { reps: Some(1), ..spec(0, 0) });
    writer.set(SetSpec { reps: Some(2), ..spec(2, 200) });
    writer.set(SetSpec { reps: Some(9), ..spec(2, 210) });
    writer.set(rest(3, 300, 30.0));

    let record = parse(writer);

    let indexes: Vec<u16> = record.sets.iter().map(|set| set.message_index).collect();
    assert_eq!(indexes, [0, 2, 4]);
    assert_eq!(record.sets.iter().map(|set| set.reps).collect::<Vec<_>>(), [Some(reps(1)), Some(reps(2)), Some(reps(3))]);
}

#[test]
fn strength_plan_and_rest() {
    let mut writer = FitWriter::new(t0());
    writer.step(StepSpec { message_index: 0, reps: Some(8), weight_kg: Some(80.5), category: Some(0), name: Some(1), notes: None });
    // Planned reps no one could do; the weight is still a plan.
    writer.step(StepSpec { message_index: 1, reps: Some(5_000), weight_kg: Some(50.0), ..StepSpec::default() });
    // A step with nothing to plan.
    writer.step(StepSpec { message_index: 2, category: Some(31), ..StepSpec::default() });
    writer.set(SetSpec { step_index: Some(0), ..spec(0, 0) });
    writer.set(rest(1, 30, 45.5));
    // A step the file does not have.
    writer.set(SetSpec { step_index: Some(9), ..spec(2, 80) });
    writer.set(SetSpec { step_index: Some(1), ..spec(3, 120) });
    writer.set(SetSpec { step_index: Some(2), ..spec(4, 160) });
    // No rest after it: an active set follows.
    writer.set(spec(5, 200));
    writer.set(rest(7, 240, 20.0));

    let record = parse(writer);

    let first = set_at(&record, 0);
    assert_eq!(first.step_index, Some(0));
    assert_eq!(first.plan, Some(PlannedTarget { reps: Some(reps(8)), weight: Some(kg(80.5)) }));
    assert_eq!(first.rest_after_ms, Some(45_500));
    assert_eq!((set_at(&record, 2).step_index, set_at(&record, 2).plan), (Some(9), None));
    assert_eq!(set_at(&record, 3).plan, Some(PlannedTarget { reps: None, weight: Some(kg(50.0)) }));
    assert_eq!((set_at(&record, 4).step_index, set_at(&record, 4).plan), (Some(2), None));
    // No set after it at all, a set that is not a rest after it, and a rest that is not the next message.
    assert_eq!(set_at(&record, 2).rest_after_ms, None);
    assert_eq!(set_at(&record, 5).rest_after_ms, None);
    assert_eq!(set_at(&record, 5).plan, None);
}

#[test]
fn strength_heart_rate_window() {
    let mut writer = FitWriter::new(t0());
    // Set 0 covers [0 s, 30 s): the reading at 0 s is inside, the one at 30 s is not.
    writer.set(spec(0, 0));
    // Set 1 covers [100 s, 130 s): two readings, 100.5 rounds up.
    writer.set(spec(1, 100));
    // Set 2 has no reading at all.
    writer.set(spec(2, 200));
    // Readings are not in time order in this file.
    writer.heart_rate(at(30), 200);
    writer.heart_rate(at(10), 110);
    writer.heart_rate(at(-1), 40);
    writer.heart_rate(at(29), 121);
    writer.heart_rate(at(0), 100);
    writer.heart_rate(at(101), 101);
    writer.heart_rate(at(100), 100);

    let record = parse(writer);

    let first = set_at(&record, 0);
    // (100 + 110 + 121) / 3 = 110.33
    assert_eq!((first.hr_avg, first.hr_max), (Some(110), Some(121)));
    let second = set_at(&record, 1);
    assert_eq!((second.hr_avg, second.hr_max), (Some(101), Some(101)));
    let third = set_at(&record, 2);
    assert_eq!((third.hr_avg, third.hr_max), (None, None));
}

#[test]
fn strength_titles() {
    let mut writer = FitWriter::new(t0());
    writer.title(Some(8), Some(23), "Румынская становая тяга");
    writer.title(Some(31), None, "Разминка");
    // No category, FIT's unknown category and no text are not titles of an exercise.
    writer.title(None, Some(1), "Orphan");
    writer.title(Some(65534), None, "Unknown");
    writer.title(Some(8), Some(24), "");
    writer.title(Some(8), Some(25), "  ");

    let record = parse(writer);

    assert_eq!(
        record.titles,
        [(ExerciseId { category: 8, name: Some(23) }, "Румынская становая тяга".to_owned()), (ExerciseId { category: 31, name: None }, "Разминка".to_owned())]
    );
}

#[test]
fn strength_weight_and_reps_outside_the_domain_are_absent() {
    let mut writer = FitWriter::new(t0());
    writer.set(SetSpec { reps: Some(1_001), weight_kg: Some(1_500.0), category: Some(0), subtype: Some(1), ..spec(0, 0) });
    writer.set(SetSpec { reps: Some(1_000), weight_kg: Some(1_000.0), ..spec(1, 60) });

    let record = parse(writer);

    // The set itself stays, and so does everything else about it.
    let odd = set_at(&record, 0);
    assert_eq!((odd.reps, odd.weight), (None, None));
    assert_eq!(odd.exercise, exercise(0, Some(1)));
    assert_eq!(odd.duration_ms, 30_000);
    // The limits themselves are values.
    let edge = set_at(&record, 1);
    assert_eq!((edge.reps, edge.weight), (Some(reps(1_000)), Some(kg(1_000.0))));
}

fn fit_set(message_index: u16) -> FitSet {
    FitSet {
        message_index,
        kind: FitSetKind::Active,
        start_time: Some(t0()),
        duration_s: Some(30.0),
        repetitions: None,
        weight_kg: None,
        category: None,
        category_subtype: None,
        wkt_step_index: None,
    }
}

#[test]
fn strength_odd_values_in_the_decoded_file() {
    let activity = FitActivity {
        sets: vec![
            // No start time: there is nowhere to place it, so it is left out.
            FitSet { start_time: None, ..fit_set(0) },
            // Missing, negative, NaN and absurd lengths are no length.
            FitSet { duration_s: None, ..fit_set(1) },
            FitSet { duration_s: Some(-3.0), ..fit_set(2) },
            FitSet { duration_s: Some(f64::NAN), ..fit_set(3) },
            FitSet { duration_s: Some(1e30), ..fit_set(4) },
            // Weights the domain cannot hold.
            FitSet { weight_kg: Some(-1.0), ..fit_set(5) },
            FitSet { weight_kg: Some(f64::NAN), ..fit_set(6) },
            FitSet { weight_kg: Some(f64::INFINITY), ..fit_set(7) },
            // The last index there is: nothing can follow it.
            fit_set(u16::MAX),
            // A rest that says how long it was, and one that does not.
            FitSet { kind: FitSetKind::Rest, ..fit_set(9) },
            FitSet { kind: FitSetKind::Rest, duration_s: None, ..fit_set(11) },
            // A set of a kind the profile does not name is neither work nor rest.
            FitSet { kind: FitSetKind::Other, ..fit_set(12) },
        ],
        ..FitActivity::default()
    };

    let record = from_fit(activity);

    let indexes: Vec<u16> = record.sets.iter().map(|set| set.message_index).collect();
    assert_eq!(indexes, [1, 2, 3, 4, 5, 6, 7, u16::MAX]);
    let durations: Vec<u32> = record.sets.iter().map(|set| set.duration_ms).collect();
    assert_eq!(durations, [0, 0, 0, 0, 30_000, 30_000, 30_000, 30_000]);
    assert!(record.sets.iter().all(|set| set.weight.is_none() && set.rest_after_ms.is_none()));
}

#[test]
fn strength_rest_with_an_odd_length_is_no_rest() {
    let activity = FitActivity {
        sets: vec![
            fit_set(0),
            FitSet { kind: FitSetKind::Rest, duration_s: Some(60.0), ..fit_set(1) },
            fit_set(2),
            FitSet { kind: FitSetKind::Rest, duration_s: Some(-60.0), ..fit_set(3) },
            fit_set(4),
            FitSet { kind: FitSetKind::Rest, duration_s: None, ..fit_set(5) },
        ],
        ..FitActivity::default()
    };

    let record = from_fit(activity);

    let rests: Vec<Option<u32>> = record.sets.iter().map(|set| set.rest_after_ms).collect();
    assert_eq!(rests, [Some(60_000), None, None]);
}

#[test]
fn strength_heart_rate_of_a_set_that_runs_past_the_end_of_time() {
    // Neither an overflowing window nor an empty one is a panic.
    let last = DateTime::<Utc>::MAX_UTC;
    let activity = FitActivity {
        sets: vec![FitSet { start_time: Some(last), duration_s: Some(4_000_000.0), ..fit_set(0) }, FitSet { duration_s: Some(0.0), ..fit_set(1) }],
        heart_rate: vec![(last, 100), (t0(), 90)],
        ..FitActivity::default()
    };

    let record = from_fit(activity);

    assert_eq!(record.sets.len(), 2);
    assert_eq!(set_at(&record, 0).hr_avg, None);
    // A set with no duration covers no instant, not even its own start.
    assert_eq!((set_at(&record, 1).hr_avg, set_at(&record, 1).hr_max), (None, None));
}

#[test]
fn strength_steps_and_titles_of_the_decoded_file() {
    let activity = FitActivity {
        sets: vec![FitSet { wkt_step_index: Some(3), ..fit_set(0) }],
        // Two steps with the same index: the first is the plan.
        steps: vec![
            FitStep { message_index: 3, duration_reps: Some(12), exercise_weight_kg: None, category: None, name: None, notes: None },
            FitStep { message_index: 3, duration_reps: Some(99), exercise_weight_kg: Some(10.0), category: None, name: None, notes: None },
        ],
        titles: vec![FitTitle { category: Some(8), name: None, title: "  Deadlift  ".to_owned() }],
        workout_name: Some("FB2".to_owned()),
        heart_rate: Vec::new(),
    };

    let record = from_fit(activity);

    assert_eq!(set_at(&record, 0).plan, Some(PlannedTarget { reps: Some(reps(12)), weight: None }));
    // The text is kept without the padding around it.
    assert_eq!(record.titles, [(ExerciseId { category: 8, name: None }, "Deadlift".to_owned())]);
    assert_eq!(record.workout_name.as_deref(), Some("FB2"));
}

#[test]
fn parser_version_starts_at_one() {
    assert_eq!(PARSER_VERSION, 1);
}
