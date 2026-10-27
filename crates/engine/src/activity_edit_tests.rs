//! Tests of the commands that correct sets: `activity.editSets`, `activity.resetSets`, `activity.addSet` and
//! `activity.removeAddedSet`. All of them check that nothing changes when a param is bad, and that the answer is the
//! activity as it is afterwards. The helpers are those of `activity_cmds_tests`.

use gymgar_domain::{CountReason, ExerciseId, SetRef};
use gymgar_sync::exercises::exercise_key;
use serde_json::{Value, json};

use super::tests::{INCLINE_CURL, ROMANIAN, assert_bad, find, fixture, get, list, ok, recorded, refs, run, set_at, synthetic};
use crate::Session;
use crate::activity_views::{ActivityView, ExerciseGroupView, ExerciseSearchView, SetView};
use crate::testing::{seed_strength, test_session};

// ----- activity.editSets

#[test]
fn edit_sets_corrects_weight_reps_and_exercise_over_the_record() {
    let mut session = test_session();
    let id = synthetic(&session);
    let before = get(&mut session, id);
    let view: ActivityView = serde_json::from_value(ok(
        &mut session,
        "activity.editSets",
        json!({"id": id.0, "sets": [{"recorded": 0}, {"recorded": 1}], "weightKg": 72.5, "reps": 8}),
    ))
    .unwrap();
    for index in [0, 1] {
        let set = find(&view, SetRef::Recorded(index));
        let record = set.recorded.clone().unwrap();
        assert_eq!((record.weight_kg, record.reps), (Some(70.0), Some(10)), "the record stays");
        let correction = set.correction.clone().unwrap();
        assert_eq!((correction.weight_kg, correction.reps, correction.exercise), (Some(72.5), Some(8), None));
        assert_eq!((set.effective.weight_kg, set.effective.reps), (Some(72.5), Some(8)));
    }
    // The others were not touched, and a field not given does not change.
    assert_eq!(find(&view, SetRef::Recorded(4)), find(&before, SetRef::Recorded(4)));
    let again: ActivityView =
        serde_json::from_value(ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 0}], "reps": 12}))).unwrap();
    let set = find(&again, SetRef::Recorded(0));
    assert_eq!((set.effective.weight_kg, set.effective.reps), (Some(72.5), Some(12)));
    // Boundaries.
    for (weight, reps) in [(0.0, 0), (1000.0, 1000)] {
        ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 1}], "weightKg": weight, "reps": reps}));
    }
}

#[test]
fn edit_sets_assigns_an_exercise_and_moves_the_sets_between_cards() {
    let mut session = test_session();
    let id = synthetic(&session);
    let before = get(&mut session, id);
    let no_exercise = before.groups.iter().find(|group| group.exercise.is_none()).unwrap();
    assert_eq!(no_exercise.sets.len(), 2);
    let view: ActivityView =
        serde_json::from_value(ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 4}], "exercise": INCLINE_CURL}))).unwrap();
    let card = |view: &ActivityView, key: &str| -> Option<ExerciseGroupView> {
        view.groups.iter().find(|group| group.exercise.as_ref().is_some_and(|e| e.id == key)).cloned()
    };
    assert_eq!(card(&view, INCLINE_CURL).unwrap().sets.len(), 1);
    assert_eq!(view.groups.iter().find(|group| group.exercise.is_none()).unwrap().sets.len(), 1);
    let set = find(&view, SetRef::Recorded(4));
    assert_eq!(set.recorded.unwrap().exercise, None);
    assert_eq!(set.correction.unwrap().exercise.map(|e| e.id), Some(INCLINE_CURL.to_owned()));
    assert_eq!(set.effective.exercise.map(|e| (e.id, e.english)), Some((INCLINE_CURL.to_owned(), "Incline dumbbell biceps curl".to_owned())));
}

