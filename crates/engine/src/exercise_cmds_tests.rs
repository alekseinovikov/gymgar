//! Tests of `exercise.search`: matching by both names, the order (what the user did, then the rest of the catalog) and the
//! bounds of its params.

use std::collections::HashSet;

use chrono::{TimeZone, Utc};
use gymgar_domain::{ExerciseId, Mass, RecordedSet, Reps};
use gymgar_sync::exercises::{catalog, exercise_key, parse_exercise_key};
use serde_json::{Value, json};

use crate::activity_views::{ExerciseHitView, ExerciseSearchView};
use crate::testing::{seed_fixture_activities, seed_strength, test_session};
use crate::{Caller, EngineError, Outcome, Session};

fn search(session: &mut Session, params: Value) -> Vec<ExerciseHitView> {
    match session.execute("exercise.search", params.clone(), Caller::Agent) {
        Ok(Outcome::Done(value)) => serde_json::from_value::<ExerciseSearchView>(value).unwrap().items,
        other => panic!("exercise.search {params}: {other:?}"),
    }
}

fn ids(hits: &[ExerciseHitView]) -> Vec<&str> {
    hits.iter().map(|hit| hit.exercise.id.as_str()).collect()
}

fn set(index: u16, exercise: ExerciseId) -> RecordedSet {
    RecordedSet {
        message_index: index,
        start: Utc.with_ymd_and_hms(2026, 10, 21, 9, u32::from(index), 0).unwrap(),
        duration_ms: 30_000,
        reps: Some(Reps::new(10).unwrap()),
        weight: Some(Mass::from_kg(20.0).unwrap()),
        exercise: Some(exercise),
        step_index: None,
        plan: None,
        hr_avg: None,
        hr_max: None,
        rest_after_ms: None,
    }
}

fn key(key: &str) -> ExerciseId {
    parse_exercise_key(key).unwrap_or_else(|| panic!("{key} is not a key"))
}

#[test]
fn an_empty_query_lists_only_what_the_user_did_most_first() {
    let mut session = test_session();
    seed_fixture_activities(&session);
    let usage = session.store().exercise_usage().unwrap();
    assert!(!usage.is_empty());
    for query in ["", "   ", "\t"] {
        let hits = search(&mut session, json!({"query": query, "limit": 100}));
        assert_eq!(hits.len(), usage.len(), "{query:?}");
        assert!(hits.iter().all(|hit| hit.times_used > 0));
        assert!(hits.windows(2).all(|pair| pair[0].times_used >= pair[1].times_used), "most used first");
        let expected: HashSet<String> = usage.keys().map(|&id| exercise_key(id)).collect();
        assert_eq!(ids(&hits).into_iter().map(str::to_owned).collect::<HashSet<_>>(), expected);
        for hit in &hits {
            assert_eq!(usage.get(&key(&hit.exercise.id)), Some(&hit.times_used), "{}", hit.exercise.id);
        }
    }
    // No query at all is a bad one: the field is required.
    assert!(matches!(session.execute("exercise.search", json!({}), Caller::Agent), Err(EngineError::BadParams { .. })));
}

#[test]
fn used_exercises_come_first_then_the_rest_of_the_catalog() {
    let mut session = test_session();
    seed_fixture_activities(&session);
    let hits = search(&mut session, json!({"query": "deadlift", "limit": 100}));
    assert!(hits.len() > 3, "the catalog has several deadlifts: {:?}", ids(&hits));
    assert_eq!(hits[0].exercise.id, "deadlift/romanian_deadlift");
    assert!(hits[0].times_used > 0);
    assert_eq!(hits[0].exercise.english, "Romanian deadlift");
    assert!(hits[1..].iter().all(|hit| hit.times_used == 0));
    // The rest is in catalog order, and nothing twice.
    let unique: HashSet<&str> = ids(&hits).into_iter().collect();
    assert_eq!(unique.len(), hits.len());
    let position = |key: &str| catalog().iter().position(|entry| entry.key == key).unwrap();
    let rest: Vec<usize> = hits[1..].iter().map(|hit| position(&hit.exercise.id)).collect();
    assert!(rest.windows(2).all(|pair| pair[0] < pair[1]), "catalog order");
    // Every hit has the query in a name.
    for hit in &hits {
        assert!(
            hit.exercise.english.to_lowercase().contains("deadlift")
                || hit.exercise.watch_title.as_deref().is_some_and(|t| t.to_lowercase().contains("deadlift")),
            "{:?}",
            hit.exercise
        );
    }
}

#[test]
fn matching_ignores_case_and_looks_at_the_english_name() {
    let mut session = test_session();
    seed_fixture_activities(&session);
    let lower = search(&mut session, json!({"query": "romanian", "limit": 100}));
    let upper = search(&mut session, json!({"query": "ROMANIAN", "limit": 100}));
    let mixed = search(&mut session, json!({"query": "  RoManIaN  ", "limit": 100}));
    assert_eq!(lower, upper);
    assert_eq!(lower, mixed);
    assert!(ids(&lower).contains(&"deadlift/romanian_deadlift"));
    // It is a match on the name, not the key: the word is in "Romanian deadlift", not in "deadlift/romanian_deadlift" alone.
    assert!(search(&mut session, json!({"query": "romanian_deadlift"})).is_empty());
    assert!(search(&mut session, json!({"query": "no such exercise anywhere"})).is_empty());
}

