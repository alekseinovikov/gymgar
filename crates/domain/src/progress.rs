//! The progress rule: how one exercise is going, whether to hold its weight or add to it, and the series of its chart.
//!
//! Everything here is a pure function over the sessions of a single exercise, oldest first. Reading the history
//! from storage, taking "today" from a clock and showing any of it are other layers' jobs; this module knows
//! nothing about them. All weight arithmetic is in whole grams and checked, so a hostile value can neither
//! overflow nor panic.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, Utc};

use crate::{ActivityId, Mass, Reps, SetRef};

/// A pause longer than this many whole days is a break: the weight is no longer expected to hold.
pub const BREAK_DAYS: i64 = 28;
/// An exercise whose last session is older than this many days is dormant (used by the lists, not by the rule here).
pub const DORMANT_DAYS: i64 = 182;
/// The next weight is at least this many percent above the working weight.
pub const ADD_PERCENT: i64 = 5;
/// The weight step is never below this (1 kg), in grams.
pub const STEP_MIN_G: i64 = 1_000;
/// The weight step is never above this (5 kg), in grams.
pub const STEP_MAX_G: i64 = 5_000;
/// The weight step of an exercise with fewer than two different working weights (2.5 kg), in grams.
pub const STEP_DEFAULT_G: i64 = 2_500;
/// How many months back the sparkline of a tile reaches.
pub const SPARKLINE_MONTHS: u32 = 12;

/// One counted set of the exercise, as the progress rule sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressSet {
    pub activity: ActivityId,
    pub set: SetRef,
    pub weight: Option<Mass>,
    pub reps: Option<Reps>,
    /// What the workout plan asked for in this set. A plan of 0 reps is not a plan.
    pub plan_reps: Option<Reps>,
}

/// The counted sets of one exercise on one local day, in workout order.
#[derive(Debug, Clone, PartialEq)]
pub struct ExerciseSession {
    /// The start of the first activity of the day; the distance between sessions is measured on it.
    pub start: DateTime<Utc>,
    /// The local day of the session; its month is the month of the chart.
    pub day: NaiveDate,
    pub workout_name: Option<String>,
    pub sets: Vec<ProgressSet>,
}

/// The sets that stand for a session: the ones at its top weight.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingSets {
    /// The top weight of the session; 0 where no set had a weight.
    pub weight: Mass,
    /// The fewest reps among the working sets (a set without reps counts as 0): the target is met only if every one met it.
    pub reps: u16,
    /// The reps of every working set, in workout order.
    pub all_reps: Vec<u16>,
    /// The first working set with the fewest reps.
    pub set: ProgressSet,
}

fn weight_of(set: &ProgressSet) -> Mass {
    set.weight.unwrap_or(Mass::ZERO)
}

fn reps_of(set: &ProgressSet) -> u16 {
    set.reps.map_or(0, Reps::get)
}

fn plan_reps(set: &ProgressSet) -> Option<u16> {
    set.plan_reps.map(Reps::get).filter(|&reps| reps > 0)
}

/// The most frequent value; of equally frequent ones the larger.
fn most_frequent(values: impl Iterator<Item = u16>) -> Option<u16> {
    let mut counts: BTreeMap<u16, u32> = BTreeMap::new();
    for value in values {
        let count = counts.entry(value).or_insert(0);
        *count = count.saturating_add(1);
    }
    counts.into_iter().max_by_key(|&(value, count)| (count, value)).map(|(value, _)| value)
}

/// Whole days from `from` to `to`, never negative: a session dated after "today" is no time at all.
pub fn days_between(from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    to.signed_duration_since(from).num_days().max(0)
}

/// The working weight of a session and the sets at it; `None` only for a session without sets.
pub fn working_sets(session: &ExerciseSession) -> Option<WorkingSets> {
    let weight = session.sets.iter().map(weight_of).max()?;
    let at_top: Vec<ProgressSet> = session.sets.iter().filter(|set| weight_of(set) == weight).copied().collect();
    let all_reps: Vec<u16> = at_top.iter().map(reps_of).collect();
    let reps = all_reps.iter().copied().min()?;
    let set = at_top.iter().find(|set| reps_of(set) == reps).copied()?;
    Some(WorkingSets { weight, reps, all_reps, set })
}

