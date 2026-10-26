//! Activities over MCP: what an agent sees of the imported Garmin activities and what it may do with them.
//!
//! The session holds the scrubbed fixture activities (no network, nothing personal), reached through the same in-process
//! client as the other server tests. The engine's own tests cover each command; these cover the way an agent meets them:
//! the command list, the JSON that comes back, the errors that are tool errors, and the commands that stay closed.

use gymgar_engine::Session;
use gymgar_engine::testing::{RequestKind, TestHandles, seed_fixture_activities};
use serde_json::{Value, json};

use super::*;

/// The exercise the strength fixture's sets are of, and one it has none of: the keys an agent gets from `exercise.search`.
const ROMANIAN: &str = "deadlift/romanian_deadlift";
const SQUAT: &str = "squat/barbell_back_squat";

/// The scrubbed fixture activities (eight of five types, two of them strength, one with its file read) in a session
/// that is connected to a fake Garmin, so a refused `garmin.sync` could be told from one that never ran.
fn activity_session() -> (Session, TestHandles) {
    let (session, handles) = connected_session();
    seed_fixture_activities(&session);
    // What the connection's own checks asked of the fake Garmin is not what a sync would ask.
    handles.session.take_requests();
    (session, handles)
}

/// A command that must succeed; its answer.
async fn run_ok(client: &Client, id: &str, params: Value) -> Value {
    let result = run(client, id, params.clone()).await;
    assert!(!is_error(&result), "{id} {params}: {}", text(&result));
    json_of(&result)
}

/// The error kind of a command that must fail as a tool error.
async fn run_err(client: &Client, id: &str, params: Value) -> String {
    let result = run(client, id, params.clone()).await;
    assert!(is_error(&result), "{id} {params} should fail: {}", text(&result));
    let body = json_of(&result);
    body["error"].as_str().unwrap_or_else(|| panic!("an error kind: {body}")).to_owned()
}

fn activity_ids(list: &Value) -> Vec<i64> {
    list["items"].as_array().expect("items").iter().map(|item| item["id"].as_i64().expect("an activity id")).collect()
}

/// The id of the strength activity whose sets were read (the other strength one still waits for its file).
async fn strength_with_sets(client: &Client) -> i64 {
    let list = run_ok(client, "activity.list", json!({ "strengthOnly": true })).await;
    let with_file: Vec<&Value> = list["items"].as_array().expect("items").iter().filter(|item| item["fileState"] == "downloaded").collect();
    assert_eq!(with_file.len(), 1, "{list}");
    with_file[0]["id"].as_i64().expect("an activity id")
}

/// Every set of an `activity.get` answer, in order.
fn sets_of(activity: &Value) -> Vec<&Value> {
    activity["groups"].as_array().expect("groups").iter().flat_map(|group| group["sets"].as_array().expect("sets")).collect()
}

// ----- activities over MCP: what an agent sees and may do

