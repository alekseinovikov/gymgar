//! The writes of the activity import: activities from Garmin's list, their strength files, exercise titles and the
//! sync bookkeeping, plus the recount of what counts in an activity.
//!
//! Every public method is one transaction. Corrections (`set_correction`) and added sets (`added_set`) are the
//! user's: nothing here writes or deletes them, so a re-parse can rebuild `recorded_set` without losing a change.
//! Changing them is `corrections.rs`, reading them back is `activity_queries.rs`; this module decides what is stored and
//! owns the shared pieces of both (the time format, the readers of stored values, `recount`).

use std::collections::HashSet;
use std::fmt::Debug;

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use gymgar_domain::{
    ActivityId, ActivitySummary, CountReason, ExerciseId, FileState, Mass, RecordedSet, Reps, StrengthRecord, SyncErrorKind, count_reason, is_empty_set,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::{Store, StoreError};

/// One element of Garmin's activity list, ready to be stored.
#[derive(Debug, Clone, PartialEq)]
pub struct NewActivity {
    pub summary: ActivitySummary,
    /// The list element exactly as Garmin sent it, kept so that new statistics need no second download.
    pub raw_json: String,
    /// Whether the user has yet to see this activity (`seen_at` stays empty until then). Only used when the row is created.
    pub is_new: bool,
}

/// What came of fetching and reading one activity's file.
#[derive(Debug, Clone, PartialEq)]
pub enum FileOutcome {
    /// The file was read. `zip` is the file as downloaded; `None` for a re-parse, where it is already stored.
    Parsed { zip: Option<Vec<u8>>, record: StrengthRecord, parser_version: u32 },
    /// The file was downloaded but this parser version could not read it. The bytes are kept so that a newer parser can
    /// try again; the version is what tells "tried by this parser already" from "not tried by the new one yet".
    Unreadable { zip: Vec<u8>, parser_version: u32 },
    /// Garmin has no file for this activity.
    Missing,
}

/// What a sync run needs to know about the database before it starts, read in one go so that it does not look again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSnapshot {
    pub known: HashSet<ActivityId>,
    pub history_complete: bool,
    /// The start of the newest activity stored, if any.
    pub newest_start: Option<DateTime<Utc>>,
    /// Strength activities still waiting for their file, newest first.
    pub pending_files: Vec<ActivityId>,
}

/// How the last sync ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncState {
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<SyncErrorKind>,
    pub history_complete: bool,
}

pub(crate) fn corrupt(what: &str, value: impl Debug) -> StoreError {
    StoreError::Corrupt(format!("{what} holds an unusable value: {value:?}"))
}

pub(crate) fn not_found(id: ActivityId) -> StoreError {
    StoreError::NotFound(format!("activity {}", id.0))
}

