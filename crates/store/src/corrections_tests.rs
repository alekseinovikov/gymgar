use chrono::{DateTime, Utc};
use gymgar_domain::{ActivityId, AddedSet, ExerciseId, SetCorrection, SetRef};

use super::*;
use crate::ActivityDetail;
use crate::test_support::{at, blank, count_rows, curl, deadlift, everything, kg, load, recorded, reps, squat, store_with, strength, unknown_exercise, work};

const ID: ActivityId = ActivityId(1);

fn detail(store: &Store, id: i64) -> ActivityDetail {
    store.activity_detail(ActivityId(id)).unwrap().unwrap()
}

/// `(counted sets, distinct exercises)` as the activity row reports them.
fn counts(store: &Store, id: i64) -> (u32, u32) {
    let row = detail(store, id).row;
    (row.counted_sets, row.exercise_count)
}

fn weight(value: f64) -> SetChange {
    SetChange { weight: Some(kg(value)), ..SetChange::default() }
}

fn counted(value: Option<bool>) -> SetChange {
    SetChange { counted: Some(value), ..SetChange::default() }
}

/// One text column of the single row of `table` that matches `filter`.
fn text(store: &Store, table: &str, column: &str, filter: &str) -> Option<String> {
    store.conn.query_row(&format!("SELECT {column} FROM {table} WHERE {filter}"), [], |row| row.get(0)).unwrap()
}

/// What `now` looks like once stored: fixed-width RFC 3339.
fn stored(time: DateTime<Utc>) -> String {
    crate::activities::format_time(time).unwrap()
}

fn is_not_found<T: std::fmt::Debug>(result: &Result<T, StoreError>) -> bool {
    matches!(result, Err(StoreError::NotFound(_)))
}

#[test]
fn edit_sets_is_atomic() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(4, Some(deadlift())), work(6, Some(deadlift()))]);
    let result = store.edit_sets(ID, &[SetRef::Recorded(4), SetRef::Recorded(6), SetRef::Recorded(99)], &weight(77.5), at(25, 12));
    assert!(is_not_found(&result), "{result:?}");
    assert_eq!(count_rows(&store, "set_correction"), 0, "the first two sets were not corrected either");
    assert!(detail(&store, 1).corrections.is_empty());

    // Same for an added set that does not exist, and for a mix of both kinds.
    let id = store.add_set(ID, None, squat(), None, None, at(25, 12)).unwrap();
    let result = store.edit_sets(ID, &[SetRef::Recorded(4), SetRef::Added(id), SetRef::Added(id + 1)], &weight(77.5), at(25, 12));
    assert!(is_not_found(&result), "{result:?}");
    assert_eq!(count_rows(&store, "set_correction"), 0);
    assert_eq!(detail(&store, 1).added[0].weight, None, "the added set kept its weight");
}

#[test]
fn edit_then_reset() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(3, Some(deadlift())), work(4, Some(deadlift())), work(5, Some(deadlift()))]);
    store.edit_sets(ID, &[SetRef::Recorded(4)], &weight(77.5), at(25, 12)).unwrap();

    let after = detail(&store, 1);
    assert_eq!(after.corrections.len(), 1);
    assert_eq!(after.corrections.get(&4), Some(&SetCorrection { weight: Some(kg(77.5)), ..SetCorrection::default() }));
    let record = after.recorded.iter().find(|set| set.message_index == 4).unwrap();
    assert_eq!(record.weight, Some(kg(50.0)), "the watch's value is not overwritten");
    assert_eq!(text(&store, "set_correction", "updated_at", "message_index = 4"), Some(stored(at(25, 12))));

    store.reset_sets(ID, &[4]).unwrap();
    assert!(detail(&store, 1).corrections.is_empty());
    assert_eq!(count_rows(&store, "set_correction"), 0);
    // Resetting a set that has nothing to reset is fine.
    store.reset_sets(ID, &[4, 3]).unwrap();
    store.reset_sets(ID, &[]).unwrap();
}

