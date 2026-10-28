//! Reading imported activities back: the list with its filters, one activity in full, the number of new ones, exercise
//! titles, how often each exercise was done and the counted sets one by one.
//!
//! Nothing here writes. The list never reads sets: it uses the counts stored with each activity (`counted_sets`,
//! `exercise_count`, kept by `recount`), so a long history stays cheap. Only the `exercise` filter, `exercise_usage`
//! and `strength_history` look at sets, and they apply the same rule as `recount` (the SQL below mirrors it, in one place):
//! the correction over the record, the user's flag over the empty-set rule, nothing in an excluded activity, no
//! removed added sets, and "no exercise" and Garmin's unknown category are not exercises. A test keeps them in step.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDateTime, Utc};
use gymgar_domain::{
    ActivityId, ActivitySummary, AddedSet, EMPTY_SET_MAX_MS, ExerciseId, FileState, Mass, PlannedTarget, RecordedSet, Reps, STRENGTH_TYPE_KEY, SetCorrection,
    SetRef,
};
use rusqlite::types::Value;
use rusqlite::{Row, params, params_from_iter};

use crate::activities::{corrupt, format_time, parse_time, read_exercise, read_mass, read_reps};
use crate::{Store, StoreError};

/// Which activities to list. Every condition given must hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityFilter {
    /// Started at or after this moment.
    pub from: Option<DateTime<Utc>>,
    /// Started before this moment.
    pub to: Option<DateTime<Utc>>,
    /// Garmin's type keys; empty means any type.
    pub type_keys: Vec<String>,
    pub strength_only: bool,
    /// Has a counted set of this exercise (its final one: the correction over the record). An excluded activity counts
    /// nothing, so it never has one.
    pub exercise: Option<ExerciseId>,
    pub only_new: bool,
    pub include_excluded: bool,
    pub limit: u32,
    pub offset: u32,
}

/// One line of the activity list.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityRow {
    pub summary: ActivitySummary,
    /// Not yet opened by the user.
    pub is_new: bool,
    pub excluded: bool,
    pub file_state: FileState,
    pub workout_name: Option<String>,
    pub counted_sets: u32,
    pub exercise_count: u32,
}

/// An activity in full: what the watch recorded, what the user changed on top, and what the user added.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityDetail {
    pub row: ActivityRow,
    /// In the order of the file.
    pub recorded: Vec<RecordedSet>,
    /// By the message index of the recorded set they correct. A correction of a set the file no longer has is kept in
    /// the database but is not here.
    pub corrections: BTreeMap<u16, SetCorrection>,
    /// In the order they were made; removed ones are not here.
    pub added: Vec<AddedSet>,
}

/// One counted set with its final values (the correction over the record), for the progress of an exercise.
#[derive(Debug, Clone, PartialEq)]
pub struct HistorySet {
    pub activity: ActivityId,
    /// When the activity began, in UTC.
    pub start: DateTime<Utc>,
    /// The wall-clock time of the start where it was recorded, as Garmin gives it (`startTimeLocal`). Garmin writes it
    /// without a zone, so it is a plain date and time, and it is the day the user would call the training day. `None`
    /// when the activity has none or it does not read.
    pub local_start: Option<NaiveDateTime>,
    pub workout_name: Option<String>,
    pub set: SetRef,
    pub exercise: ExerciseId,
    pub weight: Option<Mass>,
    pub reps: Option<Reps>,
    /// What the workout plan asked for. Only a set from the record has a plan; an added set has none.
    pub plan_reps: Option<Reps>,
}

const ACTIVITY_COLUMNS: &str = "a.id, a.type_key, a.name, a.start_at, a.duration_ms, a.distance_m, a.avg_hr, a.calories, a.manual,
     a.seen_at, a.excluded, a.file_state, a.workout_name, a.counted_sets, a.exercise_count";

