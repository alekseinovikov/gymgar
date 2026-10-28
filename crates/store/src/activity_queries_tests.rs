use std::collections::HashMap;

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use gymgar_domain::{ActivityId, AddedSet, ExerciseId, FileState, PlannedTarget, Reps, SetCorrection, SetRef, StrengthRecord};
use rusqlite::params_from_iter;

use super::*;
use crate::test_support::{
    activity, at, blank, count_rows, curl, deadlift, everything, kg, load, recorded, reps, squat, store_with, strength, unknown_exercise, work,
};
use crate::{FileOutcome, NewActivity, SetChange};

fn ids(rows: &[ActivityRow]) -> Vec<i64> {
    rows.iter().map(|row| row.summary.id.0).collect()
}

fn listed(store: &Store, filter: &ActivityFilter) -> Vec<i64> {
    ids(&store.list_activities(filter).unwrap())
}

fn with(change: impl FnOnce(&mut ActivityFilter)) -> ActivityFilter {
    let mut filter = everything();
    change(&mut filter);
    filter
}

fn edit(store: &Store, id: i64, set: SetRef, change: SetChange) {
    store.edit_sets(ActivityId(id), &[set], &change, at(25, 12)).unwrap();
}

fn set_counted(store: &Store, id: i64, set: SetRef, counted: Option<bool>) {
    edit(store, id, set, SetChange { counted: Some(counted), ..SetChange::default() });
}

fn set_exercise(store: &Store, id: i64, set: SetRef, exercise: ExerciseId) {
    edit(store, id, set, SetChange { exercise: Some(exercise), ..SetChange::default() });
}

fn add(store: &Store, id: i64, exercise: ExerciseId) -> i64 {
    store.add_set(ActivityId(id), None, exercise, None, None, at(25, 12)).unwrap()
}

#[test]
fn list_is_newest_first_and_ties_go_to_the_higher_id() {
    let same = at(22, 8);
    let store = store_with(&[strength(1, 20), activity(2, "strength_training", same, false), activity(3, "strength_training", same, false), strength(4, 21)]);
    assert_eq!(listed(&store, &everything()), [3, 2, 4, 1]);
    assert_eq!(listed(&Store::open_in_memory().unwrap(), &everything()), Vec::<i64>::new());
}

#[test]
fn list_rows_carry_the_summary_the_state_and_the_counts() {
    let mut summary_item = activity(7, "strength_training", at(20, 8), true);
    summary_item.summary.distance_m = Some(1234.5);
    summary_item.summary.name = None;
    let store = store_with(&[summary_item, activity(8, "running", at(21, 8), false)]);
    load(&store, 7, vec![work(0, Some(deadlift())), work(1, Some(squat())), blank(2, Some(squat()))]);

    let rows = store.list_activities(&everything()).unwrap();
    assert_eq!(rows.len(), 2);
    let run = &rows[0];
    assert_eq!(
        (run.summary.id, run.summary.type_key.as_str(), run.is_new, run.excluded, run.file_state),
        (ActivityId(8), "running", false, false, FileState::NotNeeded)
    );
    assert_eq!((run.counted_sets, run.exercise_count, run.workout_name.as_deref()), (0, 0, None));
    assert_eq!(run.summary.start, at(21, 8));

    let lifted = &rows[1];
    assert_eq!(lifted.summary.id, ActivityId(7));
    assert_eq!(lifted.summary.name, None);
    assert_eq!(lifted.summary.start, at(20, 8));
    assert_eq!(
        (lifted.summary.duration_ms, lifted.summary.distance_m, lifted.summary.avg_hr, lifted.summary.calories),
        (Some(3_600_000), Some(1234.5), Some(120), Some(300))
    );
    assert!(!lifted.summary.manual);
    assert!(lifted.is_new, "a row with no seen_at is new");
    assert_eq!(lifted.file_state, FileState::Downloaded);
    assert_eq!(lifted.workout_name.as_deref(), Some("Legs"));
    assert_eq!((lifted.counted_sets, lifted.exercise_count), (2, 2), "the blank set does not count");
}

