//! Tests of `progress.list` and `progress.get` as an agent meets them: what the database holds comes back as the tiles of the
//! list and the chart of one exercise, the filters of the list, the keys `progress.get` takes, the bad params, and that a
//! correction of the user's changes the answer. The rules and the views have their own tests; these only check the wiring.
//!
//! "Now" is 2026-10-27 12:00 UTC in all of them. Two `#[ignore]` tests look at real or long data and do nothing unless run.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use gymgar_domain::{ActivityId, ExerciseId, RecordedSet};
use gymgar_store::Store;
use gymgar_sync::exercises::{catalog, exercise_key, is_strength, parse_exercise_key};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::specs;
use crate::testing::{TestConfig, seed_fixture_activities, seed_workout, strength_set, test_session_on, test_session_with};
use crate::{AdviceKind, Caller, EngineError, Outcome, PointView, ProgressItemView, ProgressListView, ProgressView, Session, WorkingView};

const DEADLIFT: &str = "deadlift/romanian_deadlift";
const SQUAT: &str = "squat/barbell_back_squat";
const CURL: &str = "curl/incline_dumbbell_biceps_curl";
const BENCH: &str = "bench_press/barbell_bench_press";
const PUSH_UP: &str = "push_up";

fn key(key: &str) -> ExerciseId {
    parse_exercise_key(key).unwrap_or_else(|| panic!("{key} is not a key"))
}

fn date(day: &str) -> NaiveDate {
    NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap()
}

fn session_now(now: chrono::DateTime<Utc>) -> Session {
    test_session_with(TestConfig { now, ..TestConfig::default() }).0
}

fn session() -> Session {
    session_now(Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap())
}

/// `count` sets of `exercise` from message index `first` on, all alike.
fn block(first: u16, exercise: &str, weight_kg: f64, reps: u32, plan_reps: Option<u32>, count: u16) -> Vec<RecordedSet> {
    (first..first + count).map(|index| strength_set(index, key(exercise), weight_kg, reps, plan_reps)).collect()
}

/// Three workouts (all begun at 07:55 where the user was, three hours ahead of UTC):
/// - 2026-08-03 (activity 1): bench press, 100 kg for 5, the plan asking 5;
/// - 2026-10-13 (activity 2) and 2026-10-20 (activity 3): Romanian deadlift 60 kg for 10 (plan 10) and back squat 80 kg for 8 (plan 10), three sets each;
/// - activity 3 also has an incline curl, 12.5 kg for 12 with no plan, and push-ups with a weight of zero, 15 reps.
///
/// So the list is: deadlift `add`; curl `noTarget`, push-up `bodyweight` and squat `hold` (in that order of the key: they were done at
/// the same moment); bench `break`.
fn seed_three_workouts(session: &Session) {
    let store = session.store();
    seed_workout(store, 1, "2026-08-03 07:55:00", 3, Some("Push"), block(0, BENCH, 100.0, 5, Some(5), 3));
    let legs = |first: u16| [block(first, DEADLIFT, 60.0, 10, Some(10), 3), block(first + 3, SQUAT, 80.0, 8, Some(10), 3)].concat();
    seed_workout(store, 2, "2026-10-13 07:55:00", 3, Some("Legs A"), legs(0));
    let mut third = legs(0);
    third.extend(block(6, CURL, 12.5, 12, None, 3));
    third.extend(block(9, PUSH_UP, 0.0, 15, None, 3));
    seed_workout(store, 3, "2026-10-20 07:55:00", 3, Some("Legs A"), third);
}

fn run<T: DeserializeOwned>(session: &mut Session, id: &str, params: Value) -> T {
    match session.execute(id, params.clone(), Caller::Agent) {
        Ok(Outcome::Done(value)) => serde_json::from_value(value).unwrap_or_else(|error| panic!("{id} {params} answered something else: {error}")),
        other => panic!("{id} {params}: {other:?}"),
    }
}

fn list(session: &mut Session, params: Value) -> Vec<ProgressItemView> {
    run::<ProgressListView>(session, "progress.list", params).items
}