// What counts, in SQL. This is `recount` over again, for the places that cannot read the stored counts: keep the two
// in step (`the_stored_counts_the_filter_and_the_usage_agree` fails if they drift). `r` is a recorded set, `c` its
// correction, `s` an added set, `a` the activity.
const RECORDED_FROM: &str = "recorded_set r LEFT JOIN set_correction c ON c.activity_id = r.activity_id AND c.message_index = r.message_index";
const RECORDED_CATEGORY: &str = "COALESCE(c.exercise_category, r.exercise_category)";
// A correction names the exercise by its category; the name goes with it.
const RECORDED_NAME: &str = "CASE WHEN c.exercise_category IS NOT NULL THEN c.exercise_name ELSE r.exercise_name END";
const ADDED_COUNTS: &str = "s.deleted_at IS NULL AND COALESCE(s.counted, 1) = 1";
// Garmin's wall-clock start of an activity, as the raw JSON of the list item has it. `json_extract` fails the whole
// query on malformed JSON and returns a number as a number, so only text from valid JSON is let through; whether the
// text is a date and time is for the reader to say.
const LOCAL_START: &str = "CASE WHEN json_valid(a.raw_json) THEN json_extract(a.raw_json, '$.startTimeLocal') END";

/// The user's flag if there is one, else "not empty" by the effective reps and weight (`domain::is_empty_set`).
fn recorded_counts() -> String {
    format!(
        "CASE WHEN c.counted IS NOT NULL THEN c.counted = 1
              ELSE NOT ((COALESCE(c.reps, r.reps) IS NULL OR COALESCE(c.reps, r.reps) = 0)
                    AND (COALESCE(c.weight_g, r.weight_g) IS NULL OR COALESCE(c.weight_g, r.weight_g) = 0)
                    AND r.duration_ms < {EMPTY_SET_MAX_MS}) END"
    )
}

/// Binds a value and returns its placeholder, so that one value can be used in several places of the query.
fn bind(params: &mut Vec<Value>, value: Value) -> String {
    params.push(value);
    format!("?{}", params.len())
}

fn integer(value: u16) -> Value {
    Value::Integer(i64::from(value))
}

/// The list query and its parameters. Separate from `list_activities` so that a test can ask SQLite for its plan.
fn list_query(f: &ActivityFilter) -> Result<(String, Vec<Value>), StoreError> {
    let mut params: Vec<Value> = Vec::new();
    let mut conditions: Vec<String> = Vec::new();
    // Bounds are compared as text, which is why they are written with the same fixed-width form as `start_at`.
    if let Some(from) = f.from {
        conditions.push(format!("a.start_at >= {}", bind(&mut params, Value::Text(format_time(from)?))));
    }
    if let Some(to) = f.to {
        conditions.push(format!("a.start_at < {}", bind(&mut params, Value::Text(format_time(to)?))));
    }
    if !f.type_keys.is_empty() {
        let placeholders: Vec<String> = f.type_keys.iter().map(|key| bind(&mut params, Value::Text(key.clone()))).collect();
        conditions.push(format!("a.type_key IN ({})", placeholders.join(", ")));
    }
    if f.strength_only {
        conditions.push(format!("a.type_key = {}", bind(&mut params, Value::Text(STRENGTH_TYPE_KEY.to_owned()))));
    }
    if f.only_new {
        conditions.push("a.seen_at IS NULL".to_owned());
    }
    if !f.include_excluded {
        conditions.push("a.excluded = 0".to_owned());
    }
    if let Some(exercise) = f.exercise {
        let category = bind(&mut params, integer(exercise.category));
        let name = bind(&mut params, exercise.name.map_or(Value::Null, integer));
        let counts = recorded_counts();
        conditions.push(format!(
            "(a.excluded = 0 AND {category} <> {unknown}
              AND (EXISTS (SELECT 1 FROM {RECORDED_FROM} WHERE r.activity_id = a.id AND {counts} AND {RECORDED_CATEGORY} = {category} AND {RECORDED_NAME} IS {name})
                OR EXISTS (SELECT 1 FROM added_set s WHERE s.activity_id = a.id AND {ADDED_COUNTS} AND s.exercise_category = {category} AND s.exercise_name IS {name})))",
            unknown = ExerciseId::UNKNOWN_CATEGORY,
        ));
    }
    let filter = if conditions.is_empty() { String::new() } else { format!("WHERE {}", conditions.join(" AND ")) };
    let limit = bind(&mut params, Value::Integer(i64::from(f.limit)));
    let offset = bind(&mut params, Value::Integer(i64::from(f.offset)));
    // Ties on the start (two activities begun in the same second) fall back to the id, so the order is fixed.
    let sql = format!("SELECT {ACTIVITY_COLUMNS} FROM activity a {filter} ORDER BY a.start_at DESC, a.id DESC LIMIT {limit} OFFSET {offset}");
    Ok((sql, params))
}

