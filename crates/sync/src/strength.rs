//! A strength activity file in domain terms.
//!
//! [`parse_strength`] opens the ZIP, decodes the `.FIT` and maps it: the active sets with their plan, the rest after each and the
//! heart rate during it. The file is hostile input, so a value the domain refuses (a weight over a tonne, a thousand-and-one reps)
//! becomes "absent" with a log line instead of failing the file: one odd set must not cost the user the whole workout. Log lines
//! name a set by its index only; titles and notes are the user's text.
//!
//! The recorded values are what the watch wrote. Nothing here knows about the user's corrections: those are stored apart and never
//! touched by parsing, which is why a file can be parsed again whenever [`PARSER_VERSION`] changes.

use std::collections::HashMap;

use chrono::{DateTime, TimeDelta, Utc};
use gymgar_domain::{ExerciseId, Mass, PlannedTarget, RecordedSet, Reps, StrengthRecord};
use gymgar_garmin::fit::{self, FitActivity, FitSet, FitSetKind, FitStep};

use crate::SyncError;

/// Bumped whenever this mapping changes what it reads from a file, so the engine knows which stored files to parse again.
pub const PARSER_VERSION: u32 = 1;

/// Heart-rate readings: when, and beats per minute.
type HeartRate = [(DateTime<Utc>, u8)];

/// Reads the ZIP the Personal API sends for an activity (one `.FIT` inside).
///
/// A file without sets is a record without sets. A ZIP or a `.FIT` that cannot be read is [`SyncError::Fit`].
pub fn parse_strength(zip: &[u8]) -> Result<StrengthRecord, SyncError> {
    let file = fit::unzip_activity(zip)?;
    Ok(from_fit(fit::parse(&file)?))
}

/// The mapping of a decoded file, apart from opening it so that tests can hand it values no real file holds.
///
/// Only active sets are kept, in `message_index` order; the index is the set's key for good, so two sets with the same index (which
/// no watch writes) keep the first. A set the watch did not date is left out: there is nowhere to place it.
fn from_fit(activity: FitActivity) -> StrengthRecord {
    let FitActivity { workout_name, sets, titles, steps, mut heart_rate } = activity;
    // Readings come in time order from a watch, but the window lookup depends on it and the file does not promise it.
    heart_rate.sort_by_key(|(at, _)| *at);
    let plans = plans(&steps);
    let rests = rests(&sets);

    let mut active: Vec<&FitSet> = sets.iter().filter(|set| set.kind == FitSetKind::Active).collect();
    active.sort_by_key(|set| set.message_index);
    let all = active.len();
    active.dedup_by_key(|set| set.message_index);
    if active.len() < all {
        log::warn!("the activity file has {} sets with a message index already seen, ignored", all - active.len());
    }

    StrengthRecord {
        workout_name,
        sets: active.into_iter().filter_map(|set| recorded(set, &plans, &rests, &heart_rate)).collect(),
        titles: titles
            .into_iter()
            .filter_map(|title| {
                let id = exercise(title.category, title.name)?;
                let text = title.title.trim();
                (!text.is_empty()).then(|| (id, text.to_owned()))
            })
            .collect(),
    }
}

fn recorded(set: &FitSet, plans: &HashMap<u16, Option<PlannedTarget>>, rests: &HashMap<u16, u32>, heart_rate: &HeartRate) -> Option<RecordedSet> {
    let message_index = set.message_index;
    let Some(start) = set.start_time else {
        log::warn!("set {message_index} of the activity file has no start time, ignored");
        return None;
    };
    // A missing or broken length counts as none: the set has no time on it.
    let duration_ms = millis(set.duration_s).unwrap_or(0);
    let (hr_avg, hr_max) = heart_rate_during(heart_rate, start, duration_ms);
    Some(RecordedSet {
        message_index,
        start,
        duration_ms,
        reps: reps(set),
        weight: weight(set),
        exercise: exercise(set.category, set.category_subtype),
        step_index: set.wkt_step_index,
        plan: set.wkt_step_index.and_then(|step| plans.get(&step).copied().flatten()),
        hr_avg,
        hr_max,
        // The rest is the next message; the last index has no next.
        rest_after_ms: message_index.checked_add(1).and_then(|next| rests.get(&next).copied()),
    })
}

