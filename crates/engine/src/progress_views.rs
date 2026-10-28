//! The views of exercise progress: what `progress.list` and `progress.get` answer.
//!
//! Plain serializable data, read back by the UI as well. They carry everything a tile or a chart needs (the advice with
//! the sessions it rests on, the working weight, the points of the chart with the set each one came from), so neither the
//! UI nor an agent has to compute a thing.
//!
//! This module collects the strength history into the sessions of each exercise and turns the answers of the rules in
//! `gymgar_domain::progress` into views. It decides no rule itself (the advice, the chart and the step are the domain's)
//! and reads and writes nothing: the history comes in as an argument, and so does "today".

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use gymgar_domain::progress::{self, Advice, DORMANT_DAYS, ExerciseSession, Gap, MonthPoint, ProgressSet, SPARKLINE_MONTHS, Series, SeriesMode, WorkingSets};
use gymgar_domain::{ActivityId, ExerciseId, Mass, Reps};
use gymgar_store::HistorySet;
use gymgar_sync::exercises::is_strength;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::activity_views::{ExerciseView, exercise_view};

/// What a tile advises. Also the filter of `progress.list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AdviceKind {
    /// The target was met in two sessions in a row at the same weight: add weight.
    Add,
    /// Keep the weight.
    Hold,
    /// The exercise has not been done for more than four weeks.
    Break,
    /// The working sets have no weight: nothing to advise on.
    Bodyweight,
    /// No workout plan asked for any reps, so there is nothing to compare the sets with.
    NoTarget,
}

/// The working sets of one session: the sets at its top weight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingView {
    /// The top weight of the session; 0 where no set had a weight.
    pub weight_kg: f64,
    /// The reps of every set at that weight, in workout order.
    pub reps: Vec<u16>,
    /// The day of the session.
    pub date: NaiveDate,
    /// The activity of the working set that did the fewest reps, to open it.
    pub activity_id: ActivityId,
}

/// The advice of a tile with what it rests on. A field that does not belong to the kind is `None`, empty or false.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdviceView {
    pub kind: AdviceKind,
    /// The reps the workout plan asks for (`add` and `hold`).
    pub target: Option<u16>,
    /// The weight to go up to (`add`).
    pub next_weight_kg: Option<f64>,
    /// How many sessions in a row, the latest included, were at the working weight (`hold`).
    pub sessions_at_weight: Option<u32>,
    /// The latest session met the target, and one more is needed (`hold`).
    pub met_once: bool,
    /// The two sessions the advice to add rests on: the previous one, then the latest (`add`).
    pub basis: Vec<WorkingView>,
    /// The length of the pause: the break going on (`break`), or the one that came before the latest session (`hold`).
    pub break_days: Option<i64>,
    /// The session before the pause (`hold` after a break).
    pub before_break: Option<WorkingView>,
}

impl AdviceView {
    /// An advice that says nothing but its kind.
    fn bare(kind: AdviceKind) -> AdviceView {
        AdviceView {
            kind,
            target: None,
            next_weight_kg: None,
            sessions_at_weight: None,
            met_once: false,
            basis: Vec::new(),
            break_days: None,
            before_break: None,
        }
    }
}

/// One point of a chart (or one circle of heavier sets): the set that stands for its month.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointView {
    /// The month as `2026-08`.
    pub month: String,
    /// 0 in a chart of reps, where `reps` is what the chart shows.
    pub weight_kg: f64,
    pub reps: u16,
    /// The day of the session the set came from.
    pub date: NaiveDate,
    pub activity_id: ActivityId,
}

/// One exercise as a tile of the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressItemView {
    pub exercise: ExerciseView,
    pub advice: AdviceView,
    /// The working sets of the latest session.
    pub last: WorkingView,
    /// How many days the exercise was done on.
    pub sessions: u32,
    pub first_date: NaiveDate,
    /// The latest session is more than half a year old.
    pub dormant: bool,
    /// The reps the chart is drawn for; `None` when the chart shows reps (the exercise never had a weight).
    pub chart_reps: Option<u16>,
    /// The points of the last twelve months, old to new.
    pub sparkline: Vec<PointView>,
}

/// The answer of `progress.list`: a tile for every exercise with strength history, in the order they are shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressListView {
    pub items: Vec<ProgressItemView>,
}

/// A pause between two sessions of more than four weeks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GapView {
    /// The day of the session before the pause.
    pub from: NaiveDate,
    /// The day of the session after it; `None` while the pause is going on.
    pub to: Option<NaiveDate>,
    pub days: i64,
}

/// One set of a session, as the list of sessions shows it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBriefView {
    pub weight_kg: Option<f64>,
    pub reps: Option<u16>,
}