#[test]
fn edits_of_one_set_accumulate_and_the_exercise_replaces_name_and_category() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(0, None)]);
    store.edit_sets(ID, &[SetRef::Recorded(0)], &weight(60.0), at(25, 12)).unwrap();
    store.edit_sets(ID, &[SetRef::Recorded(0)], &SetChange { reps: Some(reps(5)), ..SetChange::default() }, at(25, 13)).unwrap();
    store.edit_sets(ID, &[SetRef::Recorded(0)], &SetChange { exercise: Some(curl()), ..SetChange::default() }, at(25, 14)).unwrap();
    let expected = SetCorrection { weight: Some(kg(60.0)), reps: Some(reps(5)), exercise: Some(curl()), counted: None };
    assert_eq!(detail(&store, 1).corrections.get(&0), Some(&expected), "a field that was not in the change is kept");
    assert_eq!(text(&store, "set_correction", "updated_at", "message_index = 0"), Some(stored(at(25, 14))));

    // A category alone does not keep the name of the exercise it replaces.
    let category_only = ExerciseId { category: 21, name: None };
    store.edit_sets(ID, &[SetRef::Recorded(0)], &SetChange { exercise: Some(category_only), ..SetChange::default() }, at(25, 15)).unwrap();
    assert_eq!(detail(&store, 1).corrections.get(&0).and_then(|c| c.exercise), Some(category_only));

    // One change for several sets at once.
    load(&store, 1, vec![work(0, None), work(1, None), work(2, None)]);
    store
        .edit_sets(
            ID,
            &[SetRef::Recorded(1), SetRef::Recorded(2), SetRef::Recorded(1)],
            &SetChange { exercise: Some(squat()), ..SetChange::default() },
            at(25, 16),
        )
        .unwrap();
    let after = detail(&store, 1);
    assert_eq!(after.corrections.get(&1).and_then(|c| c.exercise), Some(squat()));
    assert_eq!(after.corrections.get(&2).and_then(|c| c.exercise), Some(squat()));
    assert_eq!(counts(&store, 1), (3, 2), "set 0 keeps its category 21 correction, sets 1 and 2 are squats");
}

#[test]
fn a_correction_that_says_nothing_is_not_kept() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(0, Some(deadlift())), blank(1, Some(deadlift()))]);
    // An empty change writes nothing.
    store.edit_sets(ID, &[SetRef::Recorded(0)], &SetChange::default(), at(25, 12)).unwrap();
    assert_eq!(count_rows(&store, "set_correction"), 0);
    // Turning the only field back to "automatic" leaves a row with nothing in it, which is dropped.
    store.edit_sets(ID, &[SetRef::Recorded(1)], &counted(Some(true)), at(25, 12)).unwrap();
    assert_eq!(count_rows(&store, "set_correction"), 1);
    store.edit_sets(ID, &[SetRef::Recorded(1)], &counted(None), at(25, 13)).unwrap();
    assert_eq!(count_rows(&store, "set_correction"), 0);
    assert!(detail(&store, 1).corrections.is_empty());
    // "Automatic" for a set that never had a flag creates nothing either.
    store.edit_sets(ID, &[SetRef::Recorded(0)], &counted(None), at(25, 14)).unwrap();
    assert_eq!(count_rows(&store, "set_correction"), 0);
}

