//! Tests of `activity_views::build`: the order of the sets, the cards, the plans and what counts. Pure: the activities
//! are built by hand, nothing touches a database.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, TimeZone, Utc};
use gymgar_domain::{ActivityId, ActivitySummary, AddedSet, CountReason, ExerciseId, FileState, Mass, PlannedTarget, RecordedSet, Reps, SetCorrection, SetRef};
use gymgar_store::{ActivityDetail, ActivityRow};
use serde_json::json;

use super::*;

const DEADLIFT: ExerciseId = ExerciseId { category: 8, name: Some(23) };
const SQUAT: ExerciseId = ExerciseId { category: 28, name: Some(1) };
const CARDIO: ExerciseId = ExerciseId { category: 2, name: None };

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 10, minute, 0).unwrap()
}

fn kg(value: f64) -> Mass {
    Mass::from_kg(value).unwrap()
}

fn reps(value: u32) -> Reps {
    Reps::new(value).unwrap()
}

fn row(excluded: bool) -> ActivityRow {
    ActivityRow {
        summary: ActivitySummary {
            id: ActivityId(7),
            type_key: "strength_training".to_owned(),
            name: Some("Leg day".to_owned()),
            start: at(0),
            duration_ms: Some(3_600_000),
            distance_m: None,
            avg_hr: Some(110),
            calories: Some(400),
            manual: false,
        },
        is_new: true,
        excluded,
        file_state: FileState::Downloaded,
        workout_name: Some("FB2".to_owned()),
        counted_sets: 0,
        exercise_count: 0,
    }
}

/// A recorded set of 30 s with the given reps, weight and exercise.
fn rec(index: u16, reps_count: Option<u32>, weight_kg: Option<f64>, exercise: Option<ExerciseId>) -> RecordedSet {
    RecordedSet {
        message_index: index,
        start: at(u32::from(index)),
        duration_ms: 30_000,
        reps: reps_count.map(reps),
        weight: weight_kg.map(kg),
        exercise,
        step_index: None,
        plan: None,
        hr_avg: Some(120 + index),
        hr_max: Some(150),
        rest_after_ms: Some(60_000),
    }
}

fn plan(reps_count: u32, weight_kg: f64) -> Option<PlannedTarget> {
    Some(PlannedTarget { reps: Some(reps(reps_count)), weight: Some(kg(weight_kg)) })
}

fn added(id: i64, after: Option<SetRef>, exercise: ExerciseId) -> AddedSet {
    AddedSet { id, after, exercise, weight: Some(kg(20.0)), reps: Some(reps(5)), counted: None }
}

fn detail(recorded: Vec<RecordedSet>, corrections: Vec<(u16, SetCorrection)>, added: Vec<AddedSet>) -> ActivityDetail {
    ActivityDetail { row: row(false), recorded, corrections: corrections.into_iter().collect::<BTreeMap<_, _>>(), added }
}

fn build_with(detail: ActivityDetail) -> ActivityView {
    build(detail, &HashMap::new())
}

/// The sets of every group, in the order the view shows them.
fn order(view: &ActivityView) -> Vec<SetRef> {
    view.groups.iter().flat_map(|group| group.sets.iter().map(|set| set.set)).collect()
}

fn three_deadlifts() -> Vec<RecordedSet> {
    (0..3).map(|index| rec(index, Some(10), Some(70.0), Some(DEADLIFT))).collect()
}

fn r(index: u16) -> SetRef {
    SetRef::Recorded(index)
}

fn a(id: i64) -> SetRef {
    SetRef::Added(id)
}

#[test]
fn an_added_set_follows_the_set_it_was_placed_after() {
    let view = build_with(detail(three_deadlifts(), vec![], vec![added(1, Some(r(1)), DEADLIFT)]));
    assert_eq!(order(&view), [r(0), r(1), a(1), r(2)]);
    assert_eq!(view.groups.len(), 1);
}