#[test]
fn edit_sets_counted_is_true_false_or_null_for_the_rule() {
    let mut session = test_session();
    let id = synthetic(&session);
    let edit = |session: &mut Session, index: u16, counted: Value| -> SetView {
        let view: ActivityView =
            serde_json::from_value(ok(session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": index}], "counted": counted}))).unwrap();
        find(&view, SetRef::Recorded(index))
    };
    // A normal set, switched off, and back to the rule with null (a change on its own, not "nothing to change").
    assert_eq!(edit(&mut session, 0, json!(false)).count_reason, CountReason::ExcludedByUser);
    assert_eq!(edit(&mut session, 0, Value::Null).count_reason, CountReason::Counted);
    assert!(get(&mut session, id).groups.iter().flat_map(|g| g.sets.iter()).all(|set| set.correction.is_none()), "nothing is left of the correction");
    // An empty set, switched on over the rule, and back.
    assert_eq!(find(&get(&mut session, id), SetRef::Recorded(2)).count_reason, CountReason::Empty);
    let on = edit(&mut session, 2, json!(true));
    assert!(on.counted && on.count_reason == CountReason::Counted);
    assert_eq!(edit(&mut session, 2, Value::Null).count_reason, CountReason::Empty);
    // Absent leaves the flag alone.
    edit(&mut session, 0, json!(false));
    let view: ActivityView =
        serde_json::from_value(ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 0}], "weightKg": 71}))).unwrap();
    assert_eq!(find(&view, SetRef::Recorded(0)).count_reason, CountReason::ExcludedByUser);
}

#[test]
fn edit_sets_changes_an_added_set_itself() {
    let mut session = test_session();
    let id = synthetic(&session);
    let view: ActivityView =
        serde_json::from_value(ok(&mut session, "activity.addSet", json!({"id": id.0, "exercise": ROMANIAN, "weightKg": 40, "reps": 6}))).unwrap();
    let added = *refs(&view).iter().find(|r| matches!(r, SetRef::Added(_))).unwrap();
    let edited: ActivityView = serde_json::from_value(ok(
        &mut session,
        "activity.editSets",
        json!({"id": id.0, "sets": [recorded(added), {"recorded": 0}], "weightKg": 45, "exercise": INCLINE_CURL, "counted": false}),
    ))
    .unwrap();
    let set = find(&edited, added);
    assert!(set.added);
    assert_eq!((set.effective.weight_kg, set.effective.reps), (Some(45.0), Some(6)));
    assert_eq!(set.effective.exercise.map(|e| e.id), Some(INCLINE_CURL.to_owned()));
    assert_eq!(set.count_reason, CountReason::ExcludedByUser);
    // An added set has its own values only: no record, no correction.
    assert!(set.recorded.is_none() && set.correction.is_none());
    // And the recorded set of the same call was corrected as one.
    assert_eq!(find(&edited, SetRef::Recorded(0)).effective.weight_kg, Some(45.0));
}

