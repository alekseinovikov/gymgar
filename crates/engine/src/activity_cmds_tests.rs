//! Tests of the activity commands: the list and its filters, one activity in full, the "seen" mark, leaving an activity out,
//! and the corrections (edit, reset, add, remove). Sessions run on the fixtures (the scrubbed activity list and the file of its
//! strength workout "FB2") and on hand-made strength activities; nothing here touches the network.

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use gymgar_domain::{ActivityId, ActivitySummary, CountReason, ExerciseId, FileState, Mass, RecordedSet, Reps, SetRef};
use gymgar_store::{NewActivity, Store};
use gymgar_sync::exercises::exercise_key;
use serde_json::{Value, json};

use crate::activity_views::{ActivityListItemView, ActivityListView, ActivityView, SetView};
use crate::testing::{RecordingSleep, TestConfig, fixed_clock, seed_fixture_activities, seed_strength, test_session};
use crate::{Caller, EngineError, Outcome, Services, Session, commands};

pub(super) const ROMANIAN: &str = "deadlift/romanian_deadlift";
pub(super) const INCLINE_CURL: &str = "curl/incline_dumbbell_biceps_curl";
const ALL: [&str; 9] = [
    "activity.list",
    "activity.get",
    "activity.markSeen",
    "activity.setExcluded",
    "activity.editSets",
    "activity.resetSets",
    "activity.addSet",
    "activity.removeAddedSet",
    "exercise.search",
];

/// Runs `id` as an agent (the commands the tests use are open to agents, which is itself part of what is tested).
pub(super) fn run(session: &mut Session, id: &str, params: Value) -> Result<Value, EngineError> {
    match session.execute(id, params, Caller::Agent)? {
        Outcome::Done(value) => Ok(value),
        other => panic!("{id} did not finish at once: {other:?}"),
    }
}

pub(super) fn ok(session: &mut Session, id: &str, params: Value) -> Value {
    run(session, id, params.clone()).unwrap_or_else(|error| panic!("{id} {params}: {error}"))
}

pub(super) fn list(session: &mut Session, params: Value) -> Vec<ActivityListItemView> {
    serde_json::from_value::<ActivityListView>(ok(session, "activity.list", params)).unwrap().items
}

pub(super) fn get(session: &mut Session, id: ActivityId) -> ActivityView {
    serde_json::from_value(ok(session, "activity.get", json!({"id": id.0}))).unwrap()
}

/// Asserts `result` is `BadParams` of command `id`.
pub(super) fn assert_bad(result: Result<Value, EngineError>, id: &str, what: &str) {
    match result {
        Err(EngineError::BadParams { cmd, .. }) => assert_eq!(cmd, id, "{what}"),
        other => panic!("{id} {what}: expected BadParams, got {other:?}"),
    }
}

pub(super) fn sets(view: &ActivityView) -> Vec<&SetView> {
    view.groups.iter().flat_map(|group| group.sets.iter()).collect()
}

pub(super) fn refs(view: &ActivityView) -> Vec<SetRef> {
    sets(view).iter().map(|set| set.set).collect()
}

pub(super) fn find(view: &ActivityView, set: SetRef) -> SetView {
    sets(view).into_iter().find(|candidate| candidate.set == set).unwrap_or_else(|| panic!("{set:?} is not in the view")).clone()
}

pub(super) fn counted(view: &ActivityView) -> u32 {
    u32::try_from(sets(view).iter().filter(|set| set.counted).count()).unwrap()
}

pub(super) fn recorded(r: SetRef) -> Value {
    serde_json::to_value(r).unwrap()
}

/// The fixture session and the id of its strength workout (the one whose file is stored).
pub(super) fn fixture() -> (Session, ActivityId) {
    let mut session = test_session();
    seed_fixture_activities(&session);
    let strength = list(&mut session, json!({"strengthOnly": true})).into_iter().find(|item| item.file_state == FileState::Downloaded).unwrap().id;
    (session, strength)
}

pub(super) fn set_at(index: u16, reps: Option<u32>, weight_kg: Option<f64>, duration_ms: u32, exercise: Option<ExerciseId>) -> RecordedSet {
    RecordedSet {
        message_index: index,
        start: Utc.with_ymd_and_hms(2026, 10, 21, 9, u32::from(index), 0).unwrap(),
        duration_ms,
        reps: reps.map(|n| Reps::new(n).unwrap()),
        weight: weight_kg.map(|kg| Mass::from_kg(kg).unwrap()),
        exercise,
        step_index: None,
        plan: None,
        hr_avg: None,
        hr_max: None,
        rest_after_ms: None,
    }
}