/// RFC 3339 in UTC with a fixed number of digits, so that comparing two stored times as text compares the times:
/// SQLite sorts and takes `MAX` of `start_at` as text, and the variable-width form of `garmin_connection` would put
/// `…:15Z` after `…:15.5Z`.
pub(crate) fn format_time(time: DateTime<Utc>) -> Result<String, StoreError> {
    // RFC 3339 has four-digit years only; a row written with any other could never be read back.
    if !(0..=9999).contains(&time.year()) {
        return Err(StoreError::Sql(format!("the time {time} cannot be stored: its year is outside 0..=9999")));
    }
    Ok(time.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

pub(crate) fn parse_time(column: &str, value: &str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(value).map(|time| time.with_timezone(&Utc)).map_err(|_| corrupt(column, value))
}

fn to_i64(what: &str, value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Sql(format!("{what} {value} cannot be stored: it does not fit a 64-bit integer")))
}

/// Where a new activity's file stands before anything was downloaded.
fn initial_file_state(summary: &ActivitySummary) -> FileState {
    match (summary.is_strength(), summary.manual) {
        (true, false) => FileState::Pending,
        // A manual entry has no device file: asking Garmin for it would only produce a 404.
        (true, true) => FileState::Missing,
        (false, _) => FileState::NotNeeded,
    }
}

/// An exercise as far as statistics go: "no exercise" and Garmin's "unknown" category are the same thing (spec §7).
fn is_real_exercise(exercise: ExerciseId) -> bool {
    exercise.category != ExerciseId::UNKNOWN_CATEGORY
}

impl Store {
    /// Stores one page of Garmin's list. A new activity gets its bookkeeping (`first_seen_at`, `seen_at`, `file_state`);
    /// a known one only gets its summary and raw JSON refreshed, so a refresh never un-sees, un-excludes or re-queues
    /// anything. The same id twice in `items` is one row, the later element winning.
    pub fn upsert_activities(&self, items: &[NewActivity], now: DateTime<Utc>) -> Result<(), StoreError> {
        let now = format_time(now)?;
        let tx = self.write_tx()?;
        {
            let mut upsert = tx.prepare_cached(
                "INSERT INTO activity (id, type_key, name, start_at, duration_ms, distance_m, avg_hr, calories, manual, raw_json, first_seen_at, seen_at, file_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(id) DO UPDATE SET
                     type_key = excluded.type_key,
                     name = excluded.name,
                     start_at = excluded.start_at,
                     duration_ms = excluded.duration_ms,
                     distance_m = excluded.distance_m,
                     avg_hr = excluded.avg_hr,
                     calories = excluded.calories,
                     manual = excluded.manual,
                     raw_json = excluded.raw_json",
            )?;
            for item in items {
                let summary = &item.summary;
                upsert.execute(params![
                    summary.id.0,
                    summary.type_key,
                    summary.name,
                    format_time(summary.start)?,
                    summary.duration_ms.map(|ms| to_i64("the duration", ms)).transpose()?,
                    // SQLite turns NaN into NULL on its own, but would keep an infinity as a distance.
                    summary.distance_m.filter(|distance| distance.is_finite()),
                    summary.avg_hr,
                    summary.calories,
                    summary.manual,
                    item.raw_json,
                    now,
                    (!item.is_new).then_some(&now),
                    initial_file_state(summary).as_str(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// What a sync run starts from.
    pub fn sync_snapshot(&self) -> Result<SyncSnapshot, StoreError> {
        // One read transaction: the pieces describe the same moment even if another process writes meanwhile.
        let tx = self.conn.unchecked_transaction()?;
        let known = collect_ids(&tx, "SELECT id FROM activity")?.into_iter().collect();
        // Ties on the start (two activities begun in the same second) fall back to the id, so the order is fixed.
        let pending_files = collect_ids(&tx, "SELECT id FROM activity WHERE file_state = 'pending' ORDER BY start_at DESC, id DESC")?;
        let newest_start: Option<String> = tx.query_row("SELECT MAX(start_at) FROM activity", [], |row| row.get(0))?;
        let newest_start = newest_start.map(|text| parse_time("start_at", &text)).transpose()?;
        let history_complete = read_sync_state(&tx)?.history_complete;
        tx.commit()?;
        Ok(SyncSnapshot { known, history_complete, newest_start, pending_files })
    }

    /// Records what came of one activity's file. A parsed file replaces the activity's recorded sets as a whole and
    /// recounts; corrections and added sets are not touched. `now` is when the exercise titles of the file were seen.
    pub fn apply_file(&self, id: ActivityId, outcome: FileOutcome, now: DateTime<Utc>) -> Result<(), StoreError> {
        let now = format_time(now)?;
        let tx = self.write_tx()?;
        let known = tx.query_row("SELECT 1 FROM activity WHERE id = ?1", [id.0], |_| Ok(())).optional()?;
        if known.is_none() {
            return Err(not_found(id));
        }
        match outcome {
            FileOutcome::Parsed { zip, record, parser_version } => {
                if let Some(zip) = zip {
                    store_zip(&tx, id, &zip)?;
                }
                tx.execute(
                    "UPDATE activity SET workout_name = ?2, parser_version = ?3, file_state = ?4 WHERE id = ?1",
                    params![id.0, record.workout_name, parser_version, FileState::Downloaded.as_str()],
                )?;
                replace_recorded_sets(&tx, id, &record.sets)?;
                upsert_titles(&tx, &record.titles, &now)?;
                recount(&tx, id)?;
            }
            FileOutcome::Unreadable { zip, parser_version } => {
                store_zip(&tx, id, &zip)?;
                tx.execute(
                    "UPDATE activity SET parser_version = ?2, file_state = ?3 WHERE id = ?1",
                    params![id.0, parser_version, FileState::Unreadable.as_str()],
                )?;
            }
            FileOutcome::Missing => {
                tx.execute("UPDATE activity SET file_state = ?2 WHERE id = ?1", params![id.0, FileState::Missing.as_str()])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The stored files that a parser older than `version` read or failed to read, newest activity first, to be parsed
    /// again without the network. A file recorded at `version` or later is not listed, readable or not. A row with
    /// no parser version never had a file, so it is not listed either.
    pub fn files_to_reparse(&self, version: u32) -> Result<Vec<(ActivityId, Vec<u8>)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT activity.id, activity_file.zip
             FROM activity JOIN activity_file ON activity_file.activity_id = activity.id
             WHERE activity.file_state IN ('downloaded', 'unreadable') AND activity.parser_version < ?1
             ORDER BY activity.start_at DESC, activity.id DESC",
        )?;
        let rows = statement.query_map([version], |row| Ok((ActivityId(row.get(0)?), row.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Whether the whole history, back to the oldest activity Garmin lists, has been loaded.
    pub fn set_history_complete(&self, done: bool) -> Result<(), StoreError> {
        let tx = self.write_tx()?;
        tx.execute(
            "INSERT INTO garmin_sync (id, history_complete) VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET history_complete = excluded.history_complete",
            [done],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Records how a sync run ended. `error` always replaces the last one (`None` clears it); `success_at` replaces the
    /// time of the last success only when there is one, so a failed run keeps the time of the last good one.
    pub fn record_sync(&self, success_at: Option<DateTime<Utc>>, error: Option<SyncErrorKind>) -> Result<(), StoreError> {
        let success_at = success_at.map(format_time).transpose()?;
        let tx = self.write_tx()?;
        tx.execute(
            "INSERT INTO garmin_sync (id, last_success_at, last_error) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                 last_success_at = COALESCE(excluded.last_success_at, garmin_sync.last_success_at),
                 last_error = excluded.last_error",
            params![success_at, error.map(|error| error.as_str())],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// How the last sync ended; all empty before the first one.
    pub fn sync_state(&self) -> Result<SyncState, StoreError> {
        read_sync_state(&self.conn)
    }
}

fn collect_ids(conn: &Connection, sql: &str) -> Result<Vec<ActivityId>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map([], |row| row.get(0).map(ActivityId))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn read_sync_state(conn: &Connection) -> Result<SyncState, StoreError> {
    let row: Option<(Option<String>, Option<String>, bool)> = conn
        .query_row("SELECT last_success_at, last_error, history_complete FROM garmin_sync WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    let Some((last_success_at, last_error, history_complete)) = row else {
        return Ok(SyncState::default());
    };
    Ok(SyncState {
        last_success_at: last_success_at.map(|text| parse_time("last_success_at", &text)).transpose()?,
        last_error: last_error.map(|text| SyncErrorKind::parse(&text).ok_or_else(|| corrupt("last_error", &text))).transpose()?,
        history_complete,
    })
}

fn store_zip(tx: &Transaction<'_>, id: ActivityId, zip: &[u8]) -> Result<(), StoreError> {
    tx.execute("INSERT INTO activity_file (activity_id, zip) VALUES (?1, ?2) ON CONFLICT(activity_id) DO UPDATE SET zip = excluded.zip", params![id.0, zip])?;
    Ok(())
}

fn replace_recorded_sets(tx: &Transaction<'_>, id: ActivityId, sets: &[RecordedSet]) -> Result<(), StoreError> {
    tx.execute("DELETE FROM recorded_set WHERE activity_id = ?1", [id.0])?;
    // OR REPLACE: the table was just emptied, so this only matters if a file repeats a message index. Then the later
    // message wins instead of the whole file being refused.
    let mut insert = tx.prepare_cached(
        "INSERT OR REPLACE INTO recorded_set (activity_id, message_index, start_at, duration_ms, reps, weight_g, exercise_category, exercise_name,
                                              step_index, has_plan, plan_reps, plan_weight_g, hr_avg, hr_max, rest_after_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )?;
    for set in sets {
        insert.execute(params![
            id.0,
            set.message_index,
            format_time(set.start)?,
            set.duration_ms,
            set.reps.map(Reps::get),
            set.weight.map(Mass::grams),
            set.exercise.map(|exercise| exercise.category),
            set.exercise.and_then(|exercise| exercise.name),
            set.step_index,
            set.plan.is_some(),
            set.plan.and_then(|plan| plan.reps).map(Reps::get),
            set.plan.and_then(|plan| plan.weight).map(Mass::grams),
            set.hr_avg,
            set.hr_max,
            set.rest_after_ms,
        ])?;
    }
    Ok(())
}

/// Titles come in the order of the file and may name an exercise twice (two program steps); the last one is kept.
fn upsert_titles(tx: &Transaction<'_>, titles: &[(ExerciseId, String)], now: &str) -> Result<(), StoreError> {
    // An exercise without a number is stored as -1 because NULL would make the key non-unique.
    let mut upsert = tx.prepare_cached(
        "INSERT INTO exercise_title (category, name, title, seen_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(category, name) DO UPDATE SET title = excluded.title, seen_at = excluded.seen_at",
    )?;
    for (exercise, title) in titles {
        upsert.execute(params![exercise.category, exercise.name.map_or(-1, i64::from), title, now])?;
    }
    Ok(())
}

/// What the user did to one recorded set, or nothing.
struct SetRow {
    duration_ms: i64,
    reps: Option<i64>,
    weight_g: Option<i64>,
    exercise: (Option<i64>, Option<i64>),
    correction: CorrectionRow,
}

#[derive(Default)]
struct CorrectionRow {
    reps: Option<i64>,
    weight_g: Option<i64>,
    exercise: (Option<i64>, Option<i64>),
    counted: Option<bool>,
}

pub(crate) fn read_reps(value: Option<i64>) -> Result<Option<Reps>, StoreError> {
    value.map(|value| u32::try_from(value).ok().and_then(|reps| Reps::new(reps).ok()).ok_or_else(|| corrupt("reps", value))).transpose()
}

pub(crate) fn read_mass(value: Option<i64>) -> Result<Option<Mass>, StoreError> {
    value.map(|value| Mass::from_grams(value).map_err(|_| corrupt("weight_g", value))).transpose()
}

pub(crate) fn read_exercise((category, name): (Option<i64>, Option<i64>)) -> Result<Option<ExerciseId>, StoreError> {
    let Some(category) = category else {
        return Ok(None);
    };
    let category = u16::try_from(category).map_err(|_| corrupt("exercise_category", category))?;
    let name = name.map(|name| u16::try_from(name).map_err(|_| corrupt("exercise_name", name))).transpose()?;
    Ok(Some(ExerciseId { category, name }))
}

/// Recomputes `counted_sets` and `exercise_count` of an activity from its rows and stores them.
///
/// A recorded set counts by the effective values (the correction over the record) through the domain's rules; an
/// added set counts unless it was removed or the user switched it off (the empty-set rule is not for sets the user
/// made); an excluded activity counts nothing. Runs inside the caller's transaction, so a mutation and its recount
/// commit or roll back together. `conn` is the transaction (a `&Transaction` coerces to it).
pub(crate) fn recount(conn: &Connection, id: ActivityId) -> Result<(), StoreError> {
    let excluded: bool = conn.query_row("SELECT excluded FROM activity WHERE id = ?1", [id.0], |row| row.get(0)).optional()?.ok_or_else(|| not_found(id))?;
    let mut counted_sets = 0_i64;
    let mut exercises: HashSet<ExerciseId> = HashSet::new();
    let mut count = |exercise: Option<ExerciseId>| {
        counted_sets = counted_sets.saturating_add(1);
        exercises.extend(exercise.filter(|&exercise| is_real_exercise(exercise)));
    };

    let mut recorded = conn.prepare_cached(
        "SELECT r.duration_ms, r.reps, r.weight_g, r.exercise_category, r.exercise_name,
                c.reps, c.weight_g, c.exercise_category, c.exercise_name, c.counted
         FROM recorded_set r
         LEFT JOIN set_correction c ON c.activity_id = r.activity_id AND c.message_index = r.message_index
         WHERE r.activity_id = ?1",
    )?;
    let rows = recorded.query_map([id.0], |row| {
        Ok(SetRow {
            duration_ms: row.get(0)?,
            reps: row.get(1)?,
            weight_g: row.get(2)?,
            exercise: (row.get(3)?, row.get(4)?),
            correction: CorrectionRow { reps: row.get(5)?, weight_g: row.get(6)?, exercise: (row.get(7)?, row.get(8)?), counted: row.get(9)? },
        })
    })?;
    for row in rows {
        let row = row?;
        let correction = row.correction;
        let reps = read_reps(correction.reps.or(row.reps))?;
        let weight = read_mass(correction.weight_g.or(row.weight_g))?;
        // A correction names the exercise by its category; without one the record's exercise stands.
        let exercise = read_exercise(if correction.exercise.0.is_some() { correction.exercise } else { row.exercise })?;
        let duration_ms = u32::try_from(row.duration_ms).map_err(|_| corrupt("duration_ms", row.duration_ms))?;
        if count_reason(excluded, correction.counted, is_empty_set(reps, weight, duration_ms)) == CountReason::Counted {
            count(exercise);
        }
    }

    let mut added = conn.prepare_cached("SELECT exercise_category, exercise_name, counted FROM added_set WHERE activity_id = ?1 AND deleted_at IS NULL")?;
    let rows = added.query_map([id.0], |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<bool>>(2)?)))?;
    for row in rows {
        let (category, name, user) = row?;
        let exercise = read_exercise((category, name))?;
        if count_reason(excluded, user, false) == CountReason::Counted {
            count(exercise);
        }
    }

    let exercise_count = i64::try_from(exercises.len()).unwrap_or(i64::MAX);
    conn.execute("UPDATE activity SET counted_sets = ?2, exercise_count = ?3 WHERE id = ?1", params![id.0, counted_sets, exercise_count])?;
    Ok(())
}

#[cfg(test)]
#[path = "activities_tests.rs"]
mod tests;