/// The reps the plan asks for in one session: those of the working set, else the most common among its other sets.
fn session_target(session: &ExerciseSession) -> Option<u16> {
    let working = working_sets(session)?;
    plan_reps(&working.set).or_else(|| most_frequent(session.sets.iter().filter_map(plan_reps)))
}

/// The target reps: the plan of the latest session that had one (`sessions` run from old to new).
pub fn target_reps(sessions: &[ExerciseSession]) -> Option<u16> {
    sessions.iter().rev().find_map(session_target)
}

/// The step the user already uses in this exercise: the smallest difference between its different non-zero working
/// weights, kept within 1 to 5 kg; with fewer than two such weights, 2.5 kg.
pub fn weight_step(sessions: &[ExerciseSession]) -> Mass {
    let mut weights: Vec<i64> = sessions.iter().filter_map(working_sets).map(|working| working.weight.grams()).filter(|&grams| grams > 0).collect();
    weights.sort_unstable();
    weights.dedup();
    let smallest = weights.iter().zip(weights.iter().skip(1)).filter_map(|(low, high)| high.checked_sub(*low)).filter(|&difference| difference > 0).min();
    let grams = smallest.map_or(STEP_DEFAULT_G, |difference| difference.clamp(STEP_MIN_G, STEP_MAX_G));
    // Always within `STEP_MIN_G..=STEP_MAX_G`, so it is a valid `Mass`. Were it ever not, a step of nothing is the
    // safe answer: `next_weight` returns `None` for it and no advice to add weight is given.
    Mass::from_grams(grams).unwrap_or(Mass::ZERO)
}

/// The next weight: the smallest multiple of `step` that is strictly above `weight` and at least `ADD_PERCENT` percent
/// above it. `None` where it does not fit in a `Mass` or the step is nothing.
pub fn next_weight(weight: Mass, step: Mass) -> Option<Mass> {
    let step = step.grams();
    if step <= 0 {
        return None;
    }
    let current = weight.grams();
    let percent = 100_i64.checked_add(ADD_PERCENT)?;
    // Rounded up, so that 5 % is never rounded down to less.
    let at_least = current.checked_mul(percent)?.checked_add(99)?.checked_div(100)?;
    let lowest = at_least.max(current.checked_add(1)?);
    let multiples = lowest.checked_add(step.checked_sub(1)?)?.checked_div(step)?;
    Mass::from_grams(multiples.checked_mul(step)?).ok()
}

/// A break that came before the latest session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfterBreak {
    /// Whole days between the session before the break and the latest one.
    pub days: i64,
    /// Index (in the sessions) of the session before the break.
    pub before: usize,
}

/// What to do with the weight of the exercise. Indices point into the `sessions` slice given to `advise`.
#[derive(Debug, Clone, PartialEq)]
pub enum Advice {
    /// The target was met in two sessions in a row at the same weight: go up to `to`.
    Add { weight: Mass, to: Mass, target: u16, previous: usize, last: usize },
    /// Keep the weight. `met_once` means the latest session met the target and one more is needed.
    Hold { weight: Mass, reps: u16, target: u16, sessions_at_weight: u32, met_once: bool, after_break: Option<AfterBreak> },
    /// The latest session is more than `BREAK_DAYS` ago.
    Break { days: i64, last: usize },
    /// The working sets of the latest session have no weight: there is no weight to advise on.
    Bodyweight { last: usize },
    /// No session had a plan with reps.
    NoTarget { last: usize },
}

/// The session before the latest one, with the distance to it.
struct Previous {
    index: usize,
    gap_days: i64,
    working: Option<WorkingSets>,
}