fn read_activity_row(row: &Row<'_>) -> Result<ActivityRow, StoreError> {
    let start: String = row.get(3)?;
    let duration_ms: Option<i64> = row.get(4)?;
    let seen_at: Option<String> = row.get(9)?;
    let file_state: String = row.get(11)?;
    Ok(ActivityRow {
        summary: ActivitySummary {
            id: ActivityId(row.get(0)?),
            type_key: row.get(1)?,
            name: row.get(2)?,
            start: parse_time("start_at", &start)?,
            duration_ms: duration_ms.map(|ms| u64::try_from(ms).map_err(|_| corrupt("duration_ms", ms))).transpose()?,
            distance_m: row.get(5)?,
            avg_hr: row.get(6)?,
            calories: row.get(7)?,
            manual: row.get(8)?,
        },
        is_new: seen_at.is_none(),
        excluded: row.get(10)?,
        file_state: FileState::parse(&file_state).ok_or_else(|| corrupt("file_state", &file_state))?,
        workout_name: row.get(12)?,
        counted_sets: row.get(13)?,
        exercise_count: row.get(14)?,
    })
}

fn read_recorded_set(row: &Row<'_>) -> Result<RecordedSet, StoreError> {
    let start: String = row.get(1)?;
    let has_plan: bool = row.get(8)?;
    // A plan that asked for nothing in particular is still a plan; `has_plan` tells it from no plan at all.
    let plan = if has_plan { Some(PlannedTarget { reps: read_reps(row.get(9)?)?, weight: read_mass(row.get(10)?)? }) } else { None };
    Ok(RecordedSet {
        message_index: row.get(0)?,
        start: parse_time("recorded_set.start_at", &start)?,
        duration_ms: row.get(2)?,
        reps: read_reps(row.get(3)?)?,
        weight: read_mass(row.get(4)?)?,
        exercise: read_exercise((row.get(5)?, row.get(6)?))?,
        step_index: row.get(7)?,
        plan,
        hr_avg: row.get(11)?,
        hr_max: row.get(12)?,
        rest_after_ms: row.get(13)?,
    })
}

fn read_correction(row: &Row<'_>) -> Result<(u16, SetCorrection), StoreError> {
    let correction = SetCorrection {
        weight: read_mass(row.get(1)?)?,
        reps: read_reps(row.get(2)?)?,
        exercise: read_exercise((row.get(3)?, row.get(4)?))?,
        counted: row.get(5)?,
    };
    Ok((row.get(0)?, correction))
}

fn read_added_set(row: &Row<'_>) -> Result<AddedSet, StoreError> {
    let kind: Option<String> = row.get(1)?;
    let value: Option<i64> = row.get(2)?;
    let after = match (kind.as_deref(), value) {
        (None, None) => None,
        (Some("recorded"), Some(index)) => Some(SetRef::Recorded(u16::try_from(index).map_err(|_| corrupt("added_set.after_value", index))?)),
        (Some("added"), Some(added)) => Some(SetRef::Added(added)),
        _ => return Err(corrupt("added_set.after_kind", (&kind, value))),
    };
    let category: i64 = row.get(3)?;
    let exercise = read_exercise((Some(category), row.get(4)?))?.ok_or_else(|| corrupt("added_set.exercise_category", category))?;
    Ok(AddedSet { id: row.get(0)?, after, exercise, weight: read_mass(row.get(5)?)?, reps: read_reps(row.get(6)?)?, counted: row.get(7)? })
}

/// The kind of a row of the history query: a set from the record, or one the user added.
const HISTORY_RECORDED: i64 = 0;
const HISTORY_ADDED: i64 = 1;

/// The local start is whatever the JSON held there: anything but text of the form `2026-11-01 00:30:00` is no time.
fn read_local_start(value: Value) -> Option<NaiveDateTime> {
    match value {
        Value::Text(text) => NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S").ok(),
        _ => None,
    }
}