#[test]
fn list_filters() {
    let store = store_with(&[
        activity(1, "strength_training", at(18, 8), false),
        activity(2, "running", at(19, 8), true),
        activity(3, "strength_training", at(20, 8), true),
        activity(4, "cycling", at(21, 8), false),
        activity(5, "strength_training", at(22, 8), false),
    ]);
    // The period: `from` is included, `to` is not.
    assert_eq!(listed(&store, &with(|f| f.from = Some(at(20, 8)))), [5, 4, 3]);
    assert_eq!(listed(&store, &with(|f| f.to = Some(at(20, 8)))), [2, 1]);
    assert_eq!(listed(&store, &with(|f| (f.from, f.to) = (Some(at(20, 8)), Some(at(22, 8))))), [4, 3]);
    assert_eq!(listed(&store, &with(|f| (f.from, f.to) = (Some(at(20, 8)), Some(at(20, 8))))), Vec::<i64>::new(), "an empty period");
    assert_eq!(listed(&store, &with(|f| (f.from, f.to) = (Some(at(23, 8)), Some(at(20, 8))))), Vec::<i64>::new(), "an upside-down period");

    // Types.
    assert_eq!(listed(&store, &with(|f| f.type_keys = vec!["running".to_owned(), "cycling".to_owned()])), [4, 2]);
    assert_eq!(listed(&store, &with(|f| f.type_keys = vec!["strength_training".to_owned()])), [5, 3, 1]);
    assert_eq!(listed(&store, &with(|f| f.strength_only = true)), [5, 3, 1]);
    assert_eq!(listed(&store, &with(|f| (f.strength_only, f.type_keys) = (true, vec!["running".to_owned()]))), Vec::<i64>::new(), "both must hold");
    assert_eq!(listed(&store, &with(|f| f.type_keys = vec!["swimming".to_owned()])), Vec::<i64>::new());
    // A type key is a value, never SQL.
    assert_eq!(listed(&store, &with(|f| f.type_keys = vec!["running' OR '1'='1".to_owned()])), Vec::<i64>::new());

    // New ones only.
    assert_eq!(listed(&store, &with(|f| f.only_new = true)), [3, 2]);
    assert_eq!(listed(&store, &with(|f| (f.only_new, f.strength_only) = (true, true))), [3]);
    assert_eq!(listed(&store, &with(|f| (f.only_new, f.to) = (true, Some(at(20, 8))))), [2]);

    // Everything together.
    let all = with(|f| {
        f.from = Some(at(19, 8));
        f.to = Some(at(23, 0));
        f.strength_only = true;
        f.only_new = false;
    });
    assert_eq!(listed(&store, &all), [5, 3]);

    // Paging.
    assert_eq!(listed(&store, &with(|f| f.limit = 2)), [5, 4]);
    assert_eq!(listed(&store, &with(|f| (f.limit, f.offset) = (2, 2))), [3, 2]);
    assert_eq!(listed(&store, &with(|f| (f.limit, f.offset) = (10, 4))), [1]);
    assert_eq!(listed(&store, &with(|f| (f.limit, f.offset) = (10, 5))), Vec::<i64>::new());
    assert_eq!(listed(&store, &with(|f| f.offset = 400_000)), Vec::<i64>::new());
    assert_eq!(listed(&store, &with(|f| f.limit = 0)), Vec::<i64>::new());
    assert_eq!(listed(&store, &with(|f| (f.limit, f.offset) = (u32::MAX, u32::MAX))), Vec::<i64>::new());
    assert_eq!(listed(&store, &with(|f| f.limit = u32::MAX)).len(), 5);
}

#[test]
fn excluded_activities_are_listed_only_on_request() {
    let store = store_with(&[strength(1, 20), strength(2, 21)]);
    store.set_activity_excluded(ActivityId(2), true).unwrap();
    assert_eq!(listed(&store, &with(|f| f.include_excluded = false)), [1]);
    assert_eq!(listed(&store, &with(|f| f.include_excluded = true)), [2, 1]);
}

#[test]
fn the_period_compares_on_the_stored_width_of_a_time() {
    // The start has a fraction of a second. Stored as `…:00.500000000Z`, it must sort after a bound stored as `…:00.000000000Z`.
    let mut item = strength(1, 20);
    item.summary.start = at(20, 8) + Duration::milliseconds(500);
    let store = store_with(&[item]);
    assert_eq!(listed(&store, &with(|f| f.from = Some(at(20, 8)))), [1]);
    assert_eq!(listed(&store, &with(|f| f.to = Some(at(20, 8)))), Vec::<i64>::new());
    assert_eq!(listed(&store, &with(|f| f.from = Some(at(20, 8) + Duration::seconds(1)))), Vec::<i64>::new());
    assert_eq!(listed(&store, &with(|f| f.to = Some(at(20, 8) + Duration::seconds(1)))), [1]);
    assert_eq!(listed(&store, &with(|f| f.from = Some(at(20, 8) + Duration::milliseconds(500)))), [1], "the bound is inclusive to the nanosecond");
    // A bound the database cannot spell is an error, not a wrong answer.
    let result = store.list_activities(&with(|f| f.from = Some(DateTime::<Utc>::MAX_UTC)));
    assert!(matches!(result, Err(StoreError::Sql(_))), "{result:?}");
}

#[test]
fn list_filters_by_the_final_exercise() {
    let store = store_with(&[strength(1, 20), strength(2, 21), strength(3, 22)]);
    // Activity 1 ends with a set the watch did not name: the user gives it a curl. That finds the activity by curl.
    load(&store, 1, vec![work(0, Some(deadlift())), work(1, None)]);
    load(&store, 2, vec![work(0, Some(deadlift()))]);
    load(&store, 3, vec![work(0, Some(squat()))]);
    let by = |exercise: ExerciseId| listed(&store, &with(|f| f.exercise = Some(exercise)));
    assert_eq!(by(curl()), Vec::<i64>::new());
    assert_eq!(by(deadlift()), [2, 1]);

    set_exercise(&store, 1, SetRef::Recorded(1), curl());
    assert_eq!(by(curl()), [1]);
    assert_eq!(by(deadlift()), [2, 1], "the other set of activity 1 is still a deadlift");
    // A correction replaces the recorded exercise: the squat set becomes a curl, so squat finds nothing.
    set_exercise(&store, 3, SetRef::Recorded(0), curl());
    assert_eq!(by(squat()), Vec::<i64>::new());
    assert_eq!(by(curl()), [3, 1]);

    // The exercise is a category and a name: the category alone is a different exercise.
    assert_eq!(by(ExerciseId { category: 8, name: None }), Vec::<i64>::new());
    assert_eq!(by(ExerciseId { category: 8, name: Some(24) }), Vec::<i64>::new());
    store.add_set(ActivityId(3), None, ExerciseId { category: 8, name: None }, None, None, at(25, 12)).unwrap();
    assert_eq!(by(ExerciseId { category: 8, name: None }), [3]);
    assert_eq!(by(deadlift()), [2, 1]);

    // It combines with the other filters.
    assert_eq!(listed(&store, &with(|f| (f.exercise, f.to) = (Some(deadlift()), Some(at(21, 8))))), [1]);
    assert_eq!(listed(&store, &with(|f| (f.exercise, f.limit) = (Some(curl()), 1))), [3]);
}