/// How many sessions in a row, back from `last`, were at `weight` with no break between them (at least 1).
fn sessions_at_weight(sessions: &[ExerciseSession], last: usize, weight: Mass) -> u32 {
    let mut count = 1_u32;
    let mut index = last;
    while let Some(earlier) = index.checked_sub(1)
        && let (Some(newer_session), Some(older_session)) = (sessions.get(index), sessions.get(earlier))
        && days_between(older_session.start, newer_session.start) <= BREAK_DAYS
        && working_sets(older_session).is_some_and(|working| working.weight == weight)
    {
        count = count.saturating_add(1);
        index = earlier;
    }
    count
}

/// The advice for an exercise, from its sessions (old to new) and "now"; `None` when there is nothing to advise on
/// (no sessions, or a latest session without sets). Checked in this order: a break now, no weight, no target,
/// two sessions to add on, otherwise hold.
pub fn advise(sessions: &[ExerciseSession], today: DateTime<Utc>) -> Option<Advice> {
    let last = sessions.len().checked_sub(1)?;
    let newest = sessions.get(last)?;
    let working = working_sets(newest)?;

    let days_since = days_between(newest.start, today);
    if days_since > BREAK_DAYS {
        return Some(Advice::Break { days: days_since, last });
    }
    if working.weight.is_zero() {
        return Some(Advice::Bodyweight { last });
    }
    let Some(target) = target_reps(sessions) else {
        return Some(Advice::NoTarget { last });
    };

    let previous = last.checked_sub(1).and_then(|index| {
        let session = sessions.get(index)?;
        Some(Previous { index, gap_days: days_between(session.start, newest.start), working: working_sets(session) })
    });

    if let Some(previous) = &previous
        && previous.gap_days <= BREAK_DAYS
        && let Some(before) = &previous.working
        && before.weight == working.weight
        && before.reps >= target
        && working.reps >= target
        && let Some(to) = next_weight(working.weight, weight_step(sessions))
    {
        return Some(Advice::Add { weight: working.weight, to, target, previous: previous.index, last });
    }

    let after_break =
        previous.as_ref().filter(|previous| previous.gap_days > BREAK_DAYS).map(|previous| AfterBreak { days: previous.gap_days, before: previous.index });
    Some(Advice::Hold {
        weight: working.weight,
        reps: working.reps,
        target,
        sessions_at_weight: sessions_at_weight(sessions, last, working.weight),
        met_once: working.reps >= target,
        after_break,
    })
}

/// The reps the chart is drawn for: the target; without one, the most common reps of the working sets across the
/// history (of equally common ones the larger). Sets without reps do not vote; with no reps at all the answer is 0.
pub fn chart_reps(sessions: &[ExerciseSession]) -> u16 {
    if let Some(target) = target_reps(sessions) {
        return target;
    }
    let reps = sessions.iter().filter_map(working_sets).flat_map(|working| working.all_reps).filter(|&reps| reps > 0);
    most_frequent(reps).unwrap_or(0)
}

/// What a chart shows on its vertical axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesMode {
    /// The heaviest weight lifted for at least `reps` reps in the month.
    Weight { reps: u16 },
    /// An exercise that never had a weight: the best reps of the month.
    Reps,
}

/// One point of the chart, or one heavier-set circle: a set that stands for its month.
#[derive(Debug, Clone, PartialEq)]
pub struct MonthPoint {
    /// The first day of the month (local).
    pub month: NaiveDate,
    pub weight: Mass,
    pub reps: u16,
    /// Index (in the sessions) of the session the set came from.
    pub session: usize,
    pub set: ProgressSet,
}

/// A pause between two sessions longer than `BREAK_DAYS`; the line of the chart runs dashed across it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// Index of the session before the pause.
    pub from: usize,
    /// Index of the session after it; `None` when the pause is still going on (it leads up to today).
    pub to: Option<usize>,
    pub days: i64,
}

/// The chart of one exercise: a point per month, circles for months where only heavier weights were lifted, and the breaks.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub mode: SeriesMode,
    pub points: Vec<MonthPoint>,
    pub heavier: Vec<MonthPoint>,
    pub gaps: Vec<Gap>,
}