fn get(session: &mut Session, exercise: &str) -> ProgressView {
    run(session, "progress.get", json!({ "exercise": exercise }))
}

fn keys(items: &[ProgressItemView]) -> Vec<&str> {
    items.iter().map(|item| item.exercise.id.as_str()).collect()
}

fn kinds(items: &[ProgressItemView]) -> Vec<AdviceKind> {
    items.iter().map(|item| item.advice.kind).collect()
}

fn working(weight_kg: f64, reps: &[u16], day: &str, activity: i64) -> WorkingView {
    WorkingView { weight_kg, reps: reps.to_vec(), date: date(day), activity_id: ActivityId(activity) }
}

#[test]
fn list_answers_what_the_store_holds() {
    let mut session = session();
    assert!(list(&mut session, json!({})).is_empty(), "no history, no tiles");
    seed_three_workouts(&session);

    let items = list(&mut session, json!({}));
    assert_eq!(keys(&items), [DEADLIFT, CURL, PUSH_UP, SQUAT, BENCH]);
    assert_eq!(
        kinds(&items),
        [AdviceKind::Add, AdviceKind::NoTarget, AdviceKind::Bodyweight, AdviceKind::Hold, AdviceKind::Break],
        "what to do first, then what is going on, then the pause"
    );
    let by_key = |wanted: &str| items.iter().find(|item| item.exercise.id == wanted).unwrap();

    let deadlift = by_key(DEADLIFT);
    assert_eq!(deadlift.exercise.english, "Romanian deadlift");
    assert_eq!(deadlift.advice.target, Some(10));
    assert_eq!(deadlift.advice.next_weight_kg, Some(65.0), "5 % above 60 kg, on the step of 2.5 kg");
    assert_eq!(deadlift.last, working(60.0, &[10, 10, 10], "2026-10-20", 3));
    assert_eq!((deadlift.sessions, deadlift.first_date, deadlift.dormant, deadlift.chart_reps), (2, date("2026-10-13"), false, Some(10)));
    assert_eq!(deadlift.sparkline.len(), 1, "one month");

    let squat = by_key(SQUAT);
    assert_eq!((squat.advice.target, squat.advice.sessions_at_weight, squat.advice.met_once), (Some(10), Some(2), false));
    assert_eq!(squat.last, working(80.0, &[8, 8, 8], "2026-10-20", 3));

    assert_eq!(by_key(PUSH_UP).last, working(0.0, &[15, 15, 15], "2026-10-20", 3));
    let bench = by_key(BENCH);
    assert_eq!((bench.advice.break_days, bench.sessions, bench.dormant), (Some(85), 1, false));

    // Asking again changes nothing.
    let again = list(&mut session, json!({}));
    assert_eq!(again, items);

    // A status filter keeps the order of the list.
    assert_eq!(keys(&list(&mut session, json!({ "status": ["add"] }))), [DEADLIFT]);
    assert_eq!(keys(&list(&mut session, json!({ "status": ["hold", "break"] }))), [SQUAT, BENCH]);
    assert_eq!(keys(&list(&mut session, json!({ "status": ["break", "break", "add"] }))), [DEADLIFT, BENCH], "a status named twice is the same as once");
    assert_eq!(keys(&list(&mut session, json!({ "status": ["noTarget", "bodyweight"] }))), [CURL, PUSH_UP]);
    assert_eq!(keys(&list(&mut session, json!({ "status": [] }))), keys(&items), "an empty list of statuses is no filter");
    assert!(list(&mut session, json!({ "status": ["hold"], "offset": 1 })).is_empty());

    // The page is cut after the filter: offset and limit count the tiles that are left.
    assert_eq!(keys(&list(&mut session, json!({ "limit": 2 }))), [DEADLIFT, CURL]);
    assert_eq!(keys(&list(&mut session, json!({ "offset": 2, "limit": 2 }))), [PUSH_UP, SQUAT]);
    assert_eq!(keys(&list(&mut session, json!({ "offset": 4, "limit": 1000 }))), [BENCH]);
    assert_eq!(keys(&list(&mut session, json!({ "status": ["hold", "noTarget", "bodyweight"], "offset": 1, "limit": 1 }))), [PUSH_UP]);
    assert_eq!(keys(&list(&mut session, json!({ "limit": 1 }))), [DEADLIFT]);
    // An offset at or past the end is an empty page, not an error.
    for offset in [5, 6, 1000, u32::MAX] {
        assert!(list(&mut session, json!({ "offset": offset })).is_empty(), "{offset}");
    }
    // 100 tiles by default: the limit is not needed for a short list.
    assert_eq!(list(&mut session, json!({ "offset": 0 })).len(), 5);
}