#[test]
fn the_exercise_filter_looks_only_at_sets_that_count() {
    let store = store_with(&[strength(1, 20), strength(2, 21), strength(3, 22), strength(4, 23), strength(5, 24), strength(6, 25)]);
    let by = |exercise: ExerciseId| listed(&store, &with(|f| f.exercise = Some(exercise)));
    // 1: only an empty set of a deadlift.
    load(&store, 1, vec![blank(0, Some(deadlift())), work(1, Some(squat()))]);
    // 2: an excluded activity.
    load(&store, 2, vec![work(0, Some(deadlift()))]);
    store.set_activity_excluded(ActivityId(2), true).unwrap();
    // 3: the user switched the deadlift set off.
    load(&store, 3, vec![work(0, Some(deadlift()))]);
    set_counted(&store, 3, SetRef::Recorded(0), Some(false));
    // 4: an empty deadlift set that the user switched on.
    load(&store, 4, vec![blank(0, Some(deadlift()))]);
    set_counted(&store, 4, SetRef::Recorded(0), Some(true));
    // 5: a deadlift only as an added set, which was removed (the squat one is kept).
    load(&store, 5, vec![work(0, Some(squat()))]);
    let removed = add(&store, 5, deadlift());
    store.remove_added_set(ActivityId(5), removed, at(25, 13)).unwrap();
    // 6: a deadlift only as an added set.
    load(&store, 6, vec![work(0, Some(squat()))]);
    add(&store, 6, deadlift());

    assert_eq!(by(deadlift()), [6, 4], "empty, excluded, switched off and removed do not count");
    assert_eq!(by(squat()), [6, 5, 1]);
    // An excluded activity is not found by an exercise even when excluded ones are listed (`everything` includes them).
    assert!(listed(&store, &everything()).contains(&2));

    // Switching a set on or the activity back in changes the answer at once.
    store.set_activity_excluded(ActivityId(2), false).unwrap();
    assert_eq!(by(deadlift()), [6, 4, 2]);
    set_counted(&store, 3, SetRef::Recorded(0), None);
    assert_eq!(by(deadlift()), [6, 4, 3, 2]);
    // Giving the empty set of 1 reps lifts the rule.
    edit(&store, 1, SetRef::Recorded(0), SetChange { reps: Some(reps(5)), ..SetChange::default() });
    assert_eq!(by(deadlift()), [6, 4, 3, 2, 1]);
}

#[test]
fn no_exercise_and_the_unknown_category_are_never_found_by_an_exercise() {
    let store = store_with(&[strength(1, 20)]);
    load(
        &store,
        1,
        vec![work(0, None), work(1, Some(unknown_exercise())), work(2, Some(ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: Some(3) }))],
    );
    store.add_set(ActivityId(1), None, unknown_exercise(), None, None, at(25, 12)).unwrap();
    let row = store.list_activities(&everything()).unwrap().remove(0);
    assert_eq!((row.counted_sets, row.exercise_count), (4, 0));
    for exercise in [unknown_exercise(), ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: Some(3) }] {
        assert_eq!(listed(&store, &with(|f| f.exercise = Some(exercise))), Vec::<i64>::new(), "{exercise:?}");
    }
    assert!(store.exercise_usage().unwrap().is_empty());
}

/// The data set where the three ways of counting meet: the counts stored with an activity (`recount`), the `exercise`
/// filter of the list and the usage of each exercise. They are written separately and must never disagree.
fn mixed_history() -> Store {
    let store = store_with(&[strength(1, 20), strength(2, 21), strength(3, 22), activity(4, "running", at(23, 8), false), strength(5, 24)]);
    let category_only = ExerciseId { category: 8, name: None };
    load(
        &store,
        1,
        vec![
            work(0, Some(deadlift())),
            work(1, Some(deadlift())),
            blank(2, Some(deadlift())),
            blank(3, Some(deadlift())),
            work(4, Some(squat())),
            work(5, None),
            work(6, Some(unknown_exercise())),
            blank(7, None),
        ],
    );
    set_counted(&store, 1, SetRef::Recorded(3), Some(true));
    set_counted(&store, 1, SetRef::Recorded(4), Some(false));
    edit(&store, 1, SetRef::Recorded(7), SetChange { reps: Some(reps(5)), exercise: Some(curl()), ..SetChange::default() });
    add(&store, 1, curl());
    let off = add(&store, 1, squat());
    set_counted(&store, 1, SetRef::Added(off), Some(false));
    let gone = add(&store, 1, ExerciseId { category: 0, name: Some(2) });
    store.remove_added_set(ActivityId(1), gone, at(25, 13)).unwrap();
    add(&store, 1, category_only);

    load(&store, 2, vec![work(0, Some(deadlift())), work(1, Some(deadlift()))]);
    add(&store, 2, squat());
    store.set_activity_excluded(ActivityId(2), true).unwrap();

    load(&store, 3, vec![work(0, Some(squat())), work(1, Some(squat())), work(2, Some(squat()))]);
    set_exercise(&store, 3, SetRef::Recorded(1), deadlift());

    let on = add(&store, 5, squat());
    set_counted(&store, 5, SetRef::Added(on), Some(true));
    add(&store, 5, deadlift());
    store
}