#[test]
fn empty_set_counted_by_user() {
    let store = store_with(&[strength(1, 20)]);
    // Set 2 lasted two seconds and has no reps: the automatic rule leaves it out.
    load(&store, 1, vec![work(0, Some(deadlift())), work(1, Some(deadlift())), blank(2, Some(deadlift()))]);
    assert_eq!(counts(&store, 1).0, 2);

    store.edit_sets(ID, &[SetRef::Recorded(2)], &counted(Some(true)), at(25, 12)).unwrap();
    assert_eq!(counts(&store, 1).0, 3, "the user says it counts");
    assert_eq!(detail(&store, 1).corrections.get(&2).and_then(|c| c.counted), Some(true));

    store.edit_sets(ID, &[SetRef::Recorded(2)], &counted(None), at(25, 13)).unwrap();
    assert_eq!(counts(&store, 1).0, 2, "back to the automatic rule");
    assert_eq!(detail(&store, 1).corrections.get(&2), None);

    // The other way round: a real set that the user switches off, and back on by the rule.
    store.edit_sets(ID, &[SetRef::Recorded(0)], &counted(Some(false)), at(25, 14)).unwrap();
    assert_eq!(counts(&store, 1).0, 1);
    store.edit_sets(ID, &[SetRef::Recorded(0)], &counted(None), at(25, 15)).unwrap();
    assert_eq!(counts(&store, 1).0, 2);
}

#[test]
fn reps_correction_lifts_empty_rule() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(0, Some(deadlift())), blank(1, Some(squat()))]);
    assert_eq!(counts(&store, 1), (1, 1));

    store.edit_sets(ID, &[SetRef::Recorded(1)], &SetChange { reps: Some(reps(8)), ..SetChange::default() }, at(25, 12)).unwrap();
    assert_eq!(counts(&store, 1), (2, 2), "the set now has reps, so it counts without any flag");
    assert_eq!(detail(&store, 1).corrections.get(&1).and_then(|c| c.counted), None);

    store.reset_sets(ID, &[1]).unwrap();
    assert_eq!(counts(&store, 1), (1, 1), "as the watch recorded it, it is empty again");

    // A weight lifts the rule just as reps do.
    store.edit_sets(ID, &[SetRef::Recorded(1)], &weight(20.0), at(25, 13)).unwrap();
    assert_eq!(counts(&store, 1), (2, 2));
}

#[test]
fn an_exercise_correction_changes_the_exercise_count() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(0, None), work(1, Some(unknown_exercise())), work(2, Some(deadlift()))]);
    assert_eq!(counts(&store, 1), (3, 1), "no exercise and the unknown category are not exercises");
    store.edit_sets(ID, &[SetRef::Recorded(0), SetRef::Recorded(1)], &SetChange { exercise: Some(curl()), ..SetChange::default() }, at(25, 12)).unwrap();
    assert_eq!(counts(&store, 1), (3, 2));
}

#[test]
fn added_set_lifecycle() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(0, Some(deadlift()))]);
    assert_eq!(counts(&store, 1), (1, 1));

    let id = store.add_set(ID, Some(SetRef::Recorded(0)), curl(), Some(kg(15.0)), Some(reps(12)), at(25, 12)).unwrap();
    assert!(id > 0);
    let expected = AddedSet { id, after: Some(SetRef::Recorded(0)), exercise: curl(), weight: Some(kg(15.0)), reps: Some(reps(12)), counted: None };
    assert_eq!(detail(&store, 1).added, [expected]);
    assert_eq!(counts(&store, 1), (2, 2));
    assert_eq!(text(&store, "added_set", "created_at", &format!("id = {id}")), Some(stored(at(25, 12))));

    // A second one, placed at the end, and a third after the first added set.
    let at_the_end = store.add_set(ID, None, squat(), None, None, at(25, 13)).unwrap();
    let chained = store.add_set(ID, Some(SetRef::Added(id)), squat(), None, None, at(25, 13)).unwrap();
    assert_ne!(at_the_end, chained);
    let added = detail(&store, 1).added;
    assert_eq!(
        added.iter().map(|set| (set.id, set.after)).collect::<Vec<_>>(),
        [(id, Some(SetRef::Recorded(0))), (at_the_end, None), (chained, Some(SetRef::Added(id)))]
    );
    assert_eq!(counts(&store, 1), (4, 3), "a set the user made counts although it has no reps and no weight");

    store.remove_added_set(ID, id, at(25, 14)).unwrap();
    let after = detail(&store, 1);
    assert_eq!(after.added.iter().map(|set| set.id).collect::<Vec<_>>(), [at_the_end, chained], "a removed set is not in the detail");
    assert_eq!(
        after.added.iter().map(|set| set.after).collect::<Vec<_>>(),
        [None, Some(SetRef::Recorded(0))],
        "the set that followed it now follows what it followed"
    );
    assert_eq!(counts(&store, 1), (3, 2));
    // The row is still there, marked: nothing is lost.
    assert_eq!(count_rows(&store, "added_set"), 3);
    assert_eq!(text(&store, "added_set", "deleted_at", &format!("id = {id}")), Some(stored(at(25, 14))));
    assert_eq!(text(&store, "added_set", "deleted_at", &format!("id = {at_the_end}")), None);

    // Removing it again changes nothing, not even the time it was removed at.
    store.remove_added_set(ID, id, at(25, 18)).unwrap();
    assert_eq!(text(&store, "added_set", "deleted_at", &format!("id = {id}")), Some(stored(at(25, 14))));
    assert_eq!(counts(&store, 1), (3, 2));
}