#[test]
fn get_answers_one_exercise() {
    let mut session = session();
    seed_three_workouts(&session);

    let deadlift = get(&mut session, DEADLIFT);
    assert_eq!(deadlift.exercise.id, DEADLIFT);
    assert_eq!(deadlift.exercise.english, "Romanian deadlift");
    // The tile of the list is the summary, whole.
    let tile = list(&mut session, json!({ "status": ["add"] })).remove(0);
    assert_eq!(deadlift.summary.as_ref(), Some(&tile));
    // What the advice rests on: the two sessions, the previous one first.
    let advice = &deadlift.summary.as_ref().unwrap().advice;
    assert_eq!(advice.kind, AdviceKind::Add);
    assert_eq!(advice.basis, [working(60.0, &[10, 10, 10], "2026-10-13", 2), working(60.0, &[10, 10, 10], "2026-10-20", 3)]);
    // The chart: October is one point, the first set of the earlier day that met the target (the earlier of two equal ones).
    assert_eq!(deadlift.step_kg, Some(2.5));
    assert_eq!(deadlift.points, [PointView { month: "2026-10".to_owned(), weight_kg: 60.0, reps: 10, date: date("2026-10-13"), activity_id: ActivityId(2) }]);
    assert!(deadlift.heavier.is_empty() && deadlift.gaps.is_empty());
    // The sessions, newest first, with the name of the program and every set of the day.
    assert_eq!(deadlift.sessions.iter().map(|session| session.date).collect::<Vec<_>>(), [date("2026-10-20"), date("2026-10-13")]);
    let newest = &deadlift.sessions[0];
    assert_eq!((newest.activity_id, newest.workout_name.as_deref()), (ActivityId(3), Some("Legs A")));
    assert_eq!(newest.start, Utc.with_ymd_and_hms(2026, 10, 20, 4, 55, 0).unwrap(), "07:55 three hours ahead of UTC");
    assert_eq!(newest.working, working(60.0, &[10, 10, 10], "2026-10-20", 3));
    assert_eq!(newest.sets.len(), 3);
    assert!(newest.sets.iter().all(|set| set.weight_kg == Some(60.0) && set.reps == Some(10)));

    // The bench press was last done a long time ago: a break, with the gap that is still going on.
    let bench = get(&mut session, BENCH);
    assert_eq!(bench.summary.as_ref().map(|tile| tile.advice.kind), Some(AdviceKind::Break));
    assert_eq!(bench.gaps.len(), 1);
    assert_eq!((bench.gaps[0].from, bench.gaps[0].to, bench.gaps[0].days), (date("2026-08-03"), None, 85));

    // An exercise of the catalog the user never did is a view with its names and nothing else.
    let used: HashSet<ExerciseId> = [DEADLIFT, SQUAT, CURL, BENCH, PUSH_UP].into_iter().map(key).collect();
    let untouched = catalog().iter().find(|entry| entry.id.name.is_some() && is_strength(entry.id) && !used.contains(&entry.id)).unwrap();
    for exercise in [untouched.key.as_str(), "lunge"] {
        let nothing = get(&mut session, exercise);
        assert_eq!(nothing.exercise.id, exercise);
        assert!(!nothing.exercise.english.is_empty());
        assert_eq!(nothing.summary, None, "{exercise}");
        assert_eq!(nothing.step_kg, None);
        assert!(nothing.points.is_empty() && nothing.heavier.is_empty() && nothing.gaps.is_empty() && nothing.sessions.is_empty(), "{exercise}");
    }
}

