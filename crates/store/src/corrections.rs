//! The user's changes to an imported activity: corrections of recorded sets, sets added by hand, leaving a whole
//! activity out of the statistics, and the "seen" mark.
//!
//! Every method is one transaction, and the ones that change what counts end in `recount`, so a change and the
//! counts it causes commit or roll back together. Nothing here writes `recorded_set`: the watch's data stays as it
//! was and a correction is a separate row on top of it, which a re-parse never rebuilds (spec §7, «Правки: модель»).
//! Foreign keys are not enforced by the database, so each method checks itself that the activity and the sets it was
//! pointed at exist, and reports [`StoreError::NotFound`] before writing anything when they do not.

use chrono::{DateTime, Utc};
use gymgar_domain::{ActivityId, ExerciseId, Mass, Reps, SetRef};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::activities::{format_time, not_found, recount};
use crate::{Store, StoreError};

/// What to change in a set. `None` leaves a field as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetChange {
    pub weight: Option<Mass>,
    pub reps: Option<Reps>,
    pub exercise: Option<ExerciseId>,
    /// `Some(Some(b))` says whether the set counts, whatever the automatic rule thinks; `Some(None)` takes that back
    /// and leaves it to the rule again; `None` does not touch the flag.
    pub counted: Option<Option<bool>>,
}

impl SetChange {
    fn is_empty(&self) -> bool {
        self.weight.is_none() && self.reps.is_none() && self.exercise.is_none() && self.counted.is_none()
    }
}

fn set_not_found(id: ActivityId, index: u16) -> StoreError {
    StoreError::NotFound(format!("set {index} of activity {}", id.0))
}

fn added_not_found(id: ActivityId, added: i64) -> StoreError {
    StoreError::NotFound(format!("added set {added} of activity {}", id.0))
}

fn ensure_activity(tx: &Transaction<'_>, id: ActivityId) -> Result<(), StoreError> {
    let known = tx.query_row("SELECT 1 FROM activity WHERE id = ?1", [id.0], |_| Ok(())).optional()?;
    known.ok_or_else(|| not_found(id))
}

/// A recorded set is known while the file has it: a correction of a set the file no longer has stays in the
/// database but is not something the user can point at.
fn ensure_recorded(tx: &Transaction<'_>, id: ActivityId, index: u16) -> Result<(), StoreError> {
    let known = tx.query_row("SELECT 1 FROM recorded_set WHERE activity_id = ?1 AND message_index = ?2", params![id.0, index], |_| Ok(())).optional()?;
    known.ok_or_else(|| set_not_found(id, index))
}

/// An added set is known while it was not removed, and only in the activity it was added to.
fn ensure_added(tx: &Transaction<'_>, id: ActivityId, added: i64) -> Result<(), StoreError> {
    let known =
        tx.query_row("SELECT 1 FROM added_set WHERE id = ?1 AND activity_id = ?2 AND deleted_at IS NULL", params![added, id.0], |_| Ok(())).optional()?;
    known.ok_or_else(|| added_not_found(id, added))
}

/// The same change as columns: the value of each field (NULL for "not given") and whether the flag is to be written.
struct Columns {
    weight_g: Option<i64>,
    reps: Option<u16>,
    category: Option<u16>,
    name: Option<u16>,
    counted: Option<bool>,
    counted_given: bool,
}

impl Columns {
    fn of(change: &SetChange) -> Columns {
        Columns {
            weight_g: change.weight.map(Mass::grams),
            reps: change.reps.map(Reps::get),
            category: change.exercise.map(|exercise| exercise.category),
            name: change.exercise.and_then(|exercise| exercise.name),
            counted: change.counted.flatten(),
            counted_given: change.counted.is_some(),
        }
    }
}