#[tokio::test]
async fn command_list_shows_activity_commands_but_not_sync_or_mark_seen() {
    let (session, _handles) = activity_session();
    let (client, _mcp) = start(session).await;
    let listed = listed_commands(&client, json!({})).await;
    let listed_ids = ids(&listed);
    // An agent reads the activities, finds exercise keys, and corrects sets.
    for visible in [
        "activity.list",
        "activity.get",
        "activity.setExcluded",
        "activity.editSets",
        "activity.resetSets",
        "activity.addSet",
        "activity.removeAddedSet",
        "exercise.search",
    ] {
        assert!(listed_ids.contains(&visible), "{visible} is missing from {listed_ids:?}");
    }
    // The sync and the re-parse reach Garmin and rewrite the user's data, and the "new" mark is for the user's eyes.
    for hidden in ["garmin.sync", "activity.reparse", "activity.markSeen"] {
        assert!(!listed_ids.contains(&hidden), "{hidden} must not be listed: {listed_ids:?}");
    }
    // Nothing an agent may run is marked destructive: removing a set only hides it.
    for command in listed.iter().filter(|command| command["id"].as_str().is_some_and(|id| id.starts_with("activity.") || id.starts_with("exercise."))) {
        assert_eq!(command["destructive"], false, "{command}");
        assert_eq!(command["enabled"], true, "{command}");
    }
    let by_id = |id: &str| listed.iter().find(|command| command["id"] == id).unwrap_or_else(|| panic!("{id} is listed"));
    assert_eq!(by_id("activity.list")["kind"], "query");
    assert_eq!(by_id("activity.get")["kind"], "query");
    assert_eq!(by_id("activity.editSets")["kind"], "mutation");
    // The schemas an agent builds its calls from: the filters of the list and the fields of an edit.
    let list_properties = &by_id("activity.list")["paramsSchema"]["properties"];
    for filter in ["from", "to", "types", "strengthOnly", "exercise", "onlyNew", "includeExcluded", "limit", "offset"] {
        assert!(list_properties.get(filter).is_some(), "activity.list has no {filter} param: {list_properties}");
    }
    let edit_schema = &by_id("activity.editSets")["paramsSchema"];
    for field in ["id", "sets", "weightKg", "reps", "exercise", "counted"] {
        assert!(edit_schema["properties"].get(field).is_some(), "activity.editSets has no {field} param: {edit_schema}");
    }
    assert_eq!(edit_schema["additionalProperties"], false);
    // The area prefix narrows the list to the activity commands, and still shows nothing hidden.
    let by_area = listed_commands(&client, json!({ "filter": "activity." })).await;
    assert!(ids(&by_area).iter().all(|id| id.starts_with("activity.")), "{by_area:?}");
    assert!(!ids(&by_area).contains(&"activity.markSeen") && !ids(&by_area).contains(&"activity.reparse"), "{by_area:?}");
    client.cancel().await.expect("the client closes");
}

#[tokio::test]
async fn command_run_activity_list_with_filters() {
    let (session, _handles) = activity_session();
    let (client, _mcp) = start(session).await;
    let listing = run(&client, "activity.list", json!({})).await;
    assert!(!is_error(&listing), "{}", text(&listing));
    // The answer is the views: camelCase summaries with the counts, and nothing Garmin sent verbatim.
    for leaked in ["rawJson", "raw_json", "owner", "userProfile"] {
        assert!(!text(&listing).contains(leaked), "{leaked} leaked into the list");
    }
    let all = json_of(&listing);
    let all_ids = activity_ids(&all);
    assert_eq!(all_ids.len(), 8, "{all}");
    let items = all["items"].as_array().expect("items");
    let first = items.first().expect("an activity");
    for field in ["id", "typeKey", "start", "isNew", "excluded", "fileState", "countedSets", "exerciseCount"] {
        assert!(first.get(field).is_some(), "an activity has no {field}: {first}");
    }

    // By exercise: an agent finds the key with exercise.search, and the list narrows to the activities that have it.
    let found = run_ok(&client, "exercise.search", json!({ "query": "romanian" })).await;
    let hit = found["items"].as_array().expect("items").first().expect("a hit").clone();
    assert_eq!(hit["exercise"]["id"], ROMANIAN, "{found}");
    assert!(hit["timesUsed"].as_u64().is_some_and(|times| times > 0), "{hit}");
    let strength = strength_with_sets(&client).await;
    let by_exercise = run_ok(&client, "activity.list", json!({ "exercise": ROMANIAN })).await;
    assert_eq!(activity_ids(&by_exercise), [strength], "{by_exercise}");
    assert!(activity_ids(&run_ok(&client, "activity.list", json!({ "exercise": SQUAT })).await).is_empty());

    // By period: `from` counts, `to` does not (RFC 3339 instants).
    let starts: Vec<&str> = items.iter().map(|item| item["start"].as_str().expect("a start")).collect();
    let (newest, third, sixth) = (starts[0], starts[2], starts[5]);
    let period = run_ok(&client, "activity.list", json!({ "from": sixth, "to": third })).await;
    let expected: Vec<i64> = items
        .iter()
        .filter(|item| item["start"].as_str().is_some_and(|start| start >= sixth && start < third))
        .map(|item| item["id"].as_i64().expect("an id"))
        .collect();
    assert!(!expected.is_empty() && expected.len() < all_ids.len(), "{expected:?}");
    assert_eq!(activity_ids(&period), expected, "{period}");
    assert_eq!(activity_ids(&run_ok(&client, "activity.list", json!({ "from": newest, "to": newest })).await), Vec::<i64>::new());
    // An instant with an offset other than Z is read as that moment: the epoch, then everything since.
    let shifted = run_ok(&client, "activity.list", json!({ "from": "1970-01-01T03:00:00+03:00", "to": "9999-12-31T23:59:59Z" })).await;
    assert_eq!(activity_ids(&shifted), all_ids);

    // By type, with a limit and a page.
    let strength_only = run_ok(&client, "activity.list", json!({ "strengthOnly": true })).await;
    assert_eq!(activity_ids(&strength_only).len(), 2, "{strength_only}");
    let by_type = run_ok(&client, "activity.list", json!({ "types": ["strength_training"], "exercise": ROMANIAN, "limit": 5 })).await;
    assert_eq!(activity_ids(&by_type), [strength]);
    assert_eq!(activity_ids(&run_ok(&client, "activity.list", json!({ "limit": 3, "offset": 2 })).await), all_ids[2..5]);
    client.cancel().await.expect("the client closes");
}