#[test]
fn a_key_the_catalog_does_not_name_is_accepted_when_the_user_did_it() {
    let mut session = session();
    let unnamed = ExerciseId { category: 8, name: Some(999) };
    assert_eq!(exercise_key(unnamed), "deadlift/999");
    assert!(!catalog().iter().any(|entry| entry.id == unnamed), "the catalog has no deadlift 999");
    // Not in the data and not in the catalog: nothing to answer about.
    assert!(matches!(session.execute("progress.get", json!({ "exercise": "deadlift/999" }), Caller::Agent), Err(EngineError::BadParams { .. })));
    seed_workout(
        session.store(),
        1,
        "2026-10-20 07:55:00",
        0,
        None,
        vec![strength_set(0, unnamed, 40.0, 8, Some(8)), strength_set(1, unnamed, 40.0, 8, Some(8))],
    );
    let view = get(&mut session, "deadlift/999");
    assert_eq!(view.exercise.id, "deadlift/999");
    assert_eq!(view.summary.as_ref().map(|tile| (tile.sessions, tile.last.weight_kg)), Some((1, 40.0)));
    assert_eq!(keys(&list(&mut session, json!({}))), ["deadlift/999"]);
    // Still a bad key next to it.
    assert!(matches!(session.execute("progress.get", json!({ "exercise": "deadlift/998" }), Caller::Agent), Err(EngineError::BadParams { .. })));
}

/// What an agent reads about the two commands has to match what the rule does: the 28 days of `add` and `break`, what the
/// points and the circles of `progress.get` are, and the chart of reps.
#[test]
fn the_descriptions_tell_the_rules_an_agent_depends_on() {
    let description = |id: &str| specs().into_iter().find(|spec| spec.id == id).map(|spec| spec.description).unwrap_or_default();
    let list = description("progress.list");
    for words in ["no more than 28 days apart", "more than 28 days ago", "takes precedence over every other status"] {
        assert!(list.contains(words), "progress.list should say {words:?}: {list}");
    }
    let get = description("progress.get");
    for words in [
        "chartReps",
        "most common reps",
        "fewer than chartReps reps",
        "also in months that have a point",
        "weightKg 0",
        "never in a chart of reps",
        "more than 28 days",
    ] {
        assert!(get.contains(words), "progress.get should say {words:?}: {get}");
    }
    assert!(!get.contains("only a heavier weight was lifted"), "{get}");
    assert!(!list.contains("four weeks") && !get.contains("four weeks"), "the length of a break is said in days");
}