#[test]
fn an_added_set_without_an_anchor_goes_to_the_end() {
    // The store's own doc says "first" for no anchor; the view puts it last (Ruling 12 of the plan).
    let view = build_with(detail(three_deadlifts(), vec![], vec![added(1, None, DEADLIFT), added(2, Some(r(0)), DEADLIFT)]));
    assert_eq!(order(&view), [r(0), a(2), r(1), r(2), a(1)]);
}

#[test]
fn added_sets_sharing_an_anchor_keep_the_order_of_their_ids() {
    // Given in the "wrong" order on purpose: the order comes from the ids, not from the slice.
    let sets = vec![added(3, Some(a(1)), DEADLIFT), added(2, Some(r(0)), DEADLIFT), added(1, Some(r(0)), DEADLIFT)];
    let view = build_with(detail(three_deadlifts(), vec![], sets));
    // 3 follows 1; 2 shares the anchor with 1 and has the later id.
    assert_eq!(order(&view), [r(0), a(1), a(3), a(2), r(1), r(2)]);
}

#[test]
fn a_set_whose_anchor_cannot_be_found_goes_to_the_end_with_the_sets_that_follow_it() {
    let sets = vec![
        // A recorded set a re-parse dropped.
        added(1, Some(r(99)), DEADLIFT),
        // An added set that is no longer there.
        added(2, Some(a(77)), DEADLIFT),
        // Follows a set that is itself at the end.
        added(3, Some(a(1)), DEADLIFT),
        added(4, Some(r(2)), DEADLIFT),
    ];
    let view = build_with(detail(three_deadlifts(), vec![], sets));
    assert_eq!(order(&view), [r(0), r(1), r(2), a(4), a(1), a(3), a(2)]);
}

#[test]
fn no_set_is_dropped_even_when_the_anchors_form_a_loop() {
    // Impossible through the commands, possible in a damaged database: the view must neither hang nor lose a set.
    let sets = vec![added(1, Some(a(2)), DEADLIFT), added(2, Some(a(1)), DEADLIFT), added(3, Some(a(3)), DEADLIFT)];
    let view = build_with(detail(three_deadlifts(), vec![], sets));
    let mut shown = order(&view);
    assert_eq!(shown.len(), 6);
    shown.sort_by_key(|set| format!("{set:?}"));
    for expected in [r(0), r(1), r(2), a(1), a(2), a(3)] {
        assert!(shown.contains(&expected), "{expected:?} is missing");
    }
}

#[test]
fn a_long_chain_of_added_sets_does_not_overflow_the_stack() {
    let sets: Vec<AddedSet> = (1..=50_000).map(|id| added(id, if id == 1 { Some(r(0)) } else { Some(a(id - 1)) }, DEADLIFT)).collect();
    let view = build_with(detail(three_deadlifts(), vec![], sets));
    assert_eq!(order(&view).len(), 50_003);
}

#[test]
fn groups_follow_the_final_exercise_in_order_of_first_appearance() {
    let recorded = vec![
        rec(0, None, None, Some(CARDIO)),
        rec(1, Some(10), Some(70.0), Some(DEADLIFT)),
        rec(2, Some(8), Some(100.0), Some(SQUAT)),
        rec(3, Some(10), Some(70.0), Some(DEADLIFT)),
    ];
    let view = build_with(detail(recorded, vec![], vec![added(1, Some(r(2)), SQUAT)]));
    let exercises: Vec<&str> = view.groups.iter().map(|group| group.exercise.as_ref().map(|e| e.id.as_str()).unwrap_or("-")).collect();
    assert_eq!(exercises, ["cardio", "deadlift/romanian_deadlift", "squat/back_squat_with_body_bar"]);
    assert_eq!(view.groups.iter().map(|group| group.sets.len()).collect::<Vec<_>>(), [1, 2, 2]);
    // The card of an exercise is where its first set is; the added squat set joined the card of the squat set it follows.
    assert_eq!(view.groups.get(1).map(|group| group.sets.iter().map(|set| set.set).collect::<Vec<_>>()), Some(vec![r(1), r(3)]));
    assert_eq!(view.groups.get(2).map(|group| group.sets.iter().map(|set| set.set).collect::<Vec<_>>()), Some(vec![r(2), a(1)]));
}