fn correct_recorded(tx: &Transaction<'_>, id: ActivityId, index: u16, change: &SetChange, now: &str) -> Result<(), StoreError> {
    if change.is_empty() {
        return Ok(());
    }
    let columns = Columns::of(change);
    // A field the change does not give keeps what the row has. The exercise is one unit: its name goes with its
    // category, so choosing a category alone must not keep the name of the exercise it replaces.
    tx.execute(
        "INSERT INTO set_correction (activity_id, message_index, weight_g, reps, exercise_category, exercise_name, counted, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(activity_id, message_index) DO UPDATE SET
             weight_g = COALESCE(excluded.weight_g, set_correction.weight_g),
             reps = COALESCE(excluded.reps, set_correction.reps),
             exercise_category = COALESCE(excluded.exercise_category, set_correction.exercise_category),
             exercise_name = CASE WHEN excluded.exercise_category IS NOT NULL THEN excluded.exercise_name ELSE set_correction.exercise_name END,
             counted = CASE WHEN ?9 THEN excluded.counted ELSE set_correction.counted END,
             updated_at = excluded.updated_at",
        params![id.0, index, columns.weight_g, columns.reps, columns.category, columns.name, columns.counted, now, columns.counted_given],
    )?;
    // A row that ended up saying nothing (the flag was the only field and went back to "by the rule") is no correction.
    tx.execute(
        "DELETE FROM set_correction
         WHERE activity_id = ?1 AND message_index = ?2 AND weight_g IS NULL AND reps IS NULL AND exercise_category IS NULL AND counted IS NULL",
        params![id.0, index],
    )?;
    Ok(())
}

fn change_added(tx: &Transaction<'_>, id: ActivityId, added: i64, change: &SetChange) -> Result<(), StoreError> {
    if change.is_empty() {
        return Ok(());
    }
    let columns = Columns::of(change);
    tx.execute(
        "UPDATE added_set SET
             weight_g = COALESCE(?3, weight_g),
             reps = COALESCE(?4, reps),
             exercise_category = COALESCE(?5, exercise_category),
             exercise_name = CASE WHEN ?5 IS NOT NULL THEN ?6 ELSE exercise_name END,
             counted = CASE WHEN ?8 THEN ?7 ELSE counted END
         WHERE id = ?1 AND activity_id = ?2",
        params![added, id.0, columns.weight_g, columns.reps, columns.category, columns.name, columns.counted, columns.counted_given],
    )?;
    Ok(())
}