pub(super) fn deadlift() -> ExerciseId {
    ExerciseId { category: 8, name: Some(23) }
}

/// A strength activity of 6 sets: two deadlifts, an empty one, a warm-up, and two with no exercise. 5 of them count at first.
pub(super) fn synthetic(session: &Session) -> ActivityId {
    seed_strength(
        session,
        vec![
            set_at(0, Some(10), Some(70.0), 30_000, Some(deadlift())),
            set_at(1, Some(10), Some(70.0), 30_000, Some(deadlift())),
            set_at(2, None, None, 2_000, Some(deadlift())),
            set_at(3, None, None, 278_350, Some(ExerciseId { category: 31, name: None })),
            set_at(4, Some(8), Some(20.0), 30_000, None),
            set_at(5, Some(8), Some(20.0), 30_000, None),
        ],
    )
}

/// An activity that is not strength, and not seen, started later than any fixture one.
pub(super) fn new_run(session: &Session, id: i64) -> ActivityId {
    let summary = ActivitySummary {
        id: ActivityId(id),
        type_key: "running".to_owned(),
        name: Some("Morning run".to_owned()),
        start: Utc.with_ymd_and_hms(2030, 1, 1, 7, 0, 0).unwrap(),
        duration_ms: Some(1_800_000),
        distance_m: Some(5_000.0),
        avg_hr: Some(150),
        calories: Some(300),
        manual: false,
    };
    session.store().upsert_activities(&[NewActivity { summary, raw_json: "{}".to_owned(), is_new: true }], Utc::now()).unwrap();
    ActivityId(id)
}

#[test]
fn every_command_is_always_available_and_takes_an_object_schema() {
    let session = test_session();
    for id in ALL {
        let spec = commands::find(id).unwrap_or_else(|| panic!("{id} is not registered"));
        assert_eq!((spec.enabled)(&session), Ok(()), "{id}");
        let schema = (spec.params_schema)();
        assert_eq!(schema.get("type"), Some(&json!("object")), "{id}");
        assert_eq!(schema.get("additionalProperties"), Some(&json!(false)), "{id}");
        assert!(!spec.destructive, "{id}");
        assert_eq!(spec.agent, id != "activity.markSeen", "{id}");
    }
}

#[test]
fn agent_flags() {
    let mut session = test_session();
    let fresh = new_run(&session, 9_001);
    // Closed to agents: the "new" mark is for the user's eyes. Open: everything else, which an agent can also read back.
    assert_eq!(session.execute("activity.markSeen", json!({"id": fresh.0}), Caller::Agent), Err(EngineError::AgentNotAllowed("activity.markSeen".to_owned())));
    for id in ALL.into_iter().filter(|id| *id != "activity.markSeen") {
        assert!(!matches!(session.execute(id, Value::Null, Caller::Agent), Err(EngineError::AgentNotAllowed(_))), "{id} is closed to agents");
    }
    // Reading never takes the mark off; only the user's markSeen does.
    assert_eq!(session.store().new_count(), Ok(1));
    assert!(session.execute("activity.markSeen", json!({"id": fresh.0}), Caller::User).is_ok());
    assert_eq!(session.store().new_count(), Ok(0));
}

#[test]
fn the_commands_work_in_a_session_without_services() {
    // `gymgar-cli` has no secret stores, no Garmin and no spawner; reading and correcting activities needs only the database.
    let services = Services {
        secrets: None,
        garmin: None,
        spawner: None,
        notify: Arc::new(|| {}),
        clock: fixed_clock(TestConfig::default().now),
        sleep: RecordingSleep::new().sleep_fn(),
    };
    let mut session = Session::new(Store::open_in_memory().unwrap(), services);
    seed_fixture_activities(&session);
    assert_eq!(list(&mut session, json!({})).len(), 8);
    let strength = list(&mut session, json!({"strengthOnly": true})).into_iter().find(|item| item.file_state == FileState::Downloaded).unwrap().id;
    let view = get(&mut session, strength);
    ok(&mut session, "activity.editSets", json!({"id": strength.0, "sets": [recorded(refs(&view)[0])], "reps": 3}));
    ok(&mut session, "activity.setExcluded", json!({"id": strength.0, "excluded": true}));
}

