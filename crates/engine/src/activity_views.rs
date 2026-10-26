//! The views of imported activities: what `activity.list`, `activity.get`, the commands that change sets and
//! `exercise.search` answer.
//!
//! Plain serializable data, read back by the UI as well. They carry the final value of every set next to what the
//! watch recorded and what the user changed, so an agent can compute without a second call; they never carry what Garmin
//! sent verbatim (raw JSON, files, the owner's name and links).
//!
//! `build` turns what the store returns into the nested view. It decides the order of the sets (added ones next to the
//! set they follow), the cards (one per final exercise, in order of first appearance) and whether each set counts
//! (the same domain rules the store's counts use, so `countedSets` and the `counted` flags always agree). It reads nothing
//! and writes nothing.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use gymgar_domain::{ActivityId, AddedSet, CountReason, ExerciseId, FileState, Mass, PlannedTarget, RecordedSet, Reps, SetRef, count_reason, is_empty_set};
use gymgar_store::{ActivityDetail, ActivityRow};
use gymgar_sync::exercises::{english_name, exercise_key};
use serde::{Deserialize, Serialize};

/// The answer of `activity.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityListView {
    pub items: Vec<ActivityListItemView>,
}

/// One activity as a line of the list: its summary and the counts the store keeps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityListItemView {
    pub id: ActivityId,
    /// Garmin's activity type key, for example `strength_training` or `running`.
    pub type_key: String,
    pub name: Option<String>,
    /// When the activity started (UTC).
    pub start: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub distance_m: Option<f64>,
    pub avg_hr: Option<u16>,
    pub calories: Option<u32>,
    /// Not yet opened by the user.
    pub is_new: bool,
    /// Left out of the statistics by the user.
    pub excluded: bool,
    pub file_state: FileState,
    /// Sets that count towards statistics.
    pub counted_sets: u32,
    /// Different exercises among the counted sets.
    pub exercise_count: u32,
    /// The name of the workout program the watch followed.
    pub workout_name: Option<String>,
}

/// The answer of `activity.get` and of the commands that change an activity: the summary and the sets, by exercise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityView {
    #[serde(flatten)]
    pub summary: ActivityListItemView,
    /// One group per exercise, in order of the first set of each. The sets without an exercise are one group (its
    /// `exercise` is `None`), placed where the first of them is.
    pub groups: Vec<ExerciseGroupView>,
}

/// The sets of one exercise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExerciseGroupView {
    /// `None` for the sets without an exercise.
    pub exercise: Option<ExerciseView>,
    /// What the workout program asked for, when it asked the same of every set the watch recorded in this group;
    /// otherwise `None`, and each set has its own plan.
    pub plan: Option<PlanView>,
    pub sets: Vec<SetView>,
}

/// An exercise by its stable key and its names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExerciseView {
    /// The key the commands take, for example `deadlift/romanian_deadlift`.
    pub id: String,
    pub english: String,
    /// The title the watch gave it, in the watch's language.
    pub watch_title: Option<String>,
}

/// What the workout program asked for in a set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub reps: Option<u16>,
    pub weight_kg: Option<f64>,
}

/// One set: what the watch recorded, what the user changed, the final values and whether it counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetView {
    /// The key the commands take: `{"recorded": <message index>}` or `{"added": <id>}`.
    pub set: SetRef,
    /// Added by the user, not recorded by the watch.
    pub added: bool,
    /// `None` for an added set.
    pub start: Option<DateTime<Utc>>,
    pub duration_ms: Option<u32>,
    /// What the watch recorded; `None` for an added set.
    pub recorded: Option<SetValuesView>,
    /// What the user changed on top of the record; `None` when nothing was changed. Whether the set counts is not here
    /// but in `counted` and `count_reason`.
    pub correction: Option<SetValuesView>,
    /// The values the statistics use: the correction over the record (an added set is all its own).
    pub effective: SetValuesView,
    pub counted: bool,
    pub count_reason: CountReason,
    /// Only when the group has no plan of its own (the group's plan stands for all its sets).
    pub plan: Option<PlanView>,
    pub hr_avg: Option<u16>,
    pub hr_max: Option<u16>,
    pub rest_after_ms: Option<u32>,
}

/// The weight, repetitions and exercise of a set (or of its correction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetValuesView {
    pub weight_kg: Option<f64>,
    pub reps: Option<u16>,
    /// `None` is "no exercise".
    pub exercise: Option<ExerciseView>,
}

/// The answer of `exercise.search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExerciseSearchView {
    pub items: Vec<ExerciseHitView>,
}

/// One exercise found, with how many counted sets of it there are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExerciseHitView {
    pub exercise: ExerciseView,
    pub times_used: u32,
}