#[test]
fn the_stored_counts_the_filter_and_the_usage_agree() {
    let store = mixed_history();
    let rows = store.list_activities(&everything()).unwrap();
    let counted: HashMap<i64, (u32, u32)> = rows.iter().map(|row| (row.summary.id.0, (row.counted_sets, row.exercise_count))).collect();
    // Activity 1: sets 0 1 3 5 6 7 count (2 and 4 do not), plus two added ones; exercises deadlift, curl and the category 8 alone.
    // Activity 2 is excluded, 3 is three sets over two exercises, 5 holds two added sets over two exercises.
    assert_eq!(counted, HashMap::from([(1, (8, 3)), (2, (0, 0)), (3, (3, 2)), (4, (0, 0)), (5, (2, 2))]));

    let category_only = ExerciseId { category: 8, name: None };
    let usage = store.exercise_usage().unwrap();
    assert_eq!(usage, HashMap::from([(deadlift(), 5), (squat(), 3), (curl(), 2), (category_only, 1)]));
    // Of the 13 sets that count, two (no exercise, unknown) are not an exercise.
    let counted_sets: u32 = counted.values().map(|(sets, _)| sets).sum();
    assert_eq!(usage.values().sum::<u32>(), counted_sets - 2);

    // The history holds those same sets one by one: as many as the stored counts say, less the two that are no exercise,
    // and per exercise as many as the usage says.
    let history = store.strength_history().unwrap();
    assert_eq!(history.len(), usize::try_from(counted_sets - 2).unwrap());
    let mut by_exercise: HashMap<ExerciseId, u32> = HashMap::new();
    for set in &history {
        *by_exercise.entry(set.exercise).or_default() += 1;
    }
    assert_eq!(by_exercise, usage);

    // The filter finds exactly the activities whose stored count includes the exercise.
    let mut found_in: HashMap<i64, u32> = HashMap::new();
    for exercise in usage.keys() {
        let found = listed(&store, &with(|f| f.exercise = Some(*exercise)));
        assert!(!found.is_empty(), "{exercise:?}");
        for id in found {
            *found_in.entry(id).or_default() += 1;
        }
    }
    for (id, (_, exercise_count)) in &counted {
        assert_eq!(found_in.get(id).copied().unwrap_or(0), *exercise_count, "activity {id}");
    }
    assert_eq!(listed(&store, &with(|f| f.exercise = Some(deadlift()))), [5, 3, 1]);
    assert_eq!(listed(&store, &with(|f| f.exercise = Some(squat()))), [5, 3]);
    assert_eq!(listed(&store, &with(|f| f.exercise = Some(curl()))), [1]);
    assert_eq!(listed(&store, &with(|f| f.exercise = Some(category_only))), [1]);
    assert_eq!(listed(&store, &with(|f| f.exercise = Some(ExerciseId { category: 0, name: Some(2) }))), Vec::<i64>::new());
}

/// What `strength_history` should return for a set of the activity `id` begun at 08:00 on `day`, with no plan.
fn history_set(id: i64, day: u32, set: SetRef, exercise: ExerciseId, weight_kg: Option<f64>, reps_count: Option<u32>) -> HistorySet {
    HistorySet {
        activity: ActivityId(id),
        start: at(day, 8),
        local_start: None,
        workout_name: Some("Legs".to_owned()),
        set,
        exercise,
        weight: weight_kg.map(kg),
        reps: reps_count.map(reps),
        plan_reps: None,
    }
}

#[test]
fn history_holds_counted_sets_with_corrections() {
    let store = store_with(&[strength(1, 20), strength(2, 21)]);
    let unknown_name = ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: Some(3) };
    load(
        &store,
        1,
        vec![
            work(0, Some(deadlift())),         // corrected below: 60 kg, 5 reps
            work(1, Some(squat())),            // corrected below: a curl
            work(2, Some(deadlift())),         // switched off by the user
            blank(3, Some(deadlift())),        // empty by the rule
            blank(4, Some(deadlift())),        // empty by the rule, but switched on by the user
            work(5, None),                     // no exercise
            work(6, Some(unknown_exercise())), // Garmin's unknown category
            work(7, Some(unknown_name)),       // the same, with a name
        ],
    );
    edit(&store, 1, SetRef::Recorded(0), SetChange { weight: Some(kg(60.0)), reps: Some(reps(5)), ..SetChange::default() });
    set_exercise(&store, 1, SetRef::Recorded(1), curl());
    set_counted(&store, 1, SetRef::Recorded(2), Some(false));
    set_counted(&store, 1, SetRef::Recorded(4), Some(true));
    store.add_set(ActivityId(1), None, squat(), Some(kg(20.0)), Some(reps(10)), at(25, 12)).unwrap();
    let removed = store.add_set(ActivityId(1), None, deadlift(), Some(kg(99.0)), Some(reps(1)), at(25, 12)).unwrap();
    store.remove_added_set(ActivityId(1), removed, at(25, 13)).unwrap();
    // An excluded activity counts nothing, whatever its sets and added sets hold.
    load(&store, 2, vec![work(0, Some(deadlift()))]);
    add(&store, 2, squat());
    store.set_activity_excluded(ActivityId(2), true).unwrap();

    let added_id = store.activity_detail(ActivityId(1)).unwrap().unwrap().added.first().map(|set| set.id).unwrap();
    let expected = vec![
        history_set(1, 20, SetRef::Recorded(0), deadlift(), Some(60.0), Some(5)),
        history_set(1, 20, SetRef::Recorded(1), curl(), Some(50.0), Some(8)),
        history_set(1, 20, SetRef::Recorded(4), deadlift(), None, None),
        history_set(1, 20, SetRef::Added(added_id), squat(), Some(20.0), Some(10)),
    ];
    assert_eq!(store.strength_history().unwrap(), expected);

    // Excluding the activity takes everything out; putting it back changes the answer at once.
    store.set_activity_excluded(ActivityId(1), true).unwrap();
    assert_eq!(store.strength_history().unwrap(), Vec::<HistorySet>::new());
    store.set_activity_excluded(ActivityId(1), false).unwrap();
    store.set_activity_excluded(ActivityId(2), false).unwrap();
    assert_eq!(store.strength_history().unwrap().len(), 4 + 2);
}