// ----- activity.list

#[test]
fn list_returns_the_activities_newest_first_with_their_summaries() {
    let (mut session, strength) = fixture();
    let items = list(&mut session, json!({}));
    assert_eq!(items.len(), 8);
    assert!(items.windows(2).all(|pair| pair[0].start >= pair[1].start), "newest first");
    let strengths: Vec<&ActivityListItemView> = items.iter().filter(|item| item.type_key == "strength_training").collect();
    assert_eq!(strengths.len(), 2);
    let line = items.iter().find(|item| item.id == strength).unwrap();
    assert_eq!(line.file_state, FileState::Downloaded);
    assert!(line.counted_sets > 0 && line.exercise_count > 0, "{line:?}");
    assert!(!line.is_new && !line.excluded);
    // Not strength: nothing to count, no file.
    for item in items.iter().filter(|item| item.type_key != "strength_training") {
        assert_eq!((item.counted_sets, item.exercise_count, item.file_state), (0, 0, FileState::NotNeeded), "{}", item.type_key);
    }
}

#[test]
fn list_filters_by_type_period_exercise_and_state() {
    let (mut session, strength) = fixture();
    let all = list(&mut session, json!({}));

    let types = list(&mut session, json!({"types": ["running", "cycling"]}));
    let expected: Vec<ActivityId> = all.iter().filter(|item| ["running", "cycling"].contains(&item.type_key.as_str())).map(|item| item.id).collect();
    assert!(!expected.is_empty());
    assert_eq!(types.iter().map(|item| item.id).collect::<Vec<_>>(), expected);
    assert!(list(&mut session, json!({"types": ["no_such_type"]})).is_empty());
    assert_eq!(list(&mut session, json!({"types": []})).len(), 8);

    let only_strength = list(&mut session, json!({"strengthOnly": true}));
    assert_eq!(only_strength.len(), 2);

    // The period: from is inclusive and to is exclusive; an offset other than UTC means the same moment.
    let from = all[5].start;
    let to = all[2].start;
    let plus_three = FixedOffset::east_opt(3 * 3600).unwrap();
    let period = list(&mut session, json!({"from": from.with_timezone(&plus_three).to_rfc3339(), "to": to.to_rfc3339()}));
    let expected: Vec<ActivityId> = all.iter().filter(|item| item.start >= from && item.start < to).map(|item| item.id).collect();
    assert!(!expected.is_empty());
    assert_eq!(period.iter().map(|item| item.id).collect::<Vec<_>>(), expected);
    assert_eq!(list(&mut session, json!({"from": to.to_rfc3339(), "to": to.to_rfc3339()})).len(), 0);

    // By exercise: the final one of a counted set.
    assert_eq!(list(&mut session, json!({"exercise": ROMANIAN})).iter().map(|item| item.id).collect::<Vec<_>>(), [strength]);
    assert!(list(&mut session, json!({"exercise": "squat/barbell_back_squat"})).is_empty());

    // Several conditions: every one holds.
    assert!(list(&mut session, json!({"types": ["running"], "exercise": ROMANIAN})).is_empty());
    assert_eq!(list(&mut session, json!({"types": ["strength_training"], "exercise": ROMANIAN, "strengthOnly": true})).len(), 1);
}

#[test]
fn list_pages_with_limit_and_offset() {
    let (mut session, _) = fixture();
    let all = list(&mut session, json!({}));
    let ids = |items: Vec<ActivityListItemView>| items.into_iter().map(|item| item.id).collect::<Vec<_>>();
    assert_eq!(ids(list(&mut session, json!({"limit": 3}))), ids(all[..3].to_vec()));
    assert_eq!(ids(list(&mut session, json!({"limit": 3, "offset": 3}))), ids(all[3..6].to_vec()));
    assert_eq!(ids(list(&mut session, json!({"offset": 6}))), ids(all[6..].to_vec()));
    assert!(list(&mut session, json!({"offset": 8})).is_empty());
    assert_eq!(list(&mut session, json!({"limit": 1000})).len(), 8);
}