#[test]
fn matching_looks_at_the_title_the_watch_gave_too() {
    let mut session = test_session();
    seed_fixture_activities(&session);
    let titles = session.store().exercise_titles().unwrap();
    let (&id, title) = titles
        .iter()
        .find(|(_, title)| title.chars().any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap_or(c))))
        .expect("a title in Russian in the fixture");
    // A piece of the title that the English name does not have, in another case than the watch wrote it.
    let piece: String = title.chars().take(5).collect();
    for query in [piece.to_uppercase(), piece.to_lowercase()] {
        let hits = search(&mut session, json!({"query": query, "limit": 100}));
        let hit = hits.iter().find(|hit| hit.exercise.id == exercise_key(id)).unwrap_or_else(|| panic!("{query:?} does not find {title:?}: {:?}", ids(&hits)));
        assert_eq!(hit.exercise.watch_title.as_deref(), Some(title.as_str()));
    }
    // The title also comes with the exercise when it is found by its English name.
    let hits = search(&mut session, json!({"query": exercise_key(id).split('/').next_back().unwrap_or("").replace('_', " "), "limit": 100}));
    assert!(hits.iter().any(|hit| hit.exercise.id == exercise_key(id) && hit.exercise.watch_title.is_some()));
}

#[test]
fn the_most_used_comes_first_and_only_counted_sets_are_counted() {
    let mut session = test_session();
    let squat = key("squat/barbell_back_squat");
    let curl = key("curl/incline_dumbbell_biceps_curl");
    let press = key("bench_press/barbell_bench_press");
    let mut sets = vec![];
    for index in 0..2 {
        sets.push(set(index, squat));
    }
    for index in 2..7 {
        sets.push(set(index, curl));
    }
    for index in 7..10 {
        sets.push(set(index, press));
    }
    // A set that does not count (empty) is not a use.
    let mut empty = set(10, press);
    empty.reps = None;
    empty.weight = None;
    empty.duration_ms = 2_000;
    sets.push(empty);
    let id = seed_strength(&session, sets);
    let hits = search(&mut session, json!({"query": ""}));
    assert_eq!(
        hits.iter().map(|hit| (hit.exercise.id.as_str(), hit.times_used)).collect::<Vec<_>>(),
        [("curl/incline_dumbbell_biceps_curl", 5), ("bench_press/barbell_bench_press", 3), ("squat/barbell_back_squat", 2),]
    );
    // Leaving the activity out takes its sets out of the counts.
    session.execute("activity.setExcluded", json!({"id": id.0, "excluded": true}), Caller::Agent).unwrap();
    assert!(search(&mut session, json!({"query": ""})).is_empty());
    // And the catalog is still there for a query.
    let found = search(&mut session, json!({"query": "incline dumbbell biceps"}));
    assert!(ids(&found).contains(&"curl/incline_dumbbell_biceps_curl"));
    assert!(found.iter().all(|hit| hit.times_used == 0));
}

#[test]
fn the_limit_defaults_to_20_and_is_at_most_100() {
    let mut session = test_session();
    let hits = search(&mut session, json!({"query": "a"}));
    assert_eq!(hits.len(), 20);
    assert_eq!(search(&mut session, json!({"query": "a", "limit": 3})).len(), 3);
    assert_eq!(search(&mut session, json!({"query": "a", "limit": 1})).len(), 1);
    assert_eq!(search(&mut session, json!({"query": "a", "limit": 100})).len(), 100);
    // The prefix of a longer answer is the shorter answer.
    let longer = search(&mut session, json!({"query": "a", "limit": 50}));
    assert_eq!(longer[..20], hits[..]);
}

#[test]
fn every_hit_is_a_key_the_commands_take() {
    let mut session = test_session();
    for hit in search(&mut session, json!({"query": "press", "limit": 100})) {
        let id = parse_exercise_key(&hit.exercise.id).unwrap_or_else(|| panic!("{} is not a key", hit.exercise.id));
        assert_eq!(exercise_key(id), hit.exercise.id);
        assert_ne!(id.category, ExerciseId::UNKNOWN_CATEGORY);
        assert!(!hit.exercise.english.is_empty());
    }
}

#[test]
fn bad_params_are_errors() {
    let mut session = test_session();
    let long_ascii = "x".repeat(101);
    let long_text = "é".repeat(101);
    for (what, params) in [
        ("a query of 101 characters", json!({"query": long_ascii})),
        ("a query of 101 characters in two bytes each", json!({"query": long_text})),
        ("a query that is not text", json!({"query": 5})),
        ("no query", json!({"limit": 5})),
        ("limit 0", json!({"query": "a", "limit": 0})),
        ("limit 101", json!({"query": "a", "limit": 101})),
        ("a negative limit", json!({"query": "a", "limit": -1})),
        ("a fractional limit", json!({"query": "a", "limit": 1.5})),
        ("an unknown key", json!({"query": "a", "offset": 1})),
        ("not an object", json!("a")),
    ] {
        match session.execute("exercise.search", params, Caller::Agent) {
            Err(EngineError::BadParams { cmd, .. }) => assert_eq!(cmd, "exercise.search", "{what}"),
            other => panic!("{what}: expected BadParams, got {other:?}"),
        }
    }
    // The longest query is counted in characters, not bytes: 100 two-byte characters are fine.
    assert!(search(&mut session, json!({"query": "é".repeat(100)})).is_empty());
    assert!(search(&mut session, json!({"query": "x".repeat(100)})).is_empty());
}