#[test]
fn history_of_an_empty_database_is_empty() {
    assert_eq!(Store::open_in_memory().unwrap().strength_history().unwrap(), Vec::<HistorySet>::new());
    // Activities with no sets at all have no history either.
    assert_eq!(store_with(&[strength(1, 20)]).strength_history().unwrap(), Vec::<HistorySet>::new());
}

#[test]
fn history_order_and_plan() {
    // Inserted in the reverse order of their start; 3 begins at the very moment 1 does.
    let store = store_with(&[strength(1, 22), strength(2, 21), activity(3, "strength_training", at(22, 8), false)]);
    let mut planned = work(0, Some(deadlift()));
    planned.plan = Some(PlannedTarget { reps: Some(reps(10)), weight: Some(kg(70.0)) });
    let mut open_plan = work(1, Some(deadlift()));
    open_plan.plan = Some(PlannedTarget::default());
    // The sets of 1 go in out of order; the history reads them in the order of the file.
    load(&store, 1, vec![open_plan, work(2, Some(squat())), planned]);
    load(&store, 2, vec![work(1, Some(curl())), work(0, Some(curl()))]);
    load(&store, 3, vec![work(0, Some(squat()))]);
    // Added sets come after the recorded ones of their activity, in the order they were made, wherever they were put.
    let second_made = add(&store, 1, curl());
    let first_made = store.add_set(ActivityId(1), Some(SetRef::Recorded(0)), squat(), None, None, at(25, 12)).unwrap();
    let on_two = add(&store, 2, squat());
    assert!(second_made < first_made && first_made < on_two);

    let history = store.strength_history().unwrap();
    let order: Vec<(i64, SetRef)> = history.iter().map(|set| (set.activity.0, set.set)).collect();
    assert_eq!(
        order,
        [
            (2, SetRef::Recorded(0)),
            (2, SetRef::Recorded(1)),
            (2, SetRef::Added(on_two)),
            (1, SetRef::Recorded(0)),
            (1, SetRef::Recorded(1)),
            (1, SetRef::Recorded(2)),
            (1, SetRef::Added(second_made)),
            (1, SetRef::Added(first_made)),
            (3, SetRef::Recorded(0)),
        ]
    );
    // The plan is the plan of the recorded set; a plan that asked for no reps and added sets have none.
    let plans: Vec<Option<Reps>> = history.iter().map(|set| set.plan_reps).collect();
    assert_eq!(plans, [None, None, None, Some(reps(10)), None, None, None, None, None]);
    assert_eq!(history.iter().map(|set| set.start).collect::<Vec<_>>()[..4], [at(21, 8), at(21, 8), at(21, 8), at(22, 8)]);
}

#[test]
fn history_local_start() {
    let items = [
        ("{\"activityId\":1,\"startTimeLocal\":\"2026-11-01 00:30:00\"}", 20),
        ("{\"activityId\":2}", 21),
        ("{\"activityId\":3,\"startTimeLocal\":\"yesterday evening\"}", 22),
        ("{\"activityId\":4,\"startTimeLocal\":1793493000}", 23),
        ("{\"activityId\":5,\"startTimeLocal\":null}", 24),
        ("{\"activityId\":6,\"startTimeLocal\":\"2026-11-01T00:30:00\"}", 25),
        ("{\"activityId\":7,\"startTimeLocal\":\"2026-11-31 00:30:00\"}", 26),
        ("{\"activityId\":8,\"startTimeLocal\":\"2026-11-01 00:30:00.0\"}", 27),
        ("not json at all", 28),
        ("[1, 2]", 29),
    ];
    let activities: Vec<NewActivity> = items
        .iter()
        .zip(1..)
        .map(|((raw, day), id)| {
            let mut item = strength(id, *day);
            item.raw_json = (*raw).to_owned();
            item
        })
        .collect();
    let store = store_with(&activities);
    for id in 1..=10 {
        load(&store, id, vec![work(0, Some(deadlift()))]);
    }
    let local: Vec<(i64, Option<NaiveDateTime>)> = store.strength_history().unwrap().iter().map(|set| (set.activity.0, set.local_start)).collect();
    let half_past_midnight = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap().and_hms_opt(0, 30, 0);
    let mut expected = vec![(1, half_past_midnight)];
    expected.extend((2..=10).map(|id| (id, None)));
    assert_eq!(local, expected);
    // The local time is not the start in UTC: the two are kept apart.
    let first = store.strength_history().unwrap().remove(0);
    assert_eq!(first.start, at(20, 8));
}