#[test]
fn list_marks_new_and_excluded_and_can_hide_them() {
    let (mut session, strength) = fixture();
    let fresh = new_run(&session, 9_001);
    assert_eq!(list(&mut session, json!({"onlyNew": true})).iter().map(|item| item.id).collect::<Vec<_>>(), [fresh]);
    assert!(list(&mut session, json!({})).iter().find(|item| item.id == fresh).unwrap().is_new);

    // Excluded ones are listed unless asked otherwise, and are marked either way.
    ok(&mut session, "activity.setExcluded", json!({"id": strength.0, "excluded": true}));
    let shown = list(&mut session, json!({}));
    assert_eq!(shown.len(), 9);
    let line = shown.iter().find(|item| item.id == strength).unwrap();
    assert!(line.excluded);
    assert_eq!((line.counted_sets, line.exercise_count), (0, 0));
    assert_eq!(list(&mut session, json!({"includeExcluded": true})).len(), 9);
    let hidden = list(&mut session, json!({"includeExcluded": false}));
    assert_eq!(hidden.len(), 8);
    assert!(hidden.iter().all(|item| item.id != strength));
    // An excluded activity counts nothing, so it has no counted set of any exercise.
    assert!(list(&mut session, json!({"exercise": ROMANIAN})).is_empty());
}

#[test]
fn list_rejects_bad_params() {
    let (mut session, _) = fixture();
    let long_type = "x".repeat(65);
    let many_types: Vec<String> = (0..51).map(|n| format!("type{n}")).collect();
    let cases = [
        ("from after to", json!({"from": "2026-10-20T00:00:00Z", "to": "2026-10-19T00:00:00Z"})),
        ("not a date", json!({"from": "yesterday"})),
        ("a date without a time", json!({"from": "2026-10-20"})),
        ("a date without an offset", json!({"to": "2026-10-20T10:00:00"})),
        ("an empty date", json!({"from": ""})),
        ("year past 9999 after the offset", json!({"to": "9999-12-31T23:59:59-05:00"})),
        ("year before 0 after the offset", json!({"from": "0000-01-01T00:00:00+05:00"})),
        ("a date as a number", json!({"from": 5})),
        ("51 types", json!({"types": many_types})),
        ("a type of 65 characters", json!({"types": [long_type]})),
        ("a type that is not text", json!({"types": [5]})),
        ("types that is not a list", json!({"types": "running"})),
        ("an unknown exercise", json!({"exercise": "no such exercise"})),
        ("an empty exercise", json!({"exercise": ""})),
        ("an exercise key that is too long", json!({"exercise": "x".repeat(101)})),
        ("Garmin's unknown exercise", json!({"exercise": exercise_key(ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: None })})),
        ("limit 0", json!({"limit": 0})),
        ("limit 1001", json!({"limit": 1001})),
        ("a negative limit", json!({"limit": -1})),
        ("a negative offset", json!({"offset": -1})),
        ("a fractional limit", json!({"limit": 1.5})),
        ("an unknown key", json!({"strength": true})),
        ("a snake_case key", json!({"only_new": true})),
        ("a flag that is not a bool", json!({"onlyNew": "yes"})),
        ("not an object", json!([1, 2])),
    ];
    for (what, params) in cases {
        assert_bad(run(&mut session, "activity.list", params), "activity.list", what);
    }
    // The extreme valid ones are fine.
    for params in [
        json!({"from": "0000-01-01T00:00:00Z", "to": "9999-12-31T23:59:59Z"}),
        json!({"types": vec!["x".repeat(64); 50]}),
        json!({"exercise": "c65535/65535"}),
        Value::Null,
    ] {
        assert!(run(&mut session, "activity.list", params.clone()).is_ok(), "{params}");
    }
}

// ----- activity.get