/// What each step of the workout planned, by the step's `message_index`. A step with two entries keeps the first.
fn plans(steps: &[FitStep]) -> HashMap<u16, Option<PlannedTarget>> {
    let mut plans = HashMap::new();
    for step in steps {
        plans.entry(step.message_index).or_insert_with(|| plan(step));
    }
    plans
}

/// `None` when the step plans neither reps nor weight the domain accepts.
fn plan(step: &FitStep) -> Option<PlannedTarget> {
    let reps = step
        .duration_reps
        .and_then(|n| Reps::new(n).inspect_err(|_| log::warn!("step {}: the planned reps are out of range, ignored", step.message_index)).ok());
    let weight = step
        .exercise_weight_kg
        .and_then(|kg| Mass::from_kg(kg).inspect_err(|_| log::warn!("step {}: the planned weight is out of range, ignored", step.message_index)).ok());
    (reps.is_some() || weight.is_some()).then_some(PlannedTarget { reps, weight })
}

/// How long each rest lasted, by the rest's `message_index`. A rest without a usable length is left out.
fn rests(sets: &[FitSet]) -> HashMap<u16, u32> {
    let mut rests = HashMap::new();
    for set in sets.iter().filter(|set| set.kind == FitSetKind::Rest) {
        if let Some(ms) = millis(set.duration_s) {
            rests.entry(set.message_index).or_insert(ms);
        }
    }
    rests
}

fn reps(set: &FitSet) -> Option<Reps> {
    let count = set.repetitions?;
    Reps::new(u32::from(count)).inspect_err(|_| log::warn!("set {}: the rep count is out of range, ignored", set.message_index)).ok()
}

fn weight(set: &FitSet) -> Option<Mass> {
    let kg = set.weight_kg?;
    Mass::from_kg(kg).inspect_err(|_| log::warn!("set {}: the weight is out of range, ignored", set.message_index)).ok()
}

/// No category, or FIT's "unknown", is no exercise. A category alone is a real one (`warm_up`, `cardio`).
fn exercise(category: Option<u16>, name: Option<u16>) -> Option<ExerciseId> {
    category.filter(|&category| category != ExerciseId::UNKNOWN_CATEGORY).map(|category| ExerciseId { category, name })
}

/// Seconds as whole milliseconds; `None` for a missing, negative, NaN or beyond-`u32` value.
fn millis(seconds: Option<f64>) -> Option<u32> {
    let ms = (seconds? * 1000.0).round();
    // NaN is in no range. In range, so the cast cannot truncate.
    (0.0..=f64::from(u32::MAX)).contains(&ms).then_some(ms as u32)
}

/// The mean (rounded half up) and the maximum of the readings with `start <= t < start + duration`; `None` for both when there is none.
///
/// `heart_rate` is sorted by time, so the window is found by search and only the readings in it are visited: a set costs a
/// binary search plus its own window. A watch's sets do not overlap, so all sets together visit each reading about once. A
/// crafted file whose sets all span the whole activity makes every set visit every reading, which the parser's limits (2 000
/// sets, 200 000 readings) keep finite.
fn heart_rate_during(heart_rate: &HeartRate, start: DateTime<Utc>, duration_ms: u32) -> (Option<u16>, Option<u16>) {
    let Some(end) = TimeDelta::try_milliseconds(i64::from(duration_ms)).and_then(|length| start.checked_add_signed(length)) else {
        return (None, None);
    };
    let first = heart_rate.partition_point(|(at, _)| *at < start);
    let window = heart_rate.get(first..).unwrap_or_default().iter().take_while(|(at, _)| *at < end);
    let (count, sum, max) =
        window.fold((0_u64, 0_u64, 0_u8), |(count, sum, max), &(_, bpm)| (count.saturating_add(1), sum.saturating_add(u64::from(bpm)), max.max(bpm)));
    // Adding half the count before dividing rounds half up. With no readings there is nothing to divide by.
    let avg = sum.saturating_add(count / 2).checked_div(count).and_then(|mean| u16::try_from(mean).ok());
    (avg, (count > 0).then_some(u16::from(max)))
}

#[cfg(test)]
#[path = "strength_tests.rs"]
mod tests;