fn read_history_set(row: &Row<'_>) -> Result<HistorySet, StoreError> {
    let start: String = row.get(1)?;
    let kind: i64 = row.get(4)?;
    let index: i64 = row.get(5)?;
    let set = match kind {
        HISTORY_RECORDED => SetRef::Recorded(u16::try_from(index).map_err(|_| corrupt("recorded_set.message_index", index))?),
        HISTORY_ADDED => SetRef::Added(index),
        other => return Err(corrupt("history kind", other)),
    };
    let category: Option<i64> = row.get(6)?;
    let exercise = read_exercise((category, row.get(7)?))?.ok_or_else(|| corrupt("exercise_category", category))?;
    Ok(HistorySet {
        activity: ActivityId(row.get(0)?),
        start: parse_time("start_at", &start)?,
        local_start: read_local_start(row.get(2)?),
        workout_name: row.get(3)?,
        set,
        exercise,
        weight: read_mass(row.get(8)?)?,
        reps: read_reps(row.get(9)?)?,
        plan_reps: read_reps(row.get(10)?)?,
    })
}

/// Runs a query and turns each row with `read`; unlike `query_map`, `read` may fail with a `StoreError`.
fn collect<T>(
    statement: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
    read: fn(&Row<'_>) -> Result<T, StoreError>,
) -> Result<Vec<T>, StoreError> {
    let mut rows = statement.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(read(row)?);
    }
    Ok(out)
}

impl Store {
    /// The activities that pass the filter, newest first. Reads the counts stored with each activity, not its sets.
    pub fn list_activities(&self, f: &ActivityFilter) -> Result<Vec<ActivityRow>, StoreError> {
        let (sql, params) = list_query(f)?;
        let mut statement = self.conn.prepare(&sql)?;
        collect(&mut statement, params_from_iter(params), read_activity_row)
    }