/// The list line of an activity.
pub(crate) fn list_item(row: &ActivityRow) -> ActivityListItemView {
    let summary = &row.summary;
    ActivityListItemView {
        id: summary.id,
        type_key: summary.type_key.clone(),
        name: summary.name.clone(),
        start: summary.start,
        duration_ms: summary.duration_ms,
        distance_m: summary.distance_m,
        avg_hr: summary.avg_hr,
        calories: summary.calories,
        is_new: row.is_new,
        excluded: row.excluded,
        file_state: row.file_state,
        counted_sets: row.counted_sets,
        exercise_count: row.exercise_count,
        workout_name: row.workout_name.clone(),
    }
}

/// The exercise as the views show it.
pub(crate) fn exercise_view(id: ExerciseId, titles: &HashMap<ExerciseId, String>) -> ExerciseView {
    ExerciseView { id: exercise_key(id), english: english_name(id), watch_title: titles.get(&id).cloned() }
}

/// Garmin's "unknown" category says the watch did not know the exercise: for the views (as for the store's counts) it
/// is no exercise at all.
fn real_exercise(exercise: Option<ExerciseId>) -> Option<ExerciseId> {
    exercise.filter(|exercise| exercise.category != ExerciseId::UNKNOWN_CATEGORY)
}

/// A set in the order of the activity: one the watch recorded, or one the user added.
#[derive(Clone, Copy)]
enum Entry<'a> {
    Recorded(&'a RecordedSet),
    Added(&'a AddedSet),
}

/// The sets of the activity in the order they are shown: the recorded ones in the order of the file, each added set
/// right after the set it was placed after. Added sets sharing an anchor keep the order of their ids; one placed after
/// nothing goes to the end, and so does one whose anchor cannot be found (a recorded set a re-parse dropped, an added
/// set that is gone), with the sets that follow it. A set is never dropped, whatever the anchors say: the loops a damaged
/// database could hold end up at the end too.
fn ordered<'a>(recorded: &'a [RecordedSet], added: &'a [AddedSet]) -> Vec<Entry<'a>> {
    let mut by_id: Vec<&AddedSet> = added.iter().collect();
    by_id.sort_by_key(|set| set.id);
    let recorded_known: HashSet<u16> = recorded.iter().map(|set| set.message_index).collect();
    let added_known: HashSet<i64> = by_id.iter().map(|set| set.id).collect();

    let mut following: HashMap<SetRef, Vec<&AddedSet>> = HashMap::new();
    let mut loose: Vec<&AddedSet> = Vec::new();
    for &set in &by_id {
        let anchor = match set.after {
            Some(SetRef::Recorded(index)) if recorded_known.contains(&index) => Some(SetRef::Recorded(index)),
            // A set that follows itself follows nothing that can be found.
            Some(SetRef::Added(id)) if id != set.id && added_known.contains(&id) => Some(SetRef::Added(id)),
            _ => None,
        };
        match anchor {
            Some(anchor) => following.entry(anchor).or_default().push(set),
            None => loose.push(set),
        }
    }

    let mut out: Vec<Entry<'a>> = Vec::with_capacity(recorded.len().saturating_add(added.len()));
    let mut emitted: HashSet<i64> = HashSet::new();
    for set in recorded {
        out.push(Entry::Recorded(set));
        push_following(SetRef::Recorded(set.message_index), &following, &mut emitted, &mut out);
    }
    for &set in &loose {
        if emitted.insert(set.id) {
            out.push(Entry::Added(set));
            push_following(SetRef::Added(set.id), &following, &mut emitted, &mut out);
        }
    }
    // What is left follows a set that is never shown first: a loop.
    for &set in &by_id {
        if emitted.insert(set.id) {
            out.push(Entry::Added(set));
            push_following(SetRef::Added(set.id), &following, &mut emitted, &mut out);
        }
    }
    out
}

/// Appends the added sets that follow `anchor`, each with the ones that follow it in turn. An explicit stack: a chain of
/// added sets must not run the thread out of its own. `emitted` keeps a set from being shown twice.
fn push_following<'a>(anchor: SetRef, following: &HashMap<SetRef, Vec<&'a AddedSet>>, emitted: &mut HashSet<i64>, out: &mut Vec<Entry<'a>>) {
    let mut stack: Vec<&AddedSet> = following.get(&anchor).map(|sets| sets.iter().rev().copied().collect()).unwrap_or_default();
    while let Some(set) = stack.pop() {
        if !emitted.insert(set.id) {
            continue;
        }
        out.push(Entry::Added(set));
        if let Some(next) = following.get(&SetRef::Added(set.id)) {
            stack.extend(next.iter().rev().copied());
        }
    }
}

fn plan_view(plan: PlannedTarget) -> PlanView {
    PlanView { reps: plan.reps.map(Reps::get), weight_kg: plan.weight.map(Mass::kg) }
}