#[test]
fn edit_sets_rejects_bad_params_and_changes_nothing() {
    let mut session = test_session();
    let id = synthetic(&session);
    let unknown_exercise = exercise_key(ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: None });
    let too_many: Vec<Value> = (0..201).map(|_| json!({"recorded": 0})).collect();
    let cases = [
        ("an unknown activity", json!({"id": 424_242, "sets": [{"recorded": 0}], "reps": 1})),
        ("id 0", json!({"id": 0, "sets": [{"recorded": 0}], "reps": 1})),
        ("a negative id", json!({"id": -1, "sets": [{"recorded": 0}], "reps": 1})),
        ("no sets", json!({"id": id.0, "reps": 1})),
        ("an empty list of sets", json!({"id": id.0, "sets": [], "reps": 1})),
        ("201 sets", json!({"id": id.0, "sets": too_many, "reps": 1})),
        ("a set the file does not have", json!({"id": id.0, "sets": [{"recorded": 60_000}], "reps": 1})),
        ("an added set that does not exist", json!({"id": id.0, "sets": [{"added": 1}], "reps": 1})),
        ("added id 0", json!({"id": id.0, "sets": [{"added": 0}], "reps": 1})),
        ("a negative added id", json!({"id": id.0, "sets": [{"added": -1}], "reps": 1})),
        ("a negative message index", json!({"id": id.0, "sets": [{"recorded": -1}], "reps": 1})),
        ("a message index past 16 bits", json!({"id": id.0, "sets": [{"recorded": 70_000}], "reps": 1})),
        ("a set ref with two keys", json!({"id": id.0, "sets": [{"recorded": 0, "added": 1}], "reps": 1})),
        ("a set ref with an unknown key", json!({"id": id.0, "sets": [{"other": 0}], "reps": 1})),
        ("a set ref that is a number", json!({"id": id.0, "sets": [0], "reps": 1})),
        ("a negative weight", json!({"id": id.0, "sets": [{"recorded": 0}], "weightKg": -1})),
        ("a weight over 1000 kg", json!({"id": id.0, "sets": [{"recorded": 0}], "weightKg": 1000.5})),
        ("a weight that is text", json!({"id": id.0, "sets": [{"recorded": 0}], "weightKg": "heavy"})),
        ("negative reps", json!({"id": id.0, "sets": [{"recorded": 0}], "reps": -1})),
        ("reps over 1000", json!({"id": id.0, "sets": [{"recorded": 0}], "reps": 1001})),
        ("fractional reps", json!({"id": id.0, "sets": [{"recorded": 0}], "reps": 1.5})),
        ("an unknown exercise", json!({"id": id.0, "sets": [{"recorded": 0}], "exercise": "not an exercise"})),
        ("an empty exercise", json!({"id": id.0, "sets": [{"recorded": 0}], "exercise": ""})),
        ("Garmin's unknown exercise", json!({"id": id.0, "sets": [{"recorded": 0}], "exercise": unknown_exercise})),
        ("a flag that is text", json!({"id": id.0, "sets": [{"recorded": 0}], "counted": "no"})),
        ("nothing to change", json!({"id": id.0, "sets": [{"recorded": 0}]})),
        ("an unknown key", json!({"id": id.0, "sets": [{"recorded": 0}], "reps": 1, "weight": 5})),
        ("a snake_case key", json!({"id": id.0, "sets": [{"recorded": 0}], "weight_kg": 5})),
    ];
    let before = get(&mut session, id);
    for (what, params) in cases {
        assert_bad(run(&mut session, "activity.editSets", params), "activity.editSets", what);
        assert_eq!(get(&mut session, id), before, "{what} changed something");
    }
}

#[test]
fn edit_sets_is_all_or_nothing() {
    let mut session = test_session();
    let id = synthetic(&session);
    let before = get(&mut session, id);
    // The third set is not in the file: the first two must not change either.
    let result = run(
        &mut session,
        "activity.editSets",
        json!({"id": id.0, "sets": [{"recorded": 0}, {"recorded": 1}, {"recorded": 999}], "weightKg": 99, "reps": 9, "counted": false}),
    );
    assert_bad(result, "activity.editSets", "an unknown third set");
    assert_eq!(get(&mut session, id), before);
    assert_eq!(list(&mut session, json!({}))[0].counted_sets, before.summary.counted_sets);
    // The same with an added set that is not there.
    assert_bad(
        run(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 0}, {"added": 5}], "reps": 9})),
        "activity.editSets",
        "an unknown added set",
    );
    assert_eq!(get(&mut session, id), before);
}

#[test]
fn edit_sets_of_an_activity_that_is_not_strength_is_bad_params() {
    let (mut session, _) = fixture();
    let run_id = list(&mut session, json!({"types": ["running"]}))[0].id;
    let before = get(&mut session, run_id);
    for (command, params) in [
        ("activity.editSets", json!({"id": run_id.0, "sets": [{"recorded": 0}], "reps": 1})),
        ("activity.resetSets", json!({"id": run_id.0, "sets": [{"recorded": 0}]})),
        ("activity.addSet", json!({"id": run_id.0, "exercise": ROMANIAN})),
        ("activity.removeAddedSet", json!({"id": run_id.0, "set": {"added": 1}})),
    ] {
        assert_bad(run(&mut session, command, params), command, "a run");
    }
    assert_eq!(get(&mut session, run_id), before);
}

// ----- activity.resetSets