#[test]
fn a_correction_moves_a_set_to_the_card_of_its_new_exercise() {
    let recorded = vec![rec(0, Some(10), Some(70.0), Some(DEADLIFT)), rec(1, Some(10), Some(70.0), Some(DEADLIFT)), rec(2, Some(8), Some(100.0), Some(SQUAT))];
    let correction = SetCorrection { exercise: Some(SQUAT), ..SetCorrection::default() };
    let view = build_with(detail(recorded, vec![(0, correction)], vec![]));
    // Set 0 is a squat now, and it is the first set: the squat card comes first.
    assert_eq!(view.groups.iter().map(|group| group.sets.iter().map(|set| set.set).collect::<Vec<_>>()).collect::<Vec<_>>(), [vec![r(0), r(2)], vec![r(1)]]);
    assert_eq!(view.groups.first().and_then(|group| group.exercise.as_ref()).map(|e| e.id.as_str()), Some("squat/back_squat_with_body_bar"));
}

#[test]
fn sets_without_an_exercise_are_one_card_where_the_first_of_them_is() {
    let unknown = ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: None };
    let recorded = vec![
        rec(0, Some(10), Some(70.0), Some(DEADLIFT)),
        rec(1, Some(10), Some(20.0), None),
        rec(2, Some(10), Some(70.0), Some(DEADLIFT)),
        rec(3, Some(10), Some(20.0), Some(unknown)),
        rec(4, Some(10), Some(20.0), None),
    ];
    let view = build_with(detail(recorded, vec![], vec![]));
    assert_eq!(view.groups.len(), 2);
    assert_eq!(view.groups.get(1).map(|group| group.exercise.clone()), Some(None));
    assert_eq!(view.groups.get(1).map(|group| group.sets.iter().map(|set| set.set).collect::<Vec<_>>()), Some(vec![r(1), r(3), r(4)]));
    // "No exercise" is not an exercise anywhere in the set.
    for set in view.groups.iter().flat_map(|group| group.sets.iter()).filter(|set| set.set != r(0) && set.set != r(2)) {
        assert_eq!(set.effective.exercise, None, "{:?}", set.set);
        assert_eq!(set.recorded.as_ref().and_then(|values| values.exercise.clone()), None, "{:?}", set.set);
    }
}

#[test]
fn the_group_has_the_plan_when_every_recorded_set_has_the_same() {
    let mut recorded = three_deadlifts();
    for set in &mut recorded {
        set.plan = plan(10, 70.0);
    }
    let view = build_with(detail(recorded, vec![], vec![added(1, None, DEADLIFT)]));
    let group = &view.groups[0];
    assert_eq!(group.plan, Some(PlanView { reps: Some(10), weight_kg: Some(70.0) }));
    // The plan is said once; the added set (which has none) does not spoil it.
    assert!(group.sets.iter().all(|set| set.plan.is_none()));
}

#[test]
fn the_sets_have_their_plans_when_the_plans_differ() {
    let mut recorded = three_deadlifts();
    recorded[0].plan = plan(10, 70.0);
    recorded[1].plan = plan(8, 75.0);
    recorded[2].plan = None;
    let view = build_with(detail(recorded, vec![], vec![]));
    let group = &view.groups[0];
    assert_eq!(group.plan, None);
    assert_eq!(
        group.sets.iter().map(|set| set.plan).collect::<Vec<_>>(),
        [Some(PlanView { reps: Some(10), weight_kg: Some(70.0) }), Some(PlanView { reps: Some(8), weight_kg: Some(75.0) }), None]
    );
}

#[test]
fn no_plan_anywhere_is_no_plan() {
    let view = build_with(detail(three_deadlifts(), vec![], vec![]));
    assert_eq!(view.groups[0].plan, None);
    assert!(view.groups[0].sets.iter().all(|set| set.plan.is_none()));
}