#[test]
fn history_unusable_stored_values_are_errors_not_panics() {
    for sql in [
        "UPDATE activity SET start_at = 'soon'",
        "UPDATE recorded_set SET reps = 5000 WHERE message_index = 1",
        "UPDATE recorded_set SET weight_g = -1 WHERE message_index = 1",
        "UPDATE recorded_set SET exercise_category = 70000 WHERE message_index = 1",
        "UPDATE recorded_set SET exercise_name = -2 WHERE message_index = 1",
        "UPDATE recorded_set SET plan_reps = 5000 WHERE message_index = 0",
        "UPDATE recorded_set SET message_index = 70000 WHERE message_index = 1",
        "UPDATE set_correction SET weight_g = 2000000",
        "UPDATE set_correction SET reps = -1",
        "UPDATE added_set SET weight_g = -1",
        "UPDATE added_set SET reps = 5000",
        "UPDATE added_set SET exercise_name = 70000",
    ] {
        let store = damaged(sql);
        let history = store.strength_history();
        assert!(matches!(history, Err(StoreError::Corrupt(_))), "{sql}: {history:?}");
    }
    // The undamaged store reads: the corrected set, the other recorded one and the added one.
    assert_eq!(damaged("SELECT 1").strength_history().unwrap().len(), 3);
}

#[test]
fn exercise_usage_of_an_empty_database_is_empty() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.exercise_usage().unwrap().is_empty());
    assert!(store.exercise_titles().unwrap().is_empty());
    assert_eq!(store.new_count().unwrap(), 0);
}

/// The structure is what keeps a long history cheap, not a stopwatch: without an exercise filter the list reads the
/// counts stored with each activity (`counted_sets`, `exercise_count`), so no set is read however many there are.
#[test]
fn list_large_history_is_fast() {
    let store = Store::open_in_memory().unwrap();
    let items: Vec<NewActivity> = (1..=2500).map(|id| activity(id, "strength_training", at(1, 0) + Duration::hours(id), false)).collect();
    store.upsert_activities(&items, at(25, 10)).unwrap();
    let sets: Vec<_> = (0..30).map(|index| work(index, Some(deadlift()))).collect();
    for id in 1..=2500 {
        load(&store, id, sets.clone());
    }
    assert_eq!(count_rows(&store, "recorded_set"), 75_000);

    let filter = with(|f| f.limit = 1000);
    let rows = store.list_activities(&filter).unwrap();
    assert_eq!(rows.len(), 1000);
    assert_eq!(rows.first().map(|row| row.summary.id.0), Some(2500));
    assert_eq!(rows.last().map(|row| row.summary.id.0), Some(1501));
    assert!(rows.iter().all(|row| row.counted_sets == 30 && row.exercise_count == 1));

    let plan = |filter: &ActivityFilter| -> String {
        let (sql, params) = list_query(filter).unwrap();
        let mut statement = store.conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let details: Vec<String> = statement.query_map(params_from_iter(params), |row| row.get(3)).unwrap().collect::<Result<_, _>>().unwrap();
        details.join("\n")
    };
    // SQLite names a table by its alias in the plan (`SEARCH r ...` for `recorded_set r`), so the table names alone prove
    // nothing. The list without an exercise is a single scan of `activity`: no subquery, no lookup in any other table.
    let plain = plan(&filter);
    for table in ["recorded_set", "set_correction", "added_set", "SUBQUERY", "SEARCH", "JOIN"] {
        assert!(!plain.contains(table), "the list reads more than the activity ({table}):\n{plain}");
    }
    assert_eq!(plain.lines().count(), 1, "{plain}");
    assert!(plain.starts_with("SCAN a"), "{plain}");
    // The check is not vacuous: asking for an exercise does look at the sets.
    let by_exercise = plan(&with(|f| f.exercise = Some(deadlift())));
    assert!(by_exercise.contains("SUBQUERY") && by_exercise.contains("SEARCH r "), "{by_exercise}");
}