#[test]
fn bad_params_are_errors_not_panics() {
    let mut session = session();
    seed_three_workouts(&session);
    let before = list(&mut session, json!({}));
    let long = "x".repeat(101);
    let long_text = "é".repeat(101);
    for (id, params) in [
        ("progress.get", json!({ "exercise": "nonsense/!!" })),
        ("progress.get", json!({ "exercise": 5 })),
        ("progress.get", json!({ "exercise": "" })),
        ("progress.get", json!({ "exercise": long })),
        ("progress.get", json!({ "exercise": long_text })),
        ("progress.get", json!({ "exercise": "deadlift/998" })),
        ("progress.get", json!({ "exercise": "c65534/3" })),
        ("progress.get", json!({ "exercise": null })),
        ("progress.get", json!({})),
        ("progress.get", json!(null)),
        ("progress.get", json!({ "exercise": DEADLIFT, "limit": 1 })),
        ("progress.get", json!([])),
        ("progress.get", json!("deadlift")),
        ("progress.list", json!({ "limit": 0 })),
        ("progress.list", json!({ "limit": 1001 })),
        ("progress.list", json!({ "limit": -1 })),
        ("progress.list", json!({ "limit": 1.5 })),
        ("progress.list", json!({ "limit": "5" })),
        ("progress.list", json!({ "offset": -1 })),
        ("progress.list", json!({ "offset": u64::from(u32::MAX) + 1 })),
        ("progress.list", json!({ "status": ["maybe"] })),
        ("progress.list", json!({ "status": ["Add"] })),
        ("progress.list", json!({ "status": "add" })),
        ("progress.list", json!({ "status": [null] })),
        ("progress.list", json!({ "unknown": 1 })),
        ("progress.list", json!({ "exercise": DEADLIFT })),
        ("progress.list", json!([])),
        ("progress.list", json!(7)),
    ] {
        match session.execute(id, params.clone(), Caller::Agent) {
            Err(EngineError::BadParams { cmd, .. }) => assert_eq!(cmd, id, "{params}"),
            other => panic!("{id} {params}: expected BadParams, got {other:?}"),
        }
    }
    // The bounds themselves are good, and nothing above changed a thing.
    assert_eq!(list(&mut session, json!({ "limit": 1 })).len(), 1);
    assert_eq!(list(&mut session, json!({ "limit": 1000 })).len(), 5);
    assert_eq!(list(&mut session, json!(null)), before);
    assert_eq!(list(&mut session, json!({})), before);
}

#[test]
fn a_correction_moves_sessions_between_exercises() {
    let mut session = session();
    let store = session.store();
    seed_workout(store, 1, "2026-10-13 07:55:00", 3, Some("Pull"), block(0, DEADLIFT, 60.0, 10, Some(10), 3));
    seed_workout(store, 2, "2026-10-20 07:55:00", 3, Some("Pull"), block(0, DEADLIFT, 60.0, 10, Some(10), 3));
    let all = || json!([{ "recorded": 0 }, { "recorded": 1 }, { "recorded": 2 }]);
    let status_of = |session: &mut Session, exercise: &str| get(session, exercise).summary.map(|tile| tile.advice.kind);

    assert_eq!(keys(&list(&mut session, json!({}))), [DEADLIFT]);
    assert_eq!(status_of(&mut session, DEADLIFT), Some(AdviceKind::Add));
    assert_eq!(get(&mut session, DEADLIFT).sessions.len(), 2);

    // A heavier weight in the latest session: the two are no longer at the same weight, and the weight is held.
    session.execute("activity.editSets", json!({ "id": 2, "sets": all(), "weightKg": 62.5 }), Caller::Agent).unwrap();
    let held = get(&mut session, DEADLIFT);
    let tile = held.summary.unwrap();
    assert_eq!(tile.advice.kind, AdviceKind::Hold, "{tile:?}");
    assert_eq!((tile.advice.sessions_at_weight, tile.advice.met_once), (Some(1), true));
    assert_eq!(tile.last.weight_kg, 62.5);
    assert_eq!(held.step_kg, Some(2.5), "the smallest difference between two working weights: 2.5 kg");
    // Taking the correction back is the way it was.
    session.execute("activity.resetSets", json!({ "id": 2, "sets": all() }), Caller::Agent).unwrap();
    assert_eq!(status_of(&mut session, DEADLIFT), Some(AdviceKind::Add));

    // All of the latest session's sets become another exercise: the session leaves the one and comes to the other.
    session.execute("activity.editSets", json!({ "id": 2, "sets": all(), "exercise": SQUAT }), Caller::Agent).unwrap();
    assert_eq!(keys(&list(&mut session, json!({}))), [SQUAT, DEADLIFT], "squat was done last");
    let deadlift = get(&mut session, DEADLIFT);
    assert_eq!(deadlift.sessions.iter().map(|session| session.activity_id).collect::<Vec<_>>(), [ActivityId(1)]);
    assert_eq!(deadlift.summary.as_ref().map(|tile| (tile.sessions, tile.advice.kind)), Some((1, AdviceKind::Hold)));
    let squat = get(&mut session, SQUAT);
    assert_eq!(squat.sessions.iter().map(|session| session.activity_id).collect::<Vec<_>>(), [ActivityId(2)]);
    assert_eq!(squat.summary.as_ref().map(|tile| (tile.sessions, tile.last.date)), Some((1, date("2026-10-20"))));

    // Leaving the activity out of the statistics takes its session away, and putting it back brings it back.
    session.execute("activity.setExcluded", json!({ "id": 2, "excluded": true }), Caller::Agent).unwrap();
    assert_eq!(keys(&list(&mut session, json!({}))), [DEADLIFT]);
    assert_eq!(get(&mut session, SQUAT).summary, None, "no session left: the exercise is still a key, with nothing in it");
    assert!(get(&mut session, SQUAT).sessions.is_empty());
    session.execute("activity.setExcluded", json!({ "id": 2, "excluded": false }), Caller::Agent).unwrap();
    assert_eq!(keys(&list(&mut session, json!({}))), [SQUAT, DEADLIFT]);

    // A set the user adds counts too, and joins the sets of its day.
    session.execute("activity.addSet", json!({ "id": 2, "exercise": SQUAT, "weightKg": 90.0, "reps": 5 }), Caller::Agent).unwrap();
    let squat = get(&mut session, SQUAT);
    assert_eq!(squat.sessions[0].sets.len(), 4);
    assert_eq!(squat.summary.unwrap().last.weight_kg, 90.0, "the added set is the heaviest of the day");
}