#[test]
fn a_set_shows_the_record_the_correction_and_the_final_values() {
    let recorded = vec![rec(0, Some(10), Some(70.0), Some(DEADLIFT)), rec(1, Some(10), Some(70.0), Some(DEADLIFT))];
    let correction = SetCorrection { weight: Some(kg(72.5)), reps: None, exercise: None, counted: None };
    let view = build_with(detail(recorded, vec![(1, correction)], vec![]));
    let sets = &view.groups[0].sets;
    // Untouched.
    assert_eq!(sets[0].correction, None);
    assert_eq!(sets[0].effective, sets[0].recorded.clone().unwrap());
    // Corrected: the record stays, the correction holds only what was changed, the final value is the correction over the record.
    let set = &sets[1];
    let recorded = set.recorded.as_ref().unwrap();
    assert_eq!((recorded.weight_kg, recorded.reps), (Some(70.0), Some(10)));
    let correction = set.correction.as_ref().unwrap();
    assert_eq!((correction.weight_kg, correction.reps, correction.exercise.clone()), (Some(72.5), None, None));
    assert_eq!((set.effective.weight_kg, set.effective.reps), (Some(72.5), Some(10)));
    assert_eq!(set.effective.exercise.as_ref().map(|e| e.id.as_str()), Some("deadlift/romanian_deadlift"));
    // The watch's own numbers are kept as they are.
    assert_eq!((set.start, set.duration_ms, set.hr_avg, set.hr_max, set.rest_after_ms), (Some(at(1)), Some(30_000), Some(121), Some(150), Some(60_000)));
    assert!(!set.added);
    assert_eq!(set.set, r(1));
}

#[test]
fn an_added_set_is_all_its_own() {
    let view = build_with(detail(three_deadlifts(), vec![], vec![added(5, Some(r(0)), SQUAT)]));
    let set = view.groups.iter().flat_map(|group| group.sets.iter()).find(|set| set.set == a(5)).unwrap();
    assert!(set.added);
    assert_eq!((set.start, set.duration_ms, set.recorded.clone(), set.correction.clone(), set.plan), (None, None, None, None, None));
    assert_eq!((set.hr_avg, set.hr_max, set.rest_after_ms), (None, None, None));
    assert_eq!((set.effective.weight_kg, set.effective.reps), (Some(20.0), Some(5)));
    assert_eq!(set.effective.exercise.as_ref().map(|e| e.id.as_str()), Some("squat/back_squat_with_body_bar"));
}

#[test]
fn what_counts_follows_the_domain_rules() {
    use CountReason::{ActivityExcluded, Counted, Empty, ExcludedByUser};
    let mut empty_short = rec(0, None, None, Some(DEADLIFT));
    empty_short.duration_ms = 2_000;
    let mut warm_up = rec(1, None, None, Some(CARDIO));
    warm_up.duration_ms = 278_350;
    let mut empty_but_wanted = rec(2, Some(0), Some(0.0), Some(DEADLIFT));
    empty_but_wanted.duration_ms = 3_000;
    let normal = rec(3, Some(10), Some(70.0), Some(DEADLIFT));
    let dropped = rec(4, Some(10), Some(70.0), Some(DEADLIFT));
    // Giving the empty set reps takes it out of the rule.
    let mut revived = rec(5, None, None, Some(DEADLIFT));
    revived.duration_ms = 2_000;
    let recorded = vec![empty_short, warm_up, empty_but_wanted, normal, dropped, revived];
    let corrections = vec![
        (2, SetCorrection { counted: Some(true), ..SetCorrection::default() }),
        (4, SetCorrection { counted: Some(false), ..SetCorrection::default() }),
        (5, SetCorrection { reps: Some(reps(8)), ..SetCorrection::default() }),
    ];
    let mut off = added(2, Some(r(0)), DEADLIFT);
    off.counted = Some(false);
    // The rule for empty sets never applies to a set the user added, even one with nothing in it.
    let mut blank = added(3, Some(r(0)), DEADLIFT);
    blank.weight = None;
    blank.reps = None;
    let added_sets = vec![added(1, None, DEADLIFT), off, blank];

    let view = build_with(detail(recorded.clone(), corrections.clone(), added_sets.clone()));
    let reasons: HashMap<SetRef, (bool, CountReason)> =
        view.groups.iter().flat_map(|group| group.sets.iter()).map(|set| (set.set, (set.counted, set.count_reason))).collect();
    let expected = [
        (r(0), Empty),
        (r(1), Counted),
        (r(2), Counted),
        (r(3), Counted),
        (r(4), ExcludedByUser),
        (r(5), Counted),
        (a(1), Counted),
        (a(2), ExcludedByUser),
        (a(3), Counted),
    ];
    for (set, reason) in expected {
        assert_eq!(reasons.get(&set), Some(&(reason == Counted, reason)), "{set:?}");
    }

    // An excluded activity counts nothing, whatever the user said about its sets.
    let mut gone = detail(recorded, corrections, added_sets);
    gone.row = row(true);
    let view = build_with(gone);
    assert!(view.groups.iter().flat_map(|group| group.sets.iter()).all(|set| !set.counted && set.count_reason == ActivityExcluded));
}