impl Store {
    /// Applies one change to each of the given sets: for a recorded set it is stored as a correction on top of the
    /// record, for an added set the set itself is changed. All or nothing: a set that does not exist (or an
    /// added set that was removed, or one of another activity) fails the whole call and nothing is changed.
    pub fn edit_sets(&self, id: ActivityId, sets: &[SetRef], change: &SetChange, now: DateTime<Utc>) -> Result<(), StoreError> {
        let now = format_time(now)?;
        let tx = self.write_tx()?;
        ensure_activity(&tx, id)?;
        for set in sets {
            match *set {
                SetRef::Recorded(index) => {
                    ensure_recorded(&tx, id, index)?;
                    correct_recorded(&tx, id, index, change, &now)?;
                }
                SetRef::Added(added) => {
                    ensure_added(&tx, id, added)?;
                    change_added(&tx, id, added, change)?;
                }
            }
        }
        recount(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// "As the watch recorded it": drops the corrections of these recorded sets. A set with no correction is fine
    /// (it stays as it is); a set the file does not have fails the whole call.
    pub fn reset_sets(&self, id: ActivityId, indexes: &[u16]) -> Result<(), StoreError> {
        let tx = self.write_tx()?;
        ensure_activity(&tx, id)?;
        for &index in indexes {
            ensure_recorded(&tx, id, index)?;
            tx.execute("DELETE FROM set_correction WHERE activity_id = ?1 AND message_index = ?2", params![id.0, index])?;
        }
        recount(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Adds a set the watch did not record, placed after `after` (at the end when `None`). Returns its id. It counts
    /// from the start: the user made it, so the empty-set rule does not apply to it.
    pub fn add_set(
        &self,
        id: ActivityId,
        after: Option<SetRef>,
        exercise: ExerciseId,
        weight: Option<Mass>,
        reps: Option<Reps>,
        now: DateTime<Utc>,
    ) -> Result<i64, StoreError> {
        let now = format_time(now)?;
        let tx = self.write_tx()?;
        ensure_activity(&tx, id)?;
        let (after_kind, after_value) = match after {
            None => (None, None),
            Some(SetRef::Recorded(index)) => {
                ensure_recorded(&tx, id, index)?;
                (Some("recorded"), Some(i64::from(index)))
            }
            // A set that was removed cannot be pointed at any more, so nothing new is placed after it.
            Some(SetRef::Added(added)) => {
                ensure_added(&tx, id, added)?;
                (Some("added"), Some(added))
            }
        };
        tx.execute(
            "INSERT INTO added_set (activity_id, after_kind, after_value, exercise_category, exercise_name, weight_g, reps, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id.0, after_kind, after_value, exercise.category, exercise.name, weight.map(Mass::grams), reps.map(Reps::get), now],
        )?;
        let added = tx.last_insert_rowid();
        recount(&tx, id)?;
        tx.commit()?;
        Ok(added)
    }

    /// Removes an added set: it is marked (`deleted_at`) rather than deleted, so it drops out of the activity and its
    /// counts but is not lost. The sets that followed it (a live added set placed after it) follow what it followed
    /// instead, in the same transaction, so no set the activity shows is anchored to one it no longer shows. Removing one
    /// that is already removed changes nothing: not the time it was removed at, and no set is re-anchored.
    pub fn remove_added_set(&self, id: ActivityId, added: i64, now: DateTime<Utc>) -> Result<(), StoreError> {
        let now = format_time(now)?;
        let tx = self.write_tx()?;
        ensure_activity(&tx, id)?;
        let target: Option<(Option<String>, Option<i64>, Option<String>)> = tx
            .query_row("SELECT after_kind, after_value, deleted_at FROM added_set WHERE id = ?1 AND activity_id = ?2", params![added, id.0], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        let Some((after_kind, after_value, deleted_at)) = target else {
            return Err(added_not_found(id, added));
        };
        if deleted_at.is_some() {
            return Ok(());
        }
        // The kind and the value are taken over together (both NULL when the removed set was first), so the table's
        // check that they are both set or both empty keeps holding.
        tx.execute(
            "UPDATE added_set SET after_kind = ?3, after_value = ?4
             WHERE activity_id = ?2 AND deleted_at IS NULL AND after_kind = 'added' AND after_value = ?1",
            params![added, id.0, after_kind, after_value],
        )?;
        tx.execute("UPDATE added_set SET deleted_at = ?3 WHERE id = ?1 AND activity_id = ?2", params![added, id.0, now])?;
        recount(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Leaves a whole activity out of the statistics (or takes it back in). It stays in the list.
    pub fn set_activity_excluded(&self, id: ActivityId, excluded: bool) -> Result<(), StoreError> {
        let tx = self.write_tx()?;
        let changed = tx.execute("UPDATE activity SET excluded = ?2 WHERE id = ?1", params![id.0, excluded])?;
        if changed == 0 {
            return Err(not_found(id));
        }
        recount(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Takes the "new" mark off an activity. The first time stays: seeing it again changes nothing. Nothing is
    /// recounted, because the mark does not change what counts.
    pub fn mark_seen(&self, id: ActivityId, now: DateTime<Utc>) -> Result<(), StoreError> {
        let now = format_time(now)?;
        let tx = self.write_tx()?;
        let changed = tx.execute("UPDATE activity SET seen_at = COALESCE(seen_at, ?2) WHERE id = ?1", params![id.0, now])?;
        if changed == 0 {
            return Err(not_found(id));
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "corrections_tests.rs"]
mod tests;