#[tokio::test]
async fn command_run_edit_sets_then_get_shows_correction() {
    let (session, _handles) = activity_session();
    let (client, _mcp) = start(session).await;
    let strength = strength_with_sets(&client).await;
    let before = run_ok(&client, "activity.get", json!({ "id": strength })).await;
    let before_sets = sets_of(&before);
    // The first counted set of the Romanian deadlift, as `activity.get` names it.
    let target = before_sets
        .iter()
        .find(|set| set["counted"] == true && set["effective"]["exercise"]["id"] == ROMANIAN)
        .map(|set| (*set).clone())
        .expect("a counted Romanian deadlift set in the fixture");
    assert_eq!(target["correction"], Value::Null, "nothing is corrected yet: {target}");
    let key = target["set"].clone();
    assert!(key.get("recorded").is_some(), "{key}");
    let counted_before = before["countedSets"].as_u64().expect("countedSets");

    // The answer of the edit is the activity as it is now, so the agent needs no second call to see it.
    let edited = run_ok(&client, "activity.editSets", json!({ "id": strength, "sets": [key], "weightKg": 77.5, "reps": 7 })).await;
    let after = run_ok(&client, "activity.get", json!({ "id": strength })).await;
    assert_eq!(edited, after);
    let corrected = sets_of(&after).into_iter().find(|set| set["set"] == key).expect("the set is still there").clone();
    // The correction is its own layer: the final values use it, and what the watch recorded stays.
    assert_eq!(corrected["correction"]["weightKg"], 77.5, "{corrected}");
    assert_eq!(corrected["correction"]["reps"], 7, "{corrected}");
    assert_eq!(corrected["effective"]["weightKg"], 77.5, "{corrected}");
    assert_eq!(corrected["effective"]["reps"], 7, "{corrected}");
    assert_eq!(corrected["recorded"], target["recorded"], "the watch's record never changes");
    assert_eq!(corrected["counted"], true);
    assert_eq!(corrected["effective"]["exercise"], target["effective"]["exercise"], "a field not given does not change");
    // Every other set is as it was, and the summary still counts the same sets.
    let others = |activity: &Value| sets_of(activity).into_iter().filter(|set| set["set"] != key).cloned().collect::<Vec<Value>>();
    assert_eq!(others(&after), others(&before));
    assert_eq!(after["countedSets"].as_u64(), Some(counted_before));
    // The list reads the same database.
    let listed = run_ok(&client, "activity.list", json!({ "exercise": ROMANIAN })).await;
    assert_eq!(activity_ids(&listed), [strength]);

    // Putting the set back drops the correction.
    run_ok(&client, "activity.resetSets", json!({ "id": strength, "sets": [key] })).await;
    let reset = run_ok(&client, "activity.get", json!({ "id": strength })).await;
    assert_eq!(reset, before);
    client.cancel().await.expect("the client closes");
}