#[test]
fn get_groups_in_first_appearance_order() {
    let (mut session, strength) = fixture();
    let view = get(&mut session, strength);
    assert_eq!(view.summary.id, strength);
    assert_eq!(view.summary.type_key, "strength_training");
    assert_eq!(view.summary.workout_name.as_deref(), Some("FB2"));
    let keys: Vec<Option<String>> = view.groups.iter().map(|group| group.exercise.as_ref().map(|exercise| exercise.id.clone())).collect();
    assert_eq!(keys[..3], [Some("cardio".to_owned()), Some("warm_up".to_owned()), Some(ROMANIAN.to_owned())]);
    let group = &view.groups[2];
    assert_eq!(group.exercise.as_ref().map(|exercise| exercise.english.as_str()), Some("Romanian deadlift"));
    assert_eq!(group.plan.map(|plan| (plan.reps, plan.weight_kg)), Some((Some(10), Some(70.0))));
    // The plan is said once, at the card.
    assert!(group.sets.iter().all(|set| set.plan.is_none()));
    // One card per exercise.
    let mut seen = std::collections::HashSet::new();
    assert!(view.groups.iter().all(|group| seen.insert(group.exercise.as_ref().map(|exercise| exercise.id.clone()))));
    // Every set of a freshly imported activity is as recorded.
    for set in sets(&view) {
        assert!(!set.added && set.correction.is_none(), "{:?}", set.set);
        assert_eq!(Some(&set.effective), set.recorded.as_ref());
        assert!(matches!(set.set, SetRef::Recorded(_)));
        assert!(set.start.is_some() && set.duration_ms.is_some());
    }
}

#[test]
fn counted_sets_in_get_equal_counted_sets_of_the_list() {
    let (mut session, strength) = fixture();
    let check = |session: &mut Session, id: ActivityId, what: &str| {
        let view = get(session, id);
        let item = list(session, json!({})).into_iter().find(|item| item.id == id).unwrap();
        assert_eq!(counted(&view), item.counted_sets, "{what}: counted sets");
        assert_eq!(view.summary, item, "{what}: the summary is the list line");
    };
    check(&mut session, strength, "the fixture");
    let view = get(&mut session, strength);
    // The fixture has sets that do not count by the rule, or the test proves little.
    assert!(sets(&view).iter().any(|set| set.count_reason == CountReason::Empty) || sets(&view).iter().all(|set| set.counted));

    let id = synthetic(&session);
    check(&mut session, id, "synthetic, as recorded");
    let view = get(&mut session, id);
    assert_eq!(counted(&view), 5);
    assert_eq!(find(&view, SetRef::Recorded(2)).count_reason, CountReason::Empty);

    ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 2}], "counted": true}));
    check(&mut session, id, "an empty set switched on");
    ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 0}, {"recorded": 3}], "counted": false}));
    check(&mut session, id, "two sets switched off");
    ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 2}], "reps": 5}));
    check(&mut session, id, "an empty set given reps");
    ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [{"recorded": 4}, {"recorded": 5}], "exercise": INCLINE_CURL}));
    check(&mut session, id, "exercises assigned");
    let added = ok(&mut session, "activity.addSet", json!({"id": id.0, "exercise": INCLINE_CURL, "weightKg": 12.5, "reps": 10}));
    let added_ref = *refs(&serde_json::from_value::<ActivityView>(added).unwrap()).iter().find(|r| matches!(r, SetRef::Added(_))).unwrap();
    check(&mut session, id, "a set added");
    let blank = ok(&mut session, "activity.addSet", json!({"id": id.0, "exercise": INCLINE_CURL}));
    // An added set with nothing in it still counts: the empty-set rule is not for sets the user made.
    let blank: ActivityView = serde_json::from_value(blank).unwrap();
    assert!(sets(&blank).iter().filter(|set| set.added).all(|set| set.counted));
    check(&mut session, id, "a blank set added");
    ok(&mut session, "activity.editSets", json!({"id": id.0, "sets": [recorded(added_ref)], "counted": false}));
    check(&mut session, id, "an added set switched off");
    ok(&mut session, "activity.setExcluded", json!({"id": id.0, "excluded": true}));
    check(&mut session, id, "the activity excluded");
    assert_eq!(counted(&get(&mut session, id)), 0);
    ok(&mut session, "activity.setExcluded", json!({"id": id.0, "excluded": false}));
    check(&mut session, id, "the activity back");
    ok(&mut session, "activity.removeAddedSet", json!({"id": id.0, "set": recorded(added_ref)}));
    check(&mut session, id, "an added set removed");
}

#[test]
fn get_does_not_mark_seen() {
    let mut session = test_session();
    let fresh = new_run(&session, 9_001);
    for _ in 0..2 {
        let view = get(&mut session, fresh);
        assert!(view.summary.is_new);
        assert!(view.groups.is_empty(), "a run has no sets");
    }
    assert!(list(&mut session, json!({"onlyNew": true})).iter().any(|item| item.id == fresh));
}