#[test]
fn watch_titles_are_attached_to_the_exercises_that_have_one() {
    let titles = HashMap::from([(DEADLIFT, "Румынская становая тяга".to_owned())]);
    let view = build(detail(vec![rec(0, Some(10), Some(70.0), Some(DEADLIFT)), rec(1, Some(10), Some(70.0), Some(SQUAT))], vec![], vec![]), &titles);
    let titles: Vec<Option<&str>> = view.groups.iter().map(|group| group.exercise.as_ref().and_then(|e| e.watch_title.as_deref())).collect();
    assert_eq!(titles, [Some("Румынская становая тяга"), None]);
    let english: Vec<&str> = view.groups.iter().filter_map(|group| group.exercise.as_ref().map(|e| e.english.as_str())).collect();
    assert_eq!(english[0], "Romanian deadlift");
}

#[test]
fn the_summary_is_flat_in_json_and_the_view_reads_back() {
    let view = build_with(detail(three_deadlifts(), vec![], vec![added(1, None, SQUAT)]));
    let value = serde_json::to_value(&view).unwrap();
    for key in [
        "id",
        "typeKey",
        "name",
        "start",
        "durationMs",
        "distanceM",
        "avgHr",
        "calories",
        "isNew",
        "excluded",
        "fileState",
        "countedSets",
        "exerciseCount",
        "workoutName",
        "groups",
    ] {
        assert!(value.get(key).is_some(), "{key} is missing: {value}");
    }
    assert_eq!(value["id"], json!(7));
    assert_eq!(value["fileState"], json!("downloaded"));
    assert_eq!(value["groups"][0]["sets"][0]["set"], json!({"recorded": 0}));
    assert_eq!(value["groups"][0]["sets"][0]["countReason"], json!("counted"));
    assert_eq!(value["groups"][1]["sets"][0]["set"], json!({"added": 1}));
    assert_eq!(serde_json::from_value::<ActivityView>(value).unwrap(), view);
}

#[test]
fn the_list_item_has_the_summary_and_the_counts() {
    let mut line = row(true);
    line.counted_sets = 12;
    line.exercise_count = 4;
    let item = list_item(&line);
    assert_eq!((item.id, item.type_key.as_str(), item.name.as_deref()), (ActivityId(7), "strength_training", Some("Leg day")));
    assert_eq!((item.is_new, item.excluded, item.counted_sets, item.exercise_count), (true, true, 12, 4));
    assert_eq!((item.workout_name.as_deref(), item.file_state, item.duration_ms), (Some("FB2"), FileState::Downloaded, Some(3_600_000)));
    assert_eq!(serde_json::from_value::<ActivityListItemView>(serde_json::to_value(&item).unwrap()).unwrap(), item);
}