#[tokio::test]
async fn an_agent_corrects_the_exercise_it_found_with_exercise_search() {
    let (session, _handles) = activity_session();
    let (client, _mcp) = start(session).await;
    let strength = strength_with_sets(&client).await;
    let activity = run_ok(&client, "activity.get", json!({ "id": strength })).await;
    let key =
        sets_of(&activity).into_iter().find(|set| set["counted"] == true && set["effective"]["exercise"]["id"] == ROMANIAN).expect("a set")["set"].clone();
    // Nothing in the activity is a squat yet; the agent finds the key by name and gives it to one set.
    assert!(activity_ids(&run_ok(&client, "activity.list", json!({ "exercise": SQUAT })).await).is_empty());
    let squat = run_ok(&client, "exercise.search", json!({ "query": "barbell back squat" })).await;
    assert!(squat["items"].as_array().expect("items").iter().any(|hit| hit["exercise"]["id"] == SQUAT), "{squat}");

    run_ok(&client, "activity.editSets", json!({ "id": strength, "sets": [key], "exercise": SQUAT })).await;
    // The activity has a counted squat now, and the list filters by what is final; the deadlifts it still has stay found.
    assert_eq!(activity_ids(&run_ok(&client, "activity.list", json!({ "exercise": SQUAT })).await), [strength]);
    assert_eq!(activity_ids(&run_ok(&client, "activity.list", json!({ "exercise": ROMANIAN })).await), [strength]);
    let after = run_ok(&client, "activity.get", json!({ "id": strength })).await;
    let moved = sets_of(&after).into_iter().find(|set| set["set"] == key).expect("the set").clone();
    assert_eq!(moved["effective"]["exercise"]["id"], SQUAT, "{moved}");
    assert_eq!(moved["correction"]["exercise"]["id"], SQUAT, "{moved}");
    assert_eq!(moved["recorded"]["exercise"]["id"], ROMANIAN, "the record keeps the exercise the watch knew: {moved}");
    client.cancel().await.expect("the client closes");
}