#[test]
fn get_of_an_activity_without_sets_is_a_summary_only() {
    let (mut session, _) = fixture();
    let run = list(&mut session, json!({"types": ["running"]}))[0].clone();
    let view = get(&mut session, run.id);
    assert_eq!(view.summary, run);
    assert!(view.groups.is_empty());
}

#[test]
fn get_rejects_bad_params() {
    let (mut session, _) = fixture();
    for (what, params) in [
        ("an unknown activity", json!({"id": 424_242})),
        ("id 0", json!({"id": 0})),
        ("a negative id", json!({"id": -3})),
        ("no id", json!({})),
        ("an id that is text", json!({"id": "7"})),
        ("a fractional id", json!({"id": 1.5})),
        ("an unknown key", json!({"id": 1, "x": 1})),
        ("no params", Value::Null),
    ] {
        assert_bad(run(&mut session, "activity.get", params), "activity.get", what);
    }
}

#[test]
fn responses_have_no_personal_fields() {
    let (mut session, _) = fixture();
    let mut texts =
        vec![ok(&mut session, "activity.list", json!({})).to_string(), ok(&mut session, "exercise.search", json!({"query": "", "limit": 100})).to_string()];
    for item in list(&mut session, json!({})) {
        texts.push(ok(&mut session, "activity.get", json!({"id": item.id.0})).to_string());
    }
    for text in texts {
        for forbidden in ["owner", "raw", "Aleksei", "s3.amazonaws.com", "profile", "https://", "uuid", "UUID"] {
            assert!(!text.contains(forbidden), "{forbidden:?} in {}", &text[..text.len().min(300)]);
        }
    }
}

// ----- activity.markSeen

#[test]
fn mark_seen_clears_new_and_answers_with_the_line() {
    let mut session = test_session();
    seed_fixture_activities(&session);
    let fresh = new_run(&session, 9_001);
    let Ok(Outcome::Done(value)) = session.execute("activity.markSeen", json!({"id": fresh.0}), Caller::User) else { panic!("markSeen failed") };
    let item: ActivityListItemView = serde_json::from_value(value).unwrap();
    assert_eq!(item.id, fresh);
    assert!(!item.is_new);
    assert!(list(&mut session, json!({"onlyNew": true})).is_empty());
    // Again: nothing changes, and it is not an error.
    assert!(session.execute("activity.markSeen", json!({"id": fresh.0}), Caller::User).is_ok());
    assert_eq!(session.store().new_count(), Ok(0));
}

#[test]
fn mark_seen_is_closed_to_agents_and_rejects_bad_params() {
    let mut session = test_session();
    let fresh = new_run(&session, 9_001);
    assert_eq!(session.execute("activity.markSeen", json!({"id": fresh.0}), Caller::Agent), Err(EngineError::AgentNotAllowed("activity.markSeen".to_owned())));
    assert_eq!(session.store().new_count(), Ok(1));
    for params in [json!({"id": 424_242}), json!({"id": 0}), json!({"id": -1}), json!({}), json!({"id": fresh.0, "x": 1}), json!("7")] {
        match session.execute("activity.markSeen", params.clone(), Caller::User) {
            Err(EngineError::BadParams { cmd, .. }) => assert_eq!(cmd, "activity.markSeen"),
            other => panic!("{params}: expected BadParams, got {other:?}"),
        }
    }
    assert_eq!(session.store().new_count(), Ok(1));
}

// ----- activity.setExcluded

#[test]
fn set_excluded_leaves_the_activity_out_and_takes_it_back() {
    let (mut session, strength) = fixture();
    let before = get(&mut session, strength);
    assert!(counted(&before) > 0);
    let excluded: ActivityView = serde_json::from_value(ok(&mut session, "activity.setExcluded", json!({"id": strength.0, "excluded": true}))).unwrap();
    assert!(excluded.summary.excluded);
    assert_eq!((excluded.summary.counted_sets, counted(&excluded)), (0, 0));
    assert!(sets(&excluded).iter().all(|set| set.count_reason == CountReason::ActivityExcluded));
    // The sets themselves are untouched.
    assert_eq!(refs(&excluded), refs(&before));
    let back: ActivityView = serde_json::from_value(ok(&mut session, "activity.setExcluded", json!({"id": strength.0, "excluded": false}))).unwrap();
    assert_eq!(back, before);
    // Any activity can be excluded, a run too.
    let run = list(&mut session, json!({"types": ["running"]}))[0].id;
    let view: ActivityView = serde_json::from_value(ok(&mut session, "activity.setExcluded", json!({"id": run.0, "excluded": true}))).unwrap();
    assert!(view.summary.excluded);
}