#[test]
fn added_sets_can_be_edited_and_switched_off() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(0, Some(deadlift()))]);
    let id = store.add_set(ID, None, squat(), None, None, at(25, 12)).unwrap();
    assert_eq!(counts(&store, 1), (2, 2));

    let change = SetChange { weight: Some(kg(100.0)), reps: Some(reps(3)), exercise: Some(curl()), counted: None };
    store.edit_sets(ID, &[SetRef::Added(id)], &change, at(25, 13)).unwrap();
    let set = detail(&store, 1).added[0];
    assert_eq!((set.exercise, set.weight, set.reps, set.counted), (curl(), Some(kg(100.0)), Some(reps(3)), None));

    // Unset fields stay; the exercise's name goes with its category.
    store
        .edit_sets(ID, &[SetRef::Added(id)], &SetChange { exercise: Some(ExerciseId { category: 21, name: None }), ..SetChange::default() }, at(25, 14))
        .unwrap();
    let set = detail(&store, 1).added[0];
    assert_eq!((set.exercise, set.weight), (ExerciseId { category: 21, name: None }, Some(kg(100.0))));

    store.edit_sets(ID, &[SetRef::Added(id)], &counted(Some(false)), at(25, 15)).unwrap();
    assert_eq!(detail(&store, 1).added[0].counted, Some(false));
    assert_eq!(counts(&store, 1), (1, 1));
    store.edit_sets(ID, &[SetRef::Added(id)], &counted(Some(true)), at(25, 16)).unwrap();
    assert_eq!(counts(&store, 1), (2, 2));
    // `Some(None)` is "by the rule", which for an added set is "counts".
    store.edit_sets(ID, &[SetRef::Added(id)], &counted(None), at(25, 17)).unwrap();
    assert_eq!(detail(&store, 1).added[0].counted, None);
    assert_eq!(counts(&store, 1), (2, 2));
}

/// Where an added set of activity 1 is placed, as the detail reports it.
fn after_of(store: &Store, added: i64) -> Option<SetRef> {
    detail(store, 1).added.iter().find(|set| set.id == added).unwrap().after
}

fn add_after(store: &Store, after: Option<SetRef>) -> i64 {
    store.add_set(ID, after, curl(), None, None, at(25, 12)).unwrap()
}

#[test]
fn removing_an_added_set_hands_its_followers_to_what_it_followed() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(3, Some(deadlift())), work(4, Some(deadlift()))]);

    // A follows nothing (it goes at the end), B and C follow A: when A goes, they follow nothing.
    let a = add_after(&store, None);
    let b = add_after(&store, Some(SetRef::Added(a)));
    let c = add_after(&store, Some(SetRef::Added(a)));
    let other = add_after(&store, Some(SetRef::Recorded(3)));
    store.remove_added_set(ID, a, at(25, 13)).unwrap();
    assert_eq!(after_of(&store, b), None);
    assert_eq!(after_of(&store, c), None);
    assert_eq!(after_of(&store, other), Some(SetRef::Recorded(3)), "a set that did not follow A is left alone");

    // Z, then A after Z, then B after A: A goes, B follows Z.
    let z = add_after(&store, None);
    let a = add_after(&store, Some(SetRef::Added(z)));
    let b = add_after(&store, Some(SetRef::Added(a)));
    store.remove_added_set(ID, a, at(25, 14)).unwrap();
    assert_eq!(after_of(&store, b), Some(SetRef::Added(z)));
    assert_eq!(after_of(&store, z), None);
}