#[test]
fn reset_sets_takes_the_corrections_back() {
    let mut session = test_session();
    let id = synthetic(&session);
    let before = get(&mut session, id);
    ok(
        &mut session,
        "activity.editSets",
        json!({"id": id.0, "sets": [{"recorded": 0}, {"recorded": 1}, {"recorded": 2}], "weightKg": 90, "reps": 3, "counted": false}),
    );
    ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 4}], "exercise": INCLINE_CURL}));
    // One set goes back; the others keep their changes.
    let view: ActivityView = serde_json::from_value(ok(&mut session, "activity.resetSets", json!({"id": id.0, "sets": [{"recorded": 0}]}))).unwrap();
    assert_eq!(find(&view, SetRef::Recorded(0)), find(&before, SetRef::Recorded(0)));
    assert!(find(&view, SetRef::Recorded(1)).correction.is_some());
    // The rest, with a set that has no correction among them (fine: it stays as it is).
    let view: ActivityView = serde_json::from_value(ok(
        &mut session,
        "activity.resetSets",
        json!({"id": id.0, "sets": [{"recorded": 1}, {"recorded": 2}, {"recorded": 3}, {"recorded": 4}]}),
    ))
    .unwrap();
    assert_eq!(view, before);
}

#[test]
fn reset_sets_rejects_bad_params_and_changes_nothing() {
    let mut session = test_session();
    let id = synthetic(&session);
    ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 0}], "reps": 3}));
    let added: ActivityView = serde_json::from_value(ok(&mut session, "activity.addSet", json!({"id": id.0, "exercise": ROMANIAN}))).unwrap();
    let added_ref = *refs(&added).iter().find(|r| matches!(r, SetRef::Added(_))).unwrap();
    let before = get(&mut session, id);
    let too_many: Vec<Value> = (0..201).map(|_| json!({"recorded": 0})).collect();
    let cases = [
        ("an added set", json!({"id": id.0, "sets": [{"recorded": 0}, recorded(added_ref)]})),
        ("an added set that does not exist", json!({"id": id.0, "sets": [{"added": 999}]})),
        ("a set the file does not have", json!({"id": id.0, "sets": [{"recorded": 0}, {"recorded": 999}]})),
        ("an unknown activity", json!({"id": 424_242, "sets": [{"recorded": 0}]})),
        ("id 0", json!({"id": 0, "sets": [{"recorded": 0}]})),
        ("an empty list", json!({"id": id.0, "sets": []})),
        ("201 sets", json!({"id": id.0, "sets": too_many})),
        ("no sets", json!({"id": id.0})),
        ("an unknown key", json!({"id": id.0, "sets": [{"recorded": 0}], "all": true})),
    ];
    for (what, params) in cases {
        assert_bad(run(&mut session, "activity.resetSets", params), "activity.resetSets", what);
        assert_eq!(get(&mut session, id), before, "{what} changed something");
    }
}

// ----- activity.addSet

#[test]
fn add_set_places_the_set_after_the_one_it_follows() {
    let mut session = test_session();
    let id = synthetic(&session);
    let view: ActivityView = serde_json::from_value(ok(
        &mut session,
        "activity.addSet",
        json!({"id": id.0, "exercise": "deadlift/romanian_deadlift", "weightKg": 65, "reps": 12, "after": {"recorded": 0}}),
    ))
    .unwrap();
    let added = *refs(&view).iter().find(|r| matches!(r, SetRef::Added(_))).unwrap();
    // It joined the deadlift card, right after set 0.
    assert_eq!(refs(&view)[..3], [SetRef::Recorded(0), added, SetRef::Recorded(1)]);
    let set = find(&view, added);
    assert_eq!((set.effective.weight_kg, set.effective.reps, set.counted), (Some(65.0), Some(12), true));
    assert_eq!(set.effective.exercise.map(|e| e.id), Some(ROMANIAN.to_owned()));

    // After an added set; and without `after`, at the end of the activity.
    let view: ActivityView =
        serde_json::from_value(ok(&mut session, "activity.addSet", json!({"id": id.0, "exercise": ROMANIAN, "after": recorded(added)}))).unwrap();
    let second = *refs(&view).iter().find(|r| matches!(r, SetRef::Added(n) if SetRef::Added(*n) != added)).unwrap();
    assert_eq!(refs(&view)[..4], [SetRef::Recorded(0), added, second, SetRef::Recorded(1)]);
    let view: ActivityView = serde_json::from_value(ok(&mut session, "activity.addSet", json!({"id": id.0, "exercise": INCLINE_CURL, "reps": 10}))).unwrap();
    let third = *refs(&view).iter().find(|r| matches!(r, SetRef::Added(n) if SetRef::Added(*n) != added && SetRef::Added(*n) != second)).unwrap();
    assert_eq!(refs(&view).last(), Some(&third), "without an anchor the set goes to the end: {:?}", refs(&view));
    // Its own card, since it is a new exercise; the weight is optional.
    let card = view.groups.iter().find(|group| group.exercise.as_ref().is_some_and(|e| e.id == INCLINE_CURL)).unwrap();
    assert_eq!(card.sets.iter().map(|set| set.effective.weight_kg).collect::<Vec<_>>(), [None]);
    // The activity's counts follow.
    assert_eq!(view.summary.counted_sets, 8);
}