#[test]
fn set_excluded_rejects_bad_params() {
    let (mut session, _) = fixture();
    for (what, params) in [
        ("an unknown activity", json!({"id": 424_242, "excluded": true})),
        ("id 0", json!({"id": 0, "excluded": true})),
        ("a negative id", json!({"id": -1, "excluded": true})),
        ("no flag", json!({"id": 1})),
        ("a flag that is text", json!({"id": 1, "excluded": "yes"})),
        ("an unknown key", json!({"id": 1, "excluded": true, "x": 1})),
    ] {
        assert_bad(run(&mut session, "activity.setExcluded", params), "activity.setExcluded", what);
    }
}

// ----- shapes

#[test]
fn schemas_describe_the_params() {
    let schema = |id: &str| (commands::find(id).unwrap().params_schema)();
    let text = |id: &str| serde_json::to_string(&schema(id)).unwrap();
    // The flag of editSets may be a boolean or null, and the set refs are a closed choice of the two kinds.
    let edit = text("activity.editSets");
    assert!(edit.contains("\"counted\""), "{edit}");
    let counted = schema("activity.editSets").get("properties").and_then(|p| p.get("counted")).cloned().unwrap();
    assert!(counted.to_string().contains("null") && counted.to_string().contains("boolean"), "{counted}");
    assert_eq!(schema("activity.editSets").get("required"), Some(&json!(["id", "sets"])));
    for id in ["activity.editSets", "activity.resetSets"] {
        let sets = schema(id).get("properties").and_then(|p| p.get("sets")).cloned().unwrap();
        assert_eq!((sets.get("minItems"), sets.get("maxItems")), (Some(&json!(1)), Some(&json!(200))), "{id}: {sets}");
    }
    let reference = text("activity.removeAddedSet");
    assert!(reference.contains("recorded") && reference.contains("added"), "{reference}");
    assert_eq!(schema("activity.list").get("required"), None);
    assert_eq!(schema("activity.get").get("required"), Some(&json!(["id"])));
    assert_eq!(schema("exercise.search").get("required"), Some(&json!(["query"])));
}

#[test]
fn no_params_never_panic() {
    let (mut session, strength) = fixture();
    let hostile = [
        Value::Null,
        json!(true),
        json!([]),
        json!({"id": i64::MAX}),
        json!({"id": i64::MIN, "excluded": true}),
        json!({"id": strength.0, "sets": [{"recorded": 0}], "weightKg": 1e308}),
        json!({"id": strength.0, "sets": [{"recorded": 0}], "weightKg": 1e-320, "reps": 4_294_967_295_u64}),
        json!({"id": strength.0, "sets": [{"added": i64::MAX}], "reps": 1}),
        json!({"limit": u64::MAX}),
        json!({"offset": u32::MAX}),
        json!({"from": "9999-12-31T23:59:59.999999999+23:59"}),
        json!({"query": "\u{0}\u{1f600}é"}),
    ];
    for id in ALL {
        for params in &hostile {
            for caller in [Caller::User, Caller::Agent] {
                match session.execute(id, params.clone(), caller) {
                    Ok(_) | Err(EngineError::BadParams { .. } | EngineError::AgentNotAllowed(_)) => {}
                    other => panic!("{id} {params}: {other:?}"),
                }
            }
        }
    }
}

/// `chrono` offsets and years do not matter to the engine's time handling beyond what the parser checks.
#[test]
fn list_period_accepts_the_edges_of_the_stored_years() {
    let (mut session, _) = fixture();
    assert_eq!(list(&mut session, json!({"from": "0000-01-01T00:00:00Z"})).len(), 8);
    assert_eq!(list(&mut session, json!({"to": "9999-12-31T23:59:59Z"})).len(), 8);
    let after_all: DateTime<Utc> = Utc.with_ymd_and_hms(2100, 1, 1, 0, 0, 0).unwrap();
    assert!(list(&mut session, json!({"from": after_all.to_rfc3339()})).is_empty());
}