#[test]
fn followers_of_a_removed_set_follow_its_recorded_set() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(3, Some(deadlift())), work(4, Some(deadlift()))]);
    let a = add_after(&store, Some(SetRef::Recorded(4)));
    let b = add_after(&store, Some(SetRef::Added(a)));
    store.remove_added_set(ID, a, at(25, 13)).unwrap();
    assert_eq!(after_of(&store, b), Some(SetRef::Recorded(4)));
    // Every set the detail returns is anchored to something it returns.
    let after = detail(&store, 1);
    assert_eq!(after.added.iter().map(|set| set.id).collect::<Vec<_>>(), [b]);
}

#[test]
fn a_chain_is_closed_up_whichever_link_goes_first() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(4, Some(deadlift()))]);
    // R <- A <- B <- C. Remove B, then A: C ends up after R.
    let a = add_after(&store, Some(SetRef::Recorded(4)));
    let b = add_after(&store, Some(SetRef::Added(a)));
    let c = add_after(&store, Some(SetRef::Added(b)));
    store.remove_added_set(ID, b, at(25, 13)).unwrap();
    assert_eq!(after_of(&store, c), Some(SetRef::Added(a)));
    store.remove_added_set(ID, a, at(25, 14)).unwrap();
    assert_eq!(after_of(&store, c), Some(SetRef::Recorded(4)));

    // The other order: remove A first, then B follows R and C follows B.
    let a = add_after(&store, Some(SetRef::Recorded(4)));
    let b = add_after(&store, Some(SetRef::Added(a)));
    let c = add_after(&store, Some(SetRef::Added(b)));
    store.remove_added_set(ID, a, at(25, 15)).unwrap();
    assert_eq!((after_of(&store, b), after_of(&store, c)), (Some(SetRef::Recorded(4)), Some(SetRef::Added(b))));
}

#[test]
fn removed_followers_keep_their_row_and_removing_twice_re_anchors_nothing() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(4, Some(deadlift()))]);
    let a = add_after(&store, Some(SetRef::Recorded(4)));
    let gone = add_after(&store, Some(SetRef::Added(a)));
    store.remove_added_set(ID, gone, at(25, 13)).unwrap();
    store.remove_added_set(ID, a, at(25, 14)).unwrap();
    // A removed set is history: it was not re-anchored when what it followed went too.
    assert_eq!(text(&store, "added_set", "CAST(after_value AS TEXT)", &format!("id = {gone}")), Some(a.to_string()));
    assert_eq!(text(&store, "added_set", "after_kind", &format!("id = {gone}")), Some("added".to_owned()));

    // Removing A again is an idempotent Ok: a live set found anchored to it (it cannot be made through the API) is not touched.
    store
        .conn
        .execute(
            "INSERT INTO added_set (activity_id, after_kind, after_value, exercise_category, created_at) VALUES (1, 'added', ?1, 7, '2026-10-25T12:00:00Z')",
            [a],
        )
        .unwrap();
    store.remove_added_set(ID, a, at(25, 18)).unwrap();
    let live: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM added_set WHERE deleted_at IS NULL AND after_kind = 'added' AND after_value = ?1", [a], |row| row.get(0))
        .unwrap();
    assert_eq!(live, 1);
    assert_eq!(text(&store, "added_set", "deleted_at", &format!("id = {a}")), Some(stored(at(25, 14))));
}