#[test]
fn activity_detail_returns_every_set_with_its_plan_and_heart_rate() {
    let store = store_with(&[strength(1, 20), strength(2, 21)]);
    let mut planned = work(2, Some(deadlift()));
    planned.step_index = Some(4);
    planned.plan = Some(PlannedTarget { reps: Some(reps(10)), weight: Some(kg(70.0)) });
    planned.hr_avg = Some(110);
    planned.hr_max = Some(131);
    planned.rest_after_ms = Some(60_000);
    // A plan that asked for nothing in particular is still a plan; no plan at all is `None`.
    let mut open_plan = work(1, None);
    open_plan.plan = Some(PlannedTarget::default());
    let plain = recorded(0, None, None, 278_350, Some(ExerciseId { category: 21, name: None }));
    // Inserted out of order: the detail is in the order of the file.
    load(&store, 1, vec![planned.clone(), plain.clone(), open_plan.clone()]);

    let detail = store.activity_detail(ActivityId(1)).unwrap().unwrap();
    assert_eq!(detail.recorded, [plain, open_plan, planned]);
    assert!(detail.corrections.is_empty());
    assert!(detail.added.is_empty());
    assert_eq!(detail.row.summary.id, ActivityId(1));
    assert_eq!(detail.row.workout_name.as_deref(), Some("Legs"));
    assert_eq!(detail.row.file_state, FileState::Downloaded);
    assert_eq!(detail.row.counted_sets, 3, "a long warm-up counts without reps and weight");
    // The row of the detail is the row of the list.
    let listed_row = store.list_activities(&everything()).unwrap().into_iter().find(|row| row.summary.id == ActivityId(1)).unwrap();
    assert_eq!(detail.row, listed_row);

    assert_eq!(store.activity_detail(ActivityId(99)).unwrap(), None);
    // An activity with no file has no recorded sets.
    let bare = store.activity_detail(ActivityId(2)).unwrap().unwrap();
    assert_eq!((bare.row.file_state, bare.recorded.len(), bare.row.counted_sets), (FileState::Pending, 0, 0));
}

#[test]
fn activity_detail_has_the_corrections_and_the_live_added_sets() {
    let store = store_with(&[strength(1, 20), strength(2, 21)]);
    load(&store, 1, vec![work(0, Some(deadlift())), blank(1, Some(deadlift())), work(2, Some(deadlift()))]);
    load(&store, 2, vec![work(0, Some(deadlift()))]);
    edit(&store, 1, SetRef::Recorded(0), SetChange { weight: Some(kg(77.5)), ..SetChange::default() });
    edit(&store, 1, SetRef::Recorded(1), SetChange { reps: Some(reps(8)), exercise: Some(curl()), counted: Some(Some(true)), ..SetChange::default() });
    edit(&store, 2, SetRef::Recorded(0), SetChange { weight: Some(kg(1.0)), ..SetChange::default() });

    let first = store.add_set(ActivityId(1), Some(SetRef::Recorded(2)), squat(), Some(kg(20.0)), Some(reps(10)), at(25, 12)).unwrap();
    let second = store.add_set(ActivityId(1), Some(SetRef::Added(first)), curl(), None, None, at(25, 12)).unwrap();
    let third = store.add_set(ActivityId(1), None, deadlift(), None, None, at(25, 12)).unwrap();
    store.remove_added_set(ActivityId(1), second, at(25, 13)).unwrap();
    set_counted(&store, 1, SetRef::Added(third), Some(false));

    let detail = store.activity_detail(ActivityId(1)).unwrap().unwrap();
    assert_eq!(detail.corrections.len(), 2, "another activity's correction is not here");
    assert_eq!(detail.corrections.get(&0), Some(&SetCorrection { weight: Some(kg(77.5)), ..SetCorrection::default() }));
    assert_eq!(detail.corrections.get(&1), Some(&SetCorrection { weight: None, reps: Some(reps(8)), exercise: Some(curl()), counted: Some(true) }));
    assert_eq!(
        detail.added,
        [
            AddedSet { id: first, after: Some(SetRef::Recorded(2)), exercise: squat(), weight: Some(kg(20.0)), reps: Some(reps(10)), counted: None },
            AddedSet { id: third, after: None, exercise: deadlift(), weight: None, reps: None, counted: Some(false) },
        ],
        "in the order they were made, the removed one left out"
    );
    // The recorded values are untouched by the corrections.
    assert_eq!(
        detail.recorded.iter().map(|set| (set.weight, set.reps.map(|r| r.get()))).collect::<Vec<_>>(),
        [(Some(kg(50.0)), Some(8)), (None, None), (Some(kg(50.0)), Some(8))]
    );

    // An activity without a file can still hold sets the user added.
    let bare = store_with(&[strength(3, 22)]);
    let id = bare.add_set(ActivityId(3), None, squat(), None, None, at(25, 12)).unwrap();
    let detail = bare.activity_detail(ActivityId(3)).unwrap().unwrap();
    assert_eq!((detail.recorded.len(), detail.added.iter().map(|set| set.id).collect::<Vec<_>>(), detail.row.counted_sets), (0, vec![id], 1));
}

#[test]
fn exercise_titles_are_returned_by_exercise() {
    let store = store_with(&[strength(1, 20)]);
    let warm_up = ExerciseId { category: 21, name: None };
    let record = StrengthRecord {
        workout_name: None,
        sets: vec![work(0, Some(deadlift()))],
        titles: vec![(deadlift(), "Deadlift".to_owned()), (warm_up, "Warm-up".to_owned()), (squat(), "Squat".to_owned()), (squat(), "Приседания".to_owned())],
    };
    store.apply_file(ActivityId(1), FileOutcome::Parsed { zip: Some(Vec::new()), record, parser_version: 1 }, at(25, 11)).unwrap();
    let expected = HashMap::from([(deadlift(), "Deadlift".to_owned()), (warm_up, "Warm-up".to_owned()), (squat(), "Приседания".to_owned())]);
    assert_eq!(store.exercise_titles().unwrap(), expected);
}