#[test]
fn add_set_rejects_bad_params_and_changes_nothing() {
    let mut session = test_session();
    let id = synthetic(&session);
    let before = get(&mut session, id);
    let unknown_exercise = exercise_key(ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: None });
    let cases = [
        ("an unknown activity", json!({"id": 424_242, "exercise": ROMANIAN})),
        ("id 0", json!({"id": 0, "exercise": ROMANIAN})),
        ("no exercise", json!({"id": id.0})),
        ("an unknown exercise", json!({"id": id.0, "exercise": "nope"})),
        ("an empty exercise", json!({"id": id.0, "exercise": ""})),
        ("Garmin's unknown exercise", json!({"id": id.0, "exercise": unknown_exercise})),
        ("a negative weight", json!({"id": id.0, "exercise": ROMANIAN, "weightKg": -1})),
        ("a weight over 1000 kg", json!({"id": id.0, "exercise": ROMANIAN, "weightKg": 1001})),
        ("reps over 1000", json!({"id": id.0, "exercise": ROMANIAN, "reps": 1001})),
        ("negative reps", json!({"id": id.0, "exercise": ROMANIAN, "reps": -2})),
        ("an anchor the file does not have", json!({"id": id.0, "exercise": ROMANIAN, "after": {"recorded": 999}})),
        ("an anchor that is an added set that does not exist", json!({"id": id.0, "exercise": ROMANIAN, "after": {"added": 3}})),
        ("added id 0", json!({"id": id.0, "exercise": ROMANIAN, "after": {"added": 0}})),
        ("a negative added id", json!({"id": id.0, "exercise": ROMANIAN, "after": {"added": -4}})),
        ("an anchor that is not a set ref", json!({"id": id.0, "exercise": ROMANIAN, "after": 3})),
        ("an unknown key", json!({"id": id.0, "exercise": ROMANIAN, "counted": false})),
    ];
    for (what, params) in cases {
        assert_bad(run(&mut session, "activity.addSet", params), "activity.addSet", what);
        assert_eq!(get(&mut session, id), before, "{what} changed something");
    }
}

// ----- activity.removeAddedSet

#[test]
fn remove_added_set_takes_the_set_out_and_keeps_the_others_in_place() {
    let mut session = test_session();
    let id = synthetic(&session);
    let before = get(&mut session, id);
    let first: ActivityView =
        serde_json::from_value(ok(&mut session, "activity.addSet", json!({"id": id.0, "exercise": ROMANIAN, "reps": 5, "after": {"recorded": 0}}))).unwrap();
    let a = *refs(&first).iter().find(|r| matches!(r, SetRef::Added(_))).unwrap();
    let second: ActivityView =
        serde_json::from_value(ok(&mut session, "activity.addSet", json!({"id": id.0, "exercise": ROMANIAN, "reps": 6, "after": recorded(a)}))).unwrap();
    let b = *refs(&second).iter().find(|r| matches!(r, SetRef::Added(_)) && **r != a).unwrap();
    assert_eq!(refs(&second)[..4], [SetRef::Recorded(0), a, b, SetRef::Recorded(1)]);
    // Removing the first leaves the second where it was (it follows what the removed one followed).
    let view: ActivityView = serde_json::from_value(ok(&mut session, "activity.removeAddedSet", json!({"id": id.0, "set": recorded(a)}))).unwrap();
    assert_eq!(refs(&view)[..3], [SetRef::Recorded(0), b, SetRef::Recorded(1)]);
    let view: ActivityView = serde_json::from_value(ok(&mut session, "activity.removeAddedSet", json!({"id": id.0, "set": recorded(b)}))).unwrap();
    assert_eq!(view, before);
    // It is not lost: removing it again is not an error and changes nothing.
    assert_eq!(ok(&mut session, "activity.removeAddedSet", json!({"id": id.0, "set": recorded(b)})), serde_json::to_value(&before).unwrap());
}