#[test]
fn removing_and_re_anchoring_are_one_transaction() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(4, Some(deadlift()))]);
    let a = add_after(&store, Some(SetRef::Recorded(4)));
    let b = add_after(&store, Some(SetRef::Added(a)));
    // The recount at the end of the removal fails on a damaged value, after the follower was re-anchored and A marked.
    store.conn.execute("UPDATE recorded_set SET reps = 5000", []).unwrap();
    let result = store.remove_added_set(ID, a, at(25, 13));
    assert!(matches!(result, Err(StoreError::Corrupt(_))), "{result:?}");
    assert_eq!(text(&store, "added_set", "deleted_at", &format!("id = {a}")), None, "A is not removed");
    assert_eq!(text(&store, "added_set", "after_kind", &format!("id = {b}")), Some("added".to_owned()), "B is not re-anchored");
    assert_eq!(text(&store, "added_set", "CAST(after_value AS TEXT)", &format!("id = {b}")), Some(a.to_string()));

    // Once the damage is gone the same call goes through, both changes together.
    store.conn.execute("UPDATE recorded_set SET reps = 8", []).unwrap();
    store.remove_added_set(ID, a, at(25, 13)).unwrap();
    assert_eq!(text(&store, "added_set", "deleted_at", &format!("id = {a}")), Some(stored(at(25, 13))));
    assert_eq!(after_of(&store, b), Some(SetRef::Recorded(4)));
}

#[test]
fn excluded_activity_counts_nothing() {
    let store = store_with(&[strength(1, 20), strength(2, 21)]);
    load(&store, 1, vec![work(0, Some(deadlift())), work(1, Some(squat()))]);
    load(&store, 2, vec![work(0, Some(deadlift()))]);
    let id = store.add_set(ID, None, curl(), None, None, at(25, 12)).unwrap();
    assert_eq!(counts(&store, 1), (3, 3));

    store.set_activity_excluded(ID, true).unwrap();
    assert_eq!(counts(&store, 1), (0, 0), "added sets and corrections count for nothing in an excluded activity");
    assert_eq!(counts(&store, 2), (1, 1), "another activity is untouched");
    store.edit_sets(ID, &[SetRef::Recorded(0)], &counted(Some(true)), at(25, 13)).unwrap();
    store.edit_sets(ID, &[SetRef::Added(id)], &weight(5.0), at(25, 13)).unwrap();
    assert_eq!(counts(&store, 1), (0, 0), "editing keeps it at zero");

    let mut filter = everything();
    filter.include_excluded = false;
    let listed: Vec<i64> = store.list_activities(&filter).unwrap().iter().map(|row| row.summary.id.0).collect();
    assert_eq!(listed, [2], "hidden by default");
    filter.include_excluded = true;
    let listed = store.list_activities(&filter).unwrap();
    assert_eq!(listed.iter().map(|row| (row.summary.id.0, row.excluded)).collect::<Vec<_>>(), [(2, false), (1, true)]);

    store.set_activity_excluded(ID, false).unwrap();
    assert_eq!(counts(&store, 1), (3, 3));
    // Setting the same value again is fine.
    store.set_activity_excluded(ID, false).unwrap();
    assert_eq!(counts(&store, 1), (3, 3));
}

#[test]
fn mark_seen_sets_the_time_once() {
    let store = store_with(&[crate::test_support::activity(1, "strength_training", at(20, 8), true)]);
    assert_eq!(text(&store, "activity", "seen_at", "id = 1"), None);
    store.mark_seen(ID, at(25, 12)).unwrap();
    assert_eq!(text(&store, "activity", "seen_at", "id = 1"), Some(stored(at(25, 12))));
    // Idempotent: the first time stays.
    store.mark_seen(ID, at(26, 12)).unwrap();
    assert_eq!(text(&store, "activity", "seen_at", "id = 1"), Some(stored(at(25, 12))));
    // An activity that was seen from the start is fine too.
    let seen = store_with(&[strength(2, 21)]);
    seen.mark_seen(ActivityId(2), at(26, 12)).unwrap();
    assert_eq!(text(&seen, "activity", "seen_at", "id = 2"), Some(stored(at(25, 10))));
}