#[test]
fn mark_seen_and_new_count() {
    let store = store_with(&[
        activity(1, "strength_training", at(20, 8), true),
        activity(2, "running", at(21, 8), true),
        activity(3, "strength_training", at(22, 8), false),
        activity(4, "strength_training", at(23, 8), true),
    ]);
    assert_eq!(store.new_count().unwrap(), 3);
    assert_eq!(listed(&store, &with(|f| f.only_new = true)), [4, 2, 1]);

    store.mark_seen(ActivityId(2), at(25, 12)).unwrap();
    assert_eq!(store.new_count().unwrap(), 2);
    assert_eq!(listed(&store, &with(|f| f.only_new = true)), [4, 1]);
    let row = store.list_activities(&everything()).unwrap().into_iter().find(|row| row.summary.id == ActivityId(2)).unwrap();
    assert!(!row.is_new);

    // Seeing it again, or an activity that was never new, changes nothing.
    store.mark_seen(ActivityId(2), at(26, 12)).unwrap();
    store.mark_seen(ActivityId(3), at(26, 12)).unwrap();
    assert_eq!(store.new_count().unwrap(), 2);

    // An excluded activity is still new: the list keeps showing it when asked to, so the number must match.
    store.set_activity_excluded(ActivityId(1), true).unwrap();
    assert_eq!(store.new_count().unwrap(), 2);
    store.mark_seen(ActivityId(1), at(27, 12)).unwrap();
    store.mark_seen(ActivityId(4), at(27, 12)).unwrap();
    assert_eq!(store.new_count().unwrap(), 0);
    assert_eq!(listed(&store, &with(|f| f.only_new = true)), Vec::<i64>::new());
}

/// A store with one rich activity, to be damaged by hand.
fn damaged(sql: &str) -> Store {
    let store = store_with(&[strength(1, 20)]);
    let mut set = work(0, Some(deadlift()));
    set.plan = Some(PlannedTarget { reps: Some(reps(10)), weight: Some(kg(70.0)) });
    load(&store, 1, vec![set, work(1, Some(squat()))]);
    edit(&store, 1, SetRef::Recorded(0), SetChange { weight: Some(kg(60.0)), reps: Some(reps(5)), exercise: Some(curl()), counted: Some(Some(true)) });
    let added = store.add_set(ActivityId(1), Some(SetRef::Recorded(0)), squat(), Some(kg(20.0)), Some(reps(10)), at(25, 12)).unwrap();
    assert!(added > 0);
    store.conn.execute_batch("INSERT INTO exercise_title (category, name, title, seen_at) VALUES (8, 23, 'Deadlift', '2026-10-25T12:00:00Z')").unwrap();
    store.conn.execute_batch(sql).unwrap();
    store
}

#[test]
fn unusable_stored_values_are_errors_not_panics() {
    let in_the_row = [
        "UPDATE activity SET start_at = 'soon'",
        "UPDATE activity SET file_state = 'bogus'",
        "UPDATE activity SET counted_sets = -1",
        "UPDATE activity SET exercise_count = 5000000000",
        "UPDATE activity SET avg_hr = 70000",
        "UPDATE activity SET calories = -3",
        "UPDATE activity SET duration_ms = -5",
    ];
    for sql in in_the_row {
        let store = damaged(sql);
        let list = store.list_activities(&everything());
        assert!(matches!(list, Err(StoreError::Corrupt(_))), "{sql}: {list:?}");
        let detail = store.activity_detail(ActivityId(1));
        assert!(matches!(detail, Err(StoreError::Corrupt(_))), "{sql}: {detail:?}");
    }
    let in_the_details = [
        "UPDATE recorded_set SET start_at = 'x' WHERE message_index = 1",
        "UPDATE recorded_set SET reps = 5000 WHERE message_index = 1",
        "UPDATE recorded_set SET weight_g = -1 WHERE message_index = 1",
        "UPDATE recorded_set SET exercise_category = 70000 WHERE message_index = 1",
        "UPDATE recorded_set SET exercise_name = -2 WHERE message_index = 1",
        "UPDATE recorded_set SET duration_ms = -1 WHERE message_index = 1",
        "UPDATE recorded_set SET plan_reps = 5000 WHERE message_index = 0",
        "UPDATE recorded_set SET plan_weight_g = 2000000 WHERE message_index = 0",
        "UPDATE recorded_set SET hr_avg = 70000 WHERE message_index = 1",
        "UPDATE recorded_set SET message_index = 70000 WHERE message_index = 1",
        "UPDATE set_correction SET weight_g = 2000000",
        "UPDATE set_correction SET reps = -1",
        "UPDATE set_correction SET exercise_category = 70000",
        "UPDATE added_set SET after_value = 70000 WHERE after_kind = 'recorded'",
        "UPDATE added_set SET weight_g = -1",
        "UPDATE added_set SET reps = 5000",
        "UPDATE added_set SET exercise_category = -1",
    ];
    for sql in in_the_details {
        let store = damaged(sql);
        let detail = store.activity_detail(ActivityId(1));
        assert!(matches!(detail, Err(StoreError::Corrupt(_))), "{sql}: {detail:?}");
    }
    for sql in ["UPDATE exercise_title SET name = -5", "UPDATE exercise_title SET category = 70000"] {
        let store = damaged(sql);
        let titles = store.exercise_titles();
        assert!(matches!(titles, Err(StoreError::Corrupt(_))), "{sql}: {titles:?}");
    }
    // The undamaged store is fine.
    let store = damaged("SELECT 1");
    assert_eq!(store.activity_detail(ActivityId(1)).unwrap().map(|detail| detail.recorded.len()), Some(2));
    assert_eq!(store.exercise_titles().unwrap().len(), 1);
}