/// What a card collects while the sets go by.
struct Group {
    exercise: Option<ExerciseView>,
    /// The plan of each set the watch recorded, in order.
    plans: Vec<Option<PlannedTarget>>,
    /// The sets with their own plans, which stay with them unless the card has one plan for all.
    sets: Vec<(SetView, Option<PlanView>)>,
}

impl Group {
    fn finish(self) -> ExerciseGroupView {
        // One plan for the card when every recorded set has the same; else each set shows its own.
        let shared = match self.plans.split_first() {
            Some((Some(first), rest)) if rest.iter().all(|plan| plan.as_ref() == Some(first)) => Some(plan_view(*first)),
            _ => None,
        };
        let sets = self.sets.into_iter().map(|(set, plan)| SetView { plan: if shared.is_some() { None } else { plan }, ..set }).collect();
        ExerciseGroupView { exercise: self.exercise, plan: shared, sets }
    }
}

/// The whole activity.
pub(crate) fn build(detail: ActivityDetail, titles: &HashMap<ExerciseId, String>) -> ActivityView {
    let ActivityDetail { row, recorded, corrections, added } = detail;
    let excluded = row.excluded;
    // An exercise's names are looked up once, however many sets it has.
    let mut views: HashMap<ExerciseId, ExerciseView> = HashMap::new();
    let mut view_of = |exercise: Option<ExerciseId>| -> Option<ExerciseView> {
        let exercise = real_exercise(exercise)?;
        Some(views.entry(exercise).or_insert_with(|| exercise_view(exercise, titles)).clone())
    };

    let mut groups: Vec<Group> = Vec::new();
    let mut index_of: HashMap<Option<ExerciseId>, usize> = HashMap::new();
    for entry in ordered(&recorded, &added) {
        let (set, exercise, recorded_plan) = match entry {
            Entry::Recorded(recorded) => {
                let correction = corrections.get(&recorded.message_index).copied().unwrap_or_default();
                let weight = correction.weight.or(recorded.weight);
                let reps = correction.reps.or(recorded.reps);
                // A correction that names an exercise replaces the recorded one.
                let exercise = real_exercise(correction.exercise.or(recorded.exercise));
                let reason = count_reason(excluded, correction.counted, is_empty_set(reps, weight, recorded.duration_ms));
                let changed = correction.weight.is_some() || correction.reps.is_some() || correction.exercise.is_some();
                let set = SetView {
                    set: SetRef::Recorded(recorded.message_index),
                    added: false,
                    start: Some(recorded.start),
                    duration_ms: Some(recorded.duration_ms),
                    recorded: Some(SetValuesView {
                        weight_kg: recorded.weight.map(Mass::kg),
                        reps: recorded.reps.map(Reps::get),
                        exercise: view_of(recorded.exercise),
                    }),
                    correction: changed.then(|| SetValuesView {
                        weight_kg: correction.weight.map(Mass::kg),
                        reps: correction.reps.map(Reps::get),
                        exercise: view_of(correction.exercise),
                    }),
                    effective: SetValuesView { weight_kg: weight.map(Mass::kg), reps: reps.map(Reps::get), exercise: view_of(exercise) },
                    counted: reason == CountReason::Counted,
                    count_reason: reason,
                    plan: None,
                    hr_avg: recorded.hr_avg,
                    hr_max: recorded.hr_max,
                    rest_after_ms: recorded.rest_after_ms,
                };
                (set, exercise, Some(recorded.plan))
            }
            Entry::Added(added) => {
                // The empty-set rule is for what the watch recorded, not for what the user made.
                let reason = count_reason(excluded, added.counted, false);
                let exercise = real_exercise(Some(added.exercise));
                let set = SetView {
                    set: SetRef::Added(added.id),
                    added: true,
                    start: None,
                    duration_ms: None,
                    recorded: None,
                    correction: None,
                    effective: SetValuesView { weight_kg: added.weight.map(Mass::kg), reps: added.reps.map(Reps::get), exercise: view_of(exercise) },
                    counted: reason == CountReason::Counted,
                    count_reason: reason,
                    plan: None,
                    hr_avg: None,
                    hr_max: None,
                    rest_after_ms: None,
                };
                (set, exercise, None)
            }
        };
        let position = *index_of.entry(exercise).or_insert_with(|| {
            groups.push(Group { exercise: view_of(exercise), plans: Vec::new(), sets: Vec::new() });
            groups.len().saturating_sub(1)
        });
        let Some(group) = groups.get_mut(position) else { continue };
        let plan = recorded_plan.flatten();
        if let Some(plan) = recorded_plan {
            group.plans.push(plan);
        }
        group.sets.push((set, plan.map(plan_view)));
    }
    ActivityView { summary: list_item(&row), groups: groups.into_iter().map(Group::finish).collect() }
}

#[cfg(test)]
#[path = "activity_views_tests.rs"]
mod tests;