/// One day of an exercise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub date: NaiveDate,
    /// When the first activity of the day began (UTC).
    pub start: DateTime<Utc>,
    /// The activity of the working set, to open it.
    pub activity_id: ActivityId,
    /// The workout program of the first activity of the day.
    pub workout_name: Option<String>,
    pub working: WorkingView,
    /// Every set of the day, in workout order.
    pub sets: Vec<SetBriefView>,
}

/// The answer of `progress.get`: the tile of one exercise, its chart and its sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressView {
    pub exercise: ExerciseView,
    /// `None` for an exercise without strength history.
    pub summary: Option<ProgressItemView>,
    /// The step the weight goes up by in this exercise.
    pub step_kg: Option<f64>,
    /// One point per month, old to new.
    pub points: Vec<PointView>,
    /// The months in which a heavier weight than the point's was lifted for fewer reps than the chart asks.
    pub heavier: Vec<PointView>,
    pub gaps: Vec<GapView>,
    /// Newest first.
    pub sessions: Vec<SessionView>,
}

/// The day a set belongs to: the day on the user's clock where the activity was recorded, else the UTC date.
fn day_of(set: &HistorySet) -> NaiveDate {
    set.local_start.map_or_else(|| set.start.date_naive(), |local| local.date())
}

fn has_reps(set: &ProgressSet) -> bool {
    set.reps.is_some_and(|reps| reps.get() > 0)
}

/// The sessions of every strength exercise, old to new. One exercise on one day is one session, however many activities
/// the day had. An exercise none of whose sets has reps is left out: there is nothing in it to measure.
pub(crate) fn sessions_by_exercise(history: Vec<HistorySet>) -> BTreeMap<ExerciseId, Vec<ExerciseSession>> {
    let mut days: BTreeMap<ExerciseId, BTreeMap<NaiveDate, ExerciseSession>> = BTreeMap::new();
    for set in history.into_iter().filter(|set| is_strength(set.exercise)) {
        let day = day_of(&set);
        let session = days.entry(set.exercise).or_default().entry(day).or_insert_with(|| ExerciseSession {
            start: set.start,
            day,
            workout_name: set.workout_name.clone(),
            sets: Vec::new(),
        });
        // The history comes in order of the start, so this is the first activity of the day; it is kept true if it does not.
        if set.start < session.start {
            session.start = set.start;
            session.workout_name = set.workout_name.clone();
        }
        session.sets.push(ProgressSet { activity: set.activity, set: set.set, weight: set.weight, reps: set.reps, plan_reps: set.plan_reps });
    }
    days.into_iter()
        .filter_map(|(exercise, sessions)| {
            let mut sessions: Vec<ExerciseSession> = sessions.into_values().collect();
            if !sessions.iter().flat_map(|session| &session.sets).any(has_reps) {
                return None;
            }
            // The rule measures the distance between sessions on their start.
            sessions.sort_by_key(|session| (session.start, session.day));
            Some((exercise, sessions))
        })
        .collect()
}

fn working_view(session: &ExerciseSession, working: &WorkingSets) -> WorkingView {
    WorkingView { weight_kg: working.weight.kg(), reps: working.all_reps.clone(), date: session.day, activity_id: working.set.activity }
}

/// The working sets of the session at `index`; `None` for an index the rule did not give.
fn working_at(sessions: &[ExerciseSession], index: usize) -> Option<WorkingView> {
    let session = sessions.get(index)?;
    Some(working_view(session, &progress::working_sets(session)?))
}

fn advice_view(advice: &Advice, sessions: &[ExerciseSession]) -> AdviceView {
    match *advice {
        Advice::Add { to, target, previous, last, .. } => AdviceView {
            target: Some(target),
            next_weight_kg: Some(to.kg()),
            basis: [previous, last].into_iter().filter_map(|index| working_at(sessions, index)).collect(),
            ..AdviceView::bare(AdviceKind::Add)
        },
        Advice::Hold { target, sessions_at_weight, met_once, after_break, .. } => AdviceView {
            target: Some(target),
            sessions_at_weight: Some(sessions_at_weight),
            met_once,
            break_days: after_break.map(|pause| pause.days),
            before_break: after_break.and_then(|pause| working_at(sessions, pause.before)),
            ..AdviceView::bare(AdviceKind::Hold)
        },
        Advice::Break { days, .. } => AdviceView { break_days: Some(days), ..AdviceView::bare(AdviceKind::Break) },
        Advice::Bodyweight { .. } => AdviceView::bare(AdviceKind::Bodyweight),
        Advice::NoTarget { .. } => AdviceView::bare(AdviceKind::NoTarget),
    }
}

/// A point with the day it comes from; `None` for a session the rule did not give.
fn point_view(point: &MonthPoint, sessions: &[ExerciseSession]) -> Option<PointView> {
    Some(PointView {
        month: format!("{:04}-{:02}", point.month.year(), point.month.month()),
        weight_kg: point.weight.kg(),
        reps: point.reps,
        date: sessions.get(point.session)?.day,
        activity_id: point.set.activity,
    })
}