#[test]
fn remove_added_set_rejects_bad_params_and_changes_nothing() {
    let mut session = test_session();
    let id = synthetic(&session);
    let other = synthetic(&session);
    let view: ActivityView = serde_json::from_value(ok(&mut session, "activity.addSet", json!({"id": other.0, "exercise": ROMANIAN}))).unwrap();
    let foreign = *refs(&view).iter().find(|r| matches!(r, SetRef::Added(_))).unwrap();
    let before = get(&mut session, id);
    let cases = [
        ("a recorded set", json!({"id": id.0, "set": {"recorded": 0}})),
        ("an added set that does not exist", json!({"id": id.0, "set": {"added": 999}})),
        ("added id 0", json!({"id": id.0, "set": {"added": 0}})),
        ("a negative added id", json!({"id": id.0, "set": {"added": -1}})),
        ("an added set of another activity", json!({"id": id.0, "set": recorded(foreign)})),
        ("an unknown activity", json!({"id": 424_242, "set": {"added": 1}})),
        ("id 0", json!({"id": 0, "set": {"added": 1}})),
        ("no set", json!({"id": id.0})),
        ("sets instead of set", json!({"id": id.0, "sets": [{"added": 1}]})),
    ];
    for (what, params) in cases {
        assert_bad(run(&mut session, "activity.removeAddedSet", params), "activity.removeAddedSet", what);
        assert_eq!(get(&mut session, id), before, "{what} changed something");
    }
    // The other activity's set is still there.
    assert!(refs(&get(&mut session, other)).contains(&foreign));
}

// ----- sets with no exercise

#[test]
fn no_exercise_group_and_assign() {
    let mut session = test_session();
    // Three sets with no exercise, the way the watch leaves them when it does not know what they were.
    let id = seed_strength(
        &session,
        vec![set_at(0, Some(10), Some(12.5), 30_000, None), set_at(1, Some(10), Some(12.5), 30_000, None), set_at(2, Some(9), Some(12.5), 30_000, None)],
    );
    let view = get(&mut session, id);
    assert_eq!(view.groups.len(), 1);
    assert!(view.groups[0].exercise.is_none());
    assert_eq!(view.groups[0].sets.len(), 3);
    assert_eq!(view.summary.exercise_count, 0, "no exercise is not an exercise");
    assert_eq!(view.summary.counted_sets, 3);
    // Nothing is "used" yet.
    let used: ExerciseSearchView = serde_json::from_value(ok(&mut session, "exercise.search", json!({"query": "incline"}))).unwrap();
    assert!(used.items.iter().all(|hit| hit.times_used == 0));

    let sets_to_edit: Vec<Value> = (0..3).map(|index| json!({"recorded": index})).collect();
    let view: ActivityView =
        serde_json::from_value(ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": sets_to_edit, "exercise": INCLINE_CURL}))).unwrap();
    assert_eq!(view.groups.len(), 1);
    assert_eq!(view.groups[0].exercise.as_ref().map(|e| e.id.as_str()), Some(INCLINE_CURL));
    assert_eq!(view.groups[0].sets.len(), 3);
    assert_eq!(view.summary.exercise_count, 1);

    let found: ExerciseSearchView = serde_json::from_value(ok(&mut session, "exercise.search", json!({"query": "incline"}))).unwrap();
    assert_eq!(found.items[0].exercise.id, INCLINE_CURL);
    assert_eq!(found.items[0].times_used, 3);
    assert!(found.items[1..].iter().all(|hit| hit.times_used == 0));
    // The activity is now found by the exercise.
    assert_eq!(list(&mut session, json!({"exercise": INCLINE_CURL})).iter().map(|item| item.id).collect::<Vec<_>>(), [id]);
}