#[test]
fn unknown_activities_and_sets_are_not_found_and_change_nothing() {
    let store = store_with(&[strength(1, 20), strength(2, 21)]);
    load(&store, 1, vec![work(0, Some(deadlift()))]);
    load(&store, 2, vec![work(0, Some(deadlift()))]);
    let other = store.add_set(ActivityId(2), None, squat(), None, None, at(25, 12)).unwrap();
    let removed = store.add_set(ID, None, squat(), None, None, at(25, 12)).unwrap();
    store.remove_added_set(ID, removed, at(25, 12)).unwrap();
    let ghost = ActivityId(99);
    let now = at(25, 13);

    // The activity does not exist.
    assert!(is_not_found(&store.edit_sets(ghost, &[], &weight(1.0), now)));
    assert!(is_not_found(&store.edit_sets(ghost, &[SetRef::Recorded(0)], &weight(1.0), now)));
    assert!(is_not_found(&store.reset_sets(ghost, &[0])));
    assert!(is_not_found(&store.add_set(ghost, None, squat(), None, None, now)));
    assert!(is_not_found(&store.remove_added_set(ghost, other, now)));
    assert!(is_not_found(&store.set_activity_excluded(ghost, true)));
    assert!(is_not_found(&store.mark_seen(ghost, now)));
    // The set does not exist in this activity.
    assert!(is_not_found(&store.edit_sets(ID, &[SetRef::Recorded(7)], &weight(1.0), now)));
    assert!(is_not_found(&store.edit_sets(ID, &[SetRef::Added(other)], &weight(1.0), now)), "another activity's added set");
    assert!(is_not_found(&store.edit_sets(ID, &[SetRef::Added(removed)], &weight(1.0), now)), "a removed added set");
    assert!(is_not_found(&store.reset_sets(ID, &[0, 7])));
    assert!(is_not_found(&store.add_set(ID, Some(SetRef::Recorded(7)), squat(), None, None, now)));
    assert!(is_not_found(&store.add_set(ID, Some(SetRef::Added(other)), squat(), None, None, now)));
    assert!(is_not_found(&store.add_set(ID, Some(SetRef::Added(removed)), squat(), None, None, now)));
    assert!(is_not_found(&store.add_set(ID, Some(SetRef::Added(12345)), squat(), None, None, now)));
    assert!(is_not_found(&store.remove_added_set(ID, other, now)), "another activity's added set");
    assert!(is_not_found(&store.remove_added_set(ID, 12345, now)));

    // Nothing was written by any of them.
    assert_eq!(count_rows(&store, "set_correction"), 0);
    assert_eq!(count_rows(&store, "added_set"), 2);
    assert_eq!(text(&store, "added_set", "deleted_at", &format!("id = {other}")), None);
    assert_eq!(counts(&store, 1), (1, 1));
    assert_eq!(counts(&store, 2), (2, 2));
}

#[test]
fn an_unstorable_now_changes_nothing() {
    let store = store_with(&[crate::test_support::activity(1, "strength_training", at(20, 8), true)]);
    load(&store, 1, vec![work(0, Some(deadlift()))]);
    let id = store.add_set(ID, None, squat(), None, None, at(25, 12)).unwrap();
    let never = DateTime::<Utc>::MAX_UTC;
    assert!(matches!(store.edit_sets(ID, &[SetRef::Recorded(0)], &weight(1.0), never), Err(StoreError::Sql(_))));
    assert!(matches!(store.add_set(ID, None, squat(), None, None, never), Err(StoreError::Sql(_))));
    assert!(matches!(store.remove_added_set(ID, id, never), Err(StoreError::Sql(_))));
    assert!(matches!(store.mark_seen(ID, never), Err(StoreError::Sql(_))));
    assert_eq!(count_rows(&store, "set_correction"), 0);
    assert_eq!(count_rows(&store, "added_set"), 1);
    assert_eq!(text(&store, "added_set", "deleted_at", &format!("id = {id}")), None);
    assert_eq!(text(&store, "activity", "seen_at", "id = 1"), None);
}