/// The points of the twelve calendar months that end with the month of `today`.
fn sparkline(series: &Series, sessions: &[ExerciseSession], today: DateTime<Utc>) -> Vec<PointView> {
    let from = today.date_naive().with_day(1).and_then(|first| first.checked_sub_months(Months::new(SPARKLINE_MONTHS.saturating_sub(1))));
    series.points.iter().filter(|point| from.is_none_or(|from| point.month >= from)).filter_map(|point| point_view(point, sessions)).collect()
}

fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// The tile of an exercise; `None` when there is nothing to advise on (no sessions).
fn item_view(
    exercise: ExerciseId,
    sessions: &[ExerciseSession],
    series: &Series,
    titles: &HashMap<ExerciseId, String>,
    today: DateTime<Utc>,
) -> Option<ProgressItemView> {
    let advice = progress::advise(sessions, today)?;
    let (oldest, newest) = (sessions.first()?, sessions.last()?);
    Some(ProgressItemView {
        exercise: exercise_view(exercise, titles),
        advice: advice_view(&advice, sessions),
        last: working_view(newest, &progress::working_sets(newest)?),
        sessions: count(sessions.len()),
        first_date: oldest.day,
        dormant: progress::days_between(newest.start, today) > DORMANT_DAYS,
        chart_reps: match series.mode {
            SeriesMode::Weight { reps } => Some(reps),
            SeriesMode::Reps => None,
        },
        sparkline: sparkline(series, sessions, today),
    })
}

/// Where a tile stands in the list: what to do comes first, then what is going on, then pauses, then what was given up.
fn group(item: &ProgressItemView) -> u8 {
    if item.dormant {
        return 3;
    }
    match item.advice.kind {
        AdviceKind::Add => 0,
        AdviceKind::Hold | AdviceKind::Bodyweight | AdviceKind::NoTarget => 1,
        AdviceKind::Break => 2,
    }
}

/// The tiles in the order they are shown: `add`, then `hold`, `bodyweight` and `noTarget`, then `break`, then dormant ones;
/// within a group the exercise done last comes first, and of two done at the same moment the one whose key is first.
pub(crate) fn list_view(
    by_exercise: &BTreeMap<ExerciseId, Vec<ExerciseSession>>,
    titles: &HashMap<ExerciseId, String>,
    today: DateTime<Utc>,
) -> Vec<ProgressItemView> {
    let mut tiles: Vec<(u8, DateTime<Utc>, ProgressItemView)> = by_exercise
        .iter()
        .filter_map(|(&exercise, sessions)| {
            let item = item_view(exercise, sessions, &progress::series(sessions, today), titles, today)?;
            Some((group(&item), sessions.last()?.start, item))
        })
        .collect();
    tiles.sort_by(|(group_a, start_a, a), (group_b, start_b, b)| {
        group_a.cmp(group_b).then_with(|| start_b.cmp(start_a)).then_with(|| a.exercise.id.cmp(&b.exercise.id))
    });
    tiles.into_iter().map(|(_, _, item)| item).collect()
}

fn gap_view(gap: &Gap, sessions: &[ExerciseSession]) -> Option<GapView> {
    Some(GapView { from: sessions.get(gap.from)?.day, to: gap.to.and_then(|index| sessions.get(index)).map(|session| session.day), days: gap.days })
}

fn session_view(session: &ExerciseSession) -> Option<SessionView> {
    let working = progress::working_sets(session)?;
    Some(SessionView {
        date: session.day,
        start: session.start,
        activity_id: working.set.activity,
        workout_name: session.workout_name.clone(),
        working: working_view(session, &working),
        sets: session.sets.iter().map(|set| SetBriefView { weight_kg: set.weight.map(Mass::kg), reps: set.reps.map(Reps::get) }).collect(),
    })
}

/// Everything about one exercise. `sessions` is `None` (or empty) for an exercise with no strength history, which still
/// gets a view: its names, and nothing else.
pub(crate) fn full_view(
    exercise: ExerciseId,
    sessions: Option<&[ExerciseSession]>,
    titles: &HashMap<ExerciseId, String>,
    today: DateTime<Utc>,
) -> ProgressView {
    let sessions = sessions.unwrap_or_default();
    let series = progress::series(sessions, today);
    ProgressView {
        exercise: exercise_view(exercise, titles),
        summary: item_view(exercise, sessions, &series, titles, today),
        step_kg: (!sessions.is_empty()).then(|| progress::weight_step(sessions).kg()),
        points: series.points.iter().filter_map(|point| point_view(point, sessions)).collect(),
        heavier: series.heavier.iter().filter_map(|point| point_view(point, sessions)).collect(),
        gaps: series.gaps.iter().filter_map(|gap| gap_view(gap, sessions)).collect(),
        sessions: sessions.iter().rev().filter_map(session_view).collect(),
    }
}

#[cfg(test)]
#[path = "progress_views_tests.rs"]
mod tests;