/// The keys of every object in the JSON, however deep.
fn object_keys<'a>(value: &'a Value, found: &mut Vec<&'a str>) {
    match value {
        Value::Object(map) => {
            for (name, inner) in map {
                found.push(name);
                object_keys(inner, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| object_keys(item, found)),
        _ => {}
    }
}

#[test]
fn answers_carry_no_raw_garmin_data() {
    let mut session = session();
    seed_fixture_activities(&session);
    seed_three_workouts(&session);
    let answers = [
        match session.execute("progress.list", json!({}), Caller::Agent) {
            Ok(Outcome::Done(value)) => value,
            other => panic!("{other:?}"),
        },
        match session.execute("progress.get", json!({ "exercise": DEADLIFT }), Caller::Agent) {
            Ok(Outcome::Done(value)) => value,
            other => panic!("{other:?}"),
        },
        match session.execute("progress.get", json!({ "exercise": "deadlift/romanian_deadlift" }), Caller::User) {
            Ok(Outcome::Done(value)) => value,
            other => panic!("{other:?}"),
        },
    ];
    // The fixture's deadlift and the seeded ones are one exercise: the answers are not empty.
    assert!(answers[1]["sessions"].as_array().is_some_and(|sessions| sessions.len() >= 3), "{}", answers[1]);
    for answer in &answers {
        let mut names = Vec::new();
        object_keys(answer, &mut names);
        for name in names {
            let lower = name.to_lowercase();
            for leaked in ["raw", "owner", "zip", "file"] {
                assert!(!lower.contains(leaked), "the key {name} of an answer looks like raw data");
            }
        }
        let text = answer.to_string();
        // Words only the raw JSON of Garmin has.
        for leaked in ["startTimeLocal", "activityType", "typeKey", "userProfile"] {
            assert!(!text.contains(leaked), "{leaked} leaked into an answer");
        }
    }
}

#[test]
fn a_day_is_the_day_where_the_user_was() {
    // 23:30 on the 20th in UTC is already the 21st for a user three hours ahead; the session is the 21st.
    let mut session = session();
    seed_workout(session.store(), 1, "2026-10-21 02:30:00", 3, None, block(0, DEADLIFT, 60.0, 10, Some(10), 3));
    let view = get(&mut session, DEADLIFT);
    assert_eq!(view.sessions.len(), 1);
    assert_eq!(view.sessions[0].date, date("2026-10-21"));
    assert_eq!(view.sessions[0].start, Utc.with_ymd_and_hms(2026, 10, 20, 23, 30, 0).unwrap());
    assert_eq!(view.points.iter().map(|point| point.month.as_str()).collect::<Vec<_>>(), ["2026-10"]);
}

#[test]
#[ignore = "reads a copy of the user's database: GYMGAR_PROGRESS_DB=/path/to/copy cargo test -p gymgar-engine -- --ignored progress_on_a_copy --nocapture"]
fn progress_on_a_copy_of_the_users_database() {
    // Without the variable there is nothing to read, and the test passes without doing anything.
    let Some(path) = std::env::var_os("GYMGAR_PROGRESS_DB") else {
        return;
    };
    // The copy is opened for writing (the store may migrate it), so it must be a copy; nothing is written anywhere else.
    let store = Store::open(Path::new(&path)).expect("the copy of the database opens");
    let (mut session, _handles) = test_session_on(store, TestConfig { now: Utc::now(), ..TestConfig::default() });
    let items = list(&mut session, json!({ "limit": 1000 }));
    println!("{:<44} {:>8} {:<16} {:<11} {:>6} {:>9}", "exercise", "kg", "reps", "status", "target", "next kg");
    for item in &items {
        let status = serde_json::to_value(item.advice.kind).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default();
        let reps = item.last.reps.iter().map(u16::to_string).collect::<Vec<_>>().join("/");
        let target = item.advice.target.map_or_else(|| "-".to_owned(), |target| target.to_string());
        let next = item.advice.next_weight_kg.map_or_else(|| "-".to_owned(), |next| next.to_string());
        let name: String = item.exercise.english.chars().take(44).collect();
        println!("{name:<44} {:>8} {reps:<16} {status:<11} {target:>6} {next:>9}", item.last.weight_kg);
    }
    println!("{} exercises", items.len());
}

#[test]
#[ignore = "timing: cargo test --release -p gymgar-engine -- --ignored progress_list_on_a_long_history --nocapture"]
fn progress_list_on_a_long_history() {
    let mut session = session();
    // Ten lifts the catalog names, eight of them in each of 300 workouts (every second day for 600 days), five sets of each: 40 sets.
    let lifts: Vec<ExerciseId> =
        catalog().iter().filter(|entry| entry.id.name.is_some() && is_strength(entry.id)).map(|entry| entry.id).step_by(7).take(10).collect();
    assert_eq!(lifts.len(), 10);
    let first_day = date("2025-03-01");
    let started = Instant::now();
    for workout in 0..300_u16 {
        let day = first_day + Duration::days(2 * i64::from(workout));
        let mut sets = Vec::new();
        for slot in 0..8_u16 {
            let lift = lifts[usize::from((workout + slot) % 10)];
            let weight = 20.0 + f64::from(slot) * 5.0 + f64::from(workout / 20) * 2.5;
            for set in 0..5_u16 {
                sets.push(strength_set(slot * 5 + set, lift, weight, 8 + u32::from(set % 2), Some(8)));
            }
        }
        assert_eq!(sets.len(), 40);
        seed_workout(session.store(), i64::from(workout) + 1, &format!("{day} 07:55:00"), 3, Some("Strength"), sets);
    }
    println!("seeded 300 workouts of 40 sets in {:?}", started.elapsed());

    let history = Instant::now();
    let sets = session.store().strength_history().expect("the history reads").len();
    let read = history.elapsed();
    let mut times = Vec::new();
    let mut tiles = 0;
    for _ in 0..7 {
        let started = Instant::now();
        tiles = list(&mut session, json!({})).len();
        times.push(started.elapsed());
    }
    times.sort();
    println!("{sets} counted sets, {tiles} tiles; reading the history alone {read:?}");
    println!("progress.list: fastest {:?}, median {:?}, slowest {:?}", times[0], times[times.len() / 2], times[times.len() - 1]);
    let started = Instant::now();
    let view = get(&mut session, &exercise_key(lifts[0]));
    println!("progress.get: {:?} ({} sessions, {} points)", started.elapsed(), view.sessions.len(), view.points.len());
    assert_eq!(tiles, 10);
}
