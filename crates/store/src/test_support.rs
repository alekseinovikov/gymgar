//! Builders shared by the tests of `corrections` and `activity_queries`: activities, recorded sets and a loaded store.
//!
//! Test-only. The older `activities_tests.rs` keeps its own copies; this module is for the files written after it.

use chrono::{DateTime, TimeZone, Utc};
use gymgar_domain::{ActivityId, ActivitySummary, ExerciseId, Mass, RecordedSet, Reps, StrengthRecord};

use crate::{ActivityFilter, FileOutcome, NewActivity, Store};

pub(crate) fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

pub(crate) fn kg(value: f64) -> Mass {
    Mass::from_kg(value).unwrap()
}

pub(crate) fn reps(value: u32) -> Reps {
    Reps::new(value).unwrap()
}

pub(crate) fn activity(id: i64, type_key: &str, start: DateTime<Utc>, is_new: bool) -> NewActivity {
    let summary = ActivitySummary {
        id: ActivityId(id),
        type_key: type_key.to_owned(),
        name: Some(format!("Activity {id}")),
        start,
        duration_ms: Some(3_600_000),
        distance_m: None,
        avg_hr: Some(120),
        calories: Some(300),
        manual: false,
    };
    NewActivity { summary, raw_json: format!("{{\"activityId\":{id}}}"), is_new }
}

/// A strength activity begun at 08:00 on the given day of October 2026; already seen.
pub(crate) fn strength(id: i64, day: u32) -> NewActivity {
    activity(id, "strength_training", at(day, 8), false)
}

pub(crate) fn deadlift() -> ExerciseId {
    ExerciseId { category: 8, name: Some(23) }
}

pub(crate) fn squat() -> ExerciseId {
    ExerciseId { category: 28, name: Some(1) }
}

pub(crate) fn curl() -> ExerciseId {
    ExerciseId { category: 7, name: Some(4) }
}

pub(crate) fn unknown_exercise() -> ExerciseId {
    ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: None }
}

pub(crate) fn recorded(message_index: u16, reps_count: Option<u32>, weight_kg: Option<f64>, duration_ms: u32, exercise: Option<ExerciseId>) -> RecordedSet {
    RecordedSet {
        message_index,
        start: at(20, 8) + chrono::Duration::seconds(i64::from(message_index) * 90),
        duration_ms,
        reps: reps_count.map(reps),
        weight: weight_kg.map(kg),
        exercise,
        step_index: None,
        plan: None,
        hr_avg: None,
        hr_max: None,
        rest_after_ms: None,
    }
}

/// An ordinary working set: 8 reps with 50 kg, 40 s long.
pub(crate) fn work(message_index: u16, exercise: Option<ExerciseId>) -> RecordedSet {
    recorded(message_index, Some(8), Some(50.0), 40_000, exercise)
}

/// What the watch writes for a set nobody did: no reps, no weight, over in two seconds.
pub(crate) fn blank(message_index: u16, exercise: Option<ExerciseId>) -> RecordedSet {
    recorded(message_index, None, None, 2_000, exercise)
}

/// A filter that lets everything through, excluded activities included, with room for a large history.
pub(crate) fn everything() -> ActivityFilter {
    ActivityFilter {
        from: None,
        to: None,
        type_keys: Vec::new(),
        strength_only: false,
        exercise: None,
        only_new: false,
        include_excluded: true,
        limit: 10_000,
        offset: 0,
    }
}

/// A store holding these activities (from the list only, no files yet).
pub(crate) fn store_with(items: &[NewActivity]) -> Store {
    let store = Store::open_in_memory().unwrap();
    store.upsert_activities(items, at(25, 10)).unwrap();
    store
}

/// Gives an activity its file: these recorded sets, no workout name, no titles.
pub(crate) fn load(store: &Store, id: i64, sets: Vec<RecordedSet>) {
    let record = StrengthRecord { workout_name: Some("Legs".to_owned()), sets, titles: Vec::new() };
    store.apply_file(ActivityId(id), FileOutcome::Parsed { zip: Some(b"zip".to_vec()), record, parser_version: 1 }, at(25, 11)).unwrap();
}

pub(crate) fn count_rows(store: &Store, table: &str) -> i64 {
    store.conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap()
}