#[test]
fn reset_removes_only_the_named_sets() {
    let store = store_with(&[strength(1, 20), strength(2, 21)]);
    load(&store, 1, vec![work(0, Some(deadlift())), work(1, Some(deadlift())), work(2, Some(deadlift()))]);
    load(&store, 2, vec![work(0, Some(deadlift()))]);
    let all = [SetRef::Recorded(0), SetRef::Recorded(1), SetRef::Recorded(2)];
    store.edit_sets(ID, &all, &weight(70.0), at(25, 12)).unwrap();
    store.edit_sets(ActivityId(2), &[SetRef::Recorded(0)], &weight(70.0), at(25, 12)).unwrap();
    store.reset_sets(ID, &[0, 2]).unwrap();
    let keys: Vec<u16> = detail(&store, 1).corrections.keys().copied().collect();
    assert_eq!(keys, [1]);
    assert_eq!(detail(&store, 2).corrections.len(), 1, "another activity's correction of the same index stays");

    // A failing reset resets nothing.
    let result = store.reset_sets(ID, &[1, 9]);
    assert!(is_not_found(&result), "{result:?}");
    assert_eq!(detail(&store, 1).corrections.len(), 1);
}

#[test]
fn corrections_survive_a_reparse_and_the_counts_follow_the_new_sets() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(0, Some(deadlift())), blank(1, Some(deadlift()))]);
    store.edit_sets(ID, &[SetRef::Recorded(1)], &counted(Some(true)), at(25, 12)).unwrap();
    let id = store.add_set(ID, None, curl(), None, None, at(25, 12)).unwrap();
    assert_eq!(counts(&store, 1), (3, 2));

    // The file is read again with different sets: the set 1 is now an ordinary one and set 5 is new.
    load(&store, 1, vec![work(0, Some(deadlift())), work(1, Some(deadlift())), recorded(5, Some(1), Some(10.0), 30_000, Some(squat()))]);
    assert_eq!(count_rows(&store, "added_set"), 1, "a re-parse leaves the user's added sets alone");
    assert_eq!(count_rows(&store, "set_correction"), 1);
    let after = detail(&store, 1);
    assert_eq!(after.added.iter().map(|set| set.id).collect::<Vec<_>>(), [id]);
    assert_eq!(counts(&store, 1), (4, 3), "three recorded sets and the added one, over three exercises");
}

#[test]
fn a_correction_for_a_set_the_file_no_longer_has_is_kept_but_not_shown() {
    let store = store_with(&[strength(1, 20)]);
    load(&store, 1, vec![work(0, Some(deadlift())), work(1, Some(deadlift()))]);
    store.edit_sets(ID, &[SetRef::Recorded(1)], &weight(70.0), at(25, 12)).unwrap();
    load(&store, 1, vec![work(0, Some(deadlift()))]);
    assert_eq!(count_rows(&store, "set_correction"), 1, "kept in the database");
    assert!(detail(&store, 1).corrections.is_empty(), "not shown while there is no such set");
    // And it can neither be edited nor reset: the set is not there.
    assert!(is_not_found(&store.edit_sets(ID, &[SetRef::Recorded(1)], &weight(80.0), at(25, 13))));
    assert!(is_not_found(&store.reset_sets(ID, &[1])));
    // When a later file has the set again, the correction is back.
    load(&store, 1, vec![work(0, Some(deadlift())), work(1, Some(deadlift()))]);
    assert_eq!(detail(&store, 1).corrections.get(&1), Some(&SetCorrection { weight: Some(kg(70.0)), ..SetCorrection::default() }));
}