#[tokio::test]
async fn bad_set_ref_is_tool_error_not_crash() {
    let (session, _handles) = activity_session();
    let (client, _mcp) = start(session).await;
    let strength = strength_with_sets(&client).await;
    let before = run_ok(&client, "activity.get", json!({ "id": strength })).await;
    let good = sets_of(&before).first().expect("a set")["set"].clone();

    let bad_sets = [
        ("a recorded set the activity does not have", json!([{ "recorded": 9999 }])),
        ("a recorded index past u16", json!([{ "recorded": 70000 }])),
        ("a negative recorded index", json!([{ "recorded": -1 }])),
        ("a fractional recorded index", json!([{ "recorded": 1.5 }])),
        ("an added set that does not exist", json!([{ "added": 4242 }])),
        ("an added set with id zero", json!([{ "added": 0 }])),
        ("an added set with a negative id", json!([{ "added": -5 }])),
        ("an unknown kind of set", json!([{ "bogus": 1 }])),
        ("two kinds in one key", json!([{ "recorded": 0, "added": 1 }])),
        ("an empty key", json!([{}])),
        ("a bare number", json!([5])),
        ("a bare string", json!(["recorded"])),
        ("null", json!([null])),
        ("no sets", json!([])),
        ("sets that are not a list", json!({ "recorded": 0 })),
        ("a good set among bad ones", json!([good, { "recorded": 9999 }])),
    ];
    for (what, sets) in bad_sets {
        let kind = run_err(&client, "activity.editSets", json!({ "id": strength, "sets": sets, "reps": 3 })).await;
        assert_eq!(kind, "BadParams", "{what}");
    }
    // 201 sets is one too many.
    let too_many: Vec<Value> = (0..201).map(|index| json!({ "recorded": index })).collect();
    assert_eq!(run_err(&client, "activity.editSets", json!({ "id": strength, "sets": too_many, "reps": 3 })).await, "BadParams");
    // The other commands that take a set key refuse a bad one the same way.
    assert_eq!(run_err(&client, "activity.resetSets", json!({ "id": strength, "sets": [{ "recorded": 9999 }] })).await, "BadParams");
    assert_eq!(run_err(&client, "activity.removeAddedSet", json!({ "id": strength, "set": { "added": 4242 } })).await, "BadParams");
    assert_eq!(run_err(&client, "activity.addSet", json!({ "id": strength, "exercise": ROMANIAN, "after": { "recorded": 9999 } })).await, "BadParams");
    // So do bad values, and an activity that is not there.
    let bad_values = [
        json!({ "id": strength, "sets": [good], "weightKg": -1.0 }),
        json!({ "id": strength, "sets": [good], "weightKg": 1e9 }),
        json!({ "id": strength, "sets": [good], "reps": 100_000 }),
        json!({ "id": strength, "sets": [good], "reps": -1 }),
        json!({ "id": strength, "sets": [good], "exercise": "not/an_exercise" }),
        json!({ "id": strength, "sets": [good], "extra": true }),
        json!({ "id": 424_242, "sets": [good], "reps": 3 }),
        json!({ "id": 0, "sets": [good], "reps": 3 }),
        json!({ "sets": [good], "reps": 3 }),
    ];
    for params in bad_values {
        assert_eq!(run_err(&client, "activity.editSets", params.clone()).await, "BadParams", "{params}");
    }
    // None of it changed anything, and the server is still serving.
    assert_eq!(run_ok(&client, "activity.get", json!({ "id": strength })).await, before);
    assert!(!is_error(&run(&client, "activity.editSets", json!({ "id": strength, "sets": [good], "reps": 3 })).await));
    client.cancel().await.expect("the client closes");
}

#[tokio::test]
async fn garmin_sync_is_refused_for_agent() {
    let (session, handles) = activity_session();
    let (client, _mcp) = start(session).await;
    let before = run_ok(&client, "activity.list", json!({})).await;
    for params in [json!({}), json!({ "extra": 1 })] {
        assert_eq!(run_err(&client, "garmin.sync", params).await, "AgentNotAllowed");
    }
    // Refused before anything ran: Garmin was not asked, and the database is as it was.
    assert_eq!(handles.session.requests(), [], "a refused sync must not reach Garmin");
    assert_eq!(handles.session.count(RequestKind::Page) + handles.session.count(RequestKind::File), 0);
    assert_eq!(run_ok(&client, "activity.list", json!({})).await, before);
    let status = run_ok(&client, "garmin.status", json!({})).await;
    assert_eq!(status["status"], "connected");
    client.cancel().await.expect("the client closes");
}

#[tokio::test]
async fn reparse_and_mark_seen_are_refused_for_agent() {
    let (session, handles) = activity_session();
    let (client, _mcp) = start(session).await;
    let strength = strength_with_sets(&client).await;
    let before = run_ok(&client, "activity.get", json!({ "id": strength })).await;
    assert_eq!(run_err(&client, "activity.reparse", json!({})).await, "AgentNotAllowed");
    assert_eq!(run_err(&client, "activity.markSeen", json!({ "id": strength })).await, "AgentNotAllowed");
    assert_eq!(handles.session.requests(), []);
    assert_eq!(run_ok(&client, "activity.get", json!({ "id": strength })).await, before);
    client.cancel().await.expect("the client closes");
}