/// The best sets of a month so far: the heaviest one with enough reps, and the heaviest one without.
#[derive(Default)]
struct MonthBest {
    top: Option<MonthPoint>,
    below: Option<MonthPoint>,
}

/// Keeps the better of the held and the offered set: heavier, and at equal weight more reps; the earlier one at a tie.
fn offer(slot: &mut Option<MonthPoint>, candidate: MonthPoint) {
    if slot.as_ref().is_none_or(|best| (candidate.weight, candidate.reps) > (best.weight, best.reps)) {
        *slot = Some(candidate);
    }
}

fn month_start(day: NaiveDate) -> Option<NaiveDate> {
    day.with_day(1)
}

/// The best sets of every month in weight mode: only sets with a weight take part; any set counts, not only the working one.
fn weight_months(sessions: &[ExerciseSession], reps_needed: u16) -> BTreeMap<NaiveDate, MonthBest> {
    let mut months: BTreeMap<NaiveDate, MonthBest> = BTreeMap::new();
    for (index, session) in sessions.iter().enumerate() {
        let Some(month) = month_start(session.day) else { continue };
        for set in &session.sets {
            let Some(weight) = set.weight.filter(|weight| !weight.is_zero()) else { continue };
            let reps = reps_of(set);
            let best = months.entry(month).or_default();
            let slot = if reps >= reps_needed { &mut best.top } else { &mut best.below };
            offer(slot, MonthPoint { month, weight, reps, session: index, set: *set });
        }
    }
    months
}

/// The best session of every month in reps mode: the one whose weakest set did the most reps.
fn reps_months(sessions: &[ExerciseSession]) -> BTreeMap<NaiveDate, MonthBest> {
    let mut months: BTreeMap<NaiveDate, MonthBest> = BTreeMap::new();
    for (index, session) in sessions.iter().enumerate() {
        let (Some(month), Some(working)) = (month_start(session.day), working_sets(session)) else { continue };
        let best = months.entry(month).or_default();
        offer(&mut best.top, MonthPoint { month, weight: working.weight, reps: working.reps, session: index, set: working.set });
    }
    months
}

/// Pauses longer than `BREAK_DAYS` between neighbouring sessions, and the one still going on up to `today`.
fn gaps(sessions: &[ExerciseSession], today: DateTime<Utc>) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for (index, pair) in sessions.windows(2).enumerate() {
        let [older, newer] = pair else { continue };
        let days = days_between(older.start, newer.start);
        if days > BREAK_DAYS {
            gaps.push(Gap { from: index, to: index.checked_add(1), days });
        }
    }
    if let Some(last) = sessions.len().checked_sub(1)
        && let Some(newest) = sessions.get(last)
    {
        let days = days_between(newest.start, today);
        if days > BREAK_DAYS {
            gaps.push(Gap { from: last, to: None, days });
        }
    }
    gaps
}

/// The chart series of an exercise from its sessions (old to new) and "now".
///
/// In weight mode a month's point is its heaviest set with reps of at least `chart_reps`, and its circle the heaviest
/// set with fewer reps if that is heavier than the point (or there is no point). Sets without a weight take no part.
/// The mode is reps only when every session's working weight is 0.
pub fn series(sessions: &[ExerciseSession], today: DateTime<Utc>) -> Series {
    let without_weight = !sessions.is_empty() && sessions.iter().all(|session| working_sets(session).is_none_or(|working| working.weight.is_zero()));
    let (mode, months) = if without_weight {
        (SeriesMode::Reps, reps_months(sessions))
    } else {
        let reps = chart_reps(sessions);
        (SeriesMode::Weight { reps }, weight_months(sessions, reps))
    };

    let mut points = Vec::new();
    let mut heavier = Vec::new();
    // Months come in calendar order, so both lists do too.
    for MonthBest { top, below } in months.into_values() {
        if let Some(below) = below
            && top.as_ref().is_none_or(|top| below.weight > top.weight)
        {
            heavier.push(below);
        }
        if let Some(top) = top {
            points.push(top);
        }
    }
    Series { mode, points, heavier, gaps: gaps(sessions, today) }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