    /// One activity in full, or `None` when there is no such activity. Read in one transaction, so that the pieces
    /// describe the same moment even if another process writes meanwhile.
    pub fn activity_detail(&self, id: ActivityId) -> Result<Option<ActivityDetail>, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let rows = collect(&mut tx.prepare(&format!("SELECT {ACTIVITY_COLUMNS} FROM activity a WHERE a.id = ?1"))?, [id.0], read_activity_row)?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let recorded = collect(
            &mut tx.prepare(
                "SELECT message_index, start_at, duration_ms, reps, weight_g, exercise_category, exercise_name, step_index, has_plan, plan_reps,
                        plan_weight_g, hr_avg, hr_max, rest_after_ms
                 FROM recorded_set WHERE activity_id = ?1 ORDER BY message_index",
            )?,
            [id.0],
            read_recorded_set,
        )?;
        // Only corrections of sets the file has; the others are kept in the database, out of sight.
        let corrections = collect(
            &mut tx.prepare(
                "SELECT c.message_index, c.weight_g, c.reps, c.exercise_category, c.exercise_name, c.counted
                 FROM set_correction c
                 JOIN recorded_set r ON r.activity_id = c.activity_id AND r.message_index = c.message_index
                 WHERE c.activity_id = ?1 ORDER BY c.message_index",
            )?,
            [id.0],
            read_correction,
        )?;
        let added = collect(
            &mut tx.prepare(
                "SELECT id, after_kind, after_value, exercise_category, exercise_name, weight_g, reps, counted
                 FROM added_set WHERE activity_id = ?1 AND deleted_at IS NULL ORDER BY id",
            )?,
            [id.0],
            read_added_set,
        )?;
        tx.commit()?;
        Ok(Some(ActivityDetail { row, recorded, corrections: corrections.into_iter().collect(), added }))
    }

    /// How many activities the user has not opened yet. Excluded ones count too: the list shows them on request, and
    /// the number on the "new" chip has to match what the chip lists.
    pub fn new_count(&self) -> Result<u32, StoreError> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM activity WHERE seen_at IS NULL", [], |row| row.get(0))?)
    }

    /// The title of each exercise the files have named, in the language of the watch.
    pub fn exercise_titles(&self) -> Result<HashMap<ExerciseId, String>, StoreError> {
        let mut statement = self.conn.prepare("SELECT category, name, title FROM exercise_title")?;
        let mut rows = statement.query([])?;
        let mut titles = HashMap::new();
        while let Some(row) = rows.next()? {
            let category: i64 = row.get(0)?;
            let name: i64 = row.get(1)?;
            let category = u16::try_from(category).map_err(|_| corrupt("exercise_title.category", category))?;
            // An exercise without a number is stored as -1 because NULL would make the key non-unique.
            let name = if name == -1 { None } else { Some(u16::try_from(name).map_err(|_| corrupt("exercise_title.name", name))?) };
            titles.insert(ExerciseId { category, name }, row.get(2)?);
        }
        Ok(titles)
    }

    /// How many sets count for each exercise, by the final exercise of the set. "No exercise" and the unknown
    /// category are not exercises and are not here.
    pub fn exercise_usage(&self) -> Result<HashMap<ExerciseId, u32>, StoreError> {
        let counts = recorded_counts();
        let sql = format!(
            "SELECT category, name, COUNT(*) FROM (
                 SELECT {RECORDED_CATEGORY} AS category, {RECORDED_NAME} AS name
                 FROM {RECORDED_FROM} JOIN activity a ON a.id = r.activity_id
                 WHERE a.excluded = 0 AND {counts}
                 UNION ALL
                 SELECT s.exercise_category, s.exercise_name
                 FROM added_set s JOIN activity a ON a.id = s.activity_id
                 WHERE a.excluded = 0 AND {ADDED_COUNTS}
             ) WHERE category IS NOT NULL AND category <> {unknown} GROUP BY category, name",
            unknown = ExerciseId::UNKNOWN_CATEGORY,
        );
        let mut statement = self.conn.prepare(&sql)?;
        let mut rows = statement.query(params![])?;
        let mut usage = HashMap::new();
        while let Some(row) = rows.next()? {
            let exercise = read_exercise((row.get(0)?, row.get(1)?))?;
            if let Some(exercise) = exercise {
                usage.insert(exercise, row.get(2)?);
            }
        }
        Ok(usage)
    }

    /// Every counted set of every activity that is not excluded, with its final values, oldest activity first: by the
    /// start, then (so that two activities begun in the same second do not mix) by the id, then the sets of the record
    /// in the order of the file, then the added ones in the order they were made. The same sets `exercise_usage`
    /// counts: "no exercise" and the unknown category are not here. It is for progress, so it reads the whole history
    /// in one query and leaves the grouping to the caller.
    pub fn strength_history(&self) -> Result<Vec<HistorySet>, StoreError> {
        let counts = recorded_counts();
        let sql = format!(
            "SELECT activity_id, start_at, local_start, workout_name, kind, idx, category, name, weight_g, reps, plan_reps FROM (
                 SELECT a.id AS activity_id, a.start_at AS start_at, {LOCAL_START} AS local_start, a.workout_name AS workout_name,
                        {HISTORY_RECORDED} AS kind, r.message_index AS idx, {RECORDED_CATEGORY} AS category, {RECORDED_NAME} AS name,
                        COALESCE(c.weight_g, r.weight_g) AS weight_g, COALESCE(c.reps, r.reps) AS reps, r.plan_reps AS plan_reps
                 FROM {RECORDED_FROM} JOIN activity a ON a.id = r.activity_id
                 WHERE a.excluded = 0 AND {counts}
                 UNION ALL
                 SELECT a.id, a.start_at, {LOCAL_START}, a.workout_name,
                        {HISTORY_ADDED}, s.id, s.exercise_category, s.exercise_name,
                        s.weight_g, s.reps, NULL
                 FROM added_set s JOIN activity a ON a.id = s.activity_id
                 WHERE a.excluded = 0 AND {ADDED_COUNTS}
             ) WHERE category IS NOT NULL AND category <> {unknown}
             ORDER BY start_at, activity_id, kind, idx",
            unknown = ExerciseId::UNKNOWN_CATEGORY,
        );
        let mut statement = self.conn.prepare(&sql)?;
        collect(&mut statement, params![], read_history_set)
    }
}

#[cfg(test)]
#[path = "activity_queries_tests.rs"]
mod tests;
