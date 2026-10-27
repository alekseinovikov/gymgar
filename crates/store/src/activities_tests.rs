use chrono::{DateTime, TimeZone, Utc};
use gymgar_domain::{ActivityId, ActivitySummary, ExerciseId, FileState, Mass, PlannedTarget, RecordedSet, Reps, SetRef, StrengthRecord, SyncErrorKind};

use super::*;

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

fn summary(id: i64, type_key: &str, manual: bool, start: DateTime<Utc>) -> ActivitySummary {
    ActivitySummary {
        id: ActivityId(id),
        type_key: type_key.to_owned(),
        name: Some(format!("Activity {id}")),
        start,
        duration_ms: Some(3_600_000),
        distance_m: None,
        avg_hr: Some(120),
        calories: Some(300),
        manual,
    }
}

fn activity(id: i64, type_key: &str, start: DateTime<Utc>, is_new: bool) -> NewActivity {
    NewActivity { summary: summary(id, type_key, false, start), raw_json: format!("{{\"activityId\":{id}}}"), is_new }
}

fn strength(id: i64, day: u32) -> NewActivity {
    activity(id, "strength_training", at(day, 8), false)
}

fn deadlift() -> ExerciseId {
    ExerciseId { category: 8, name: Some(23) }
}

fn squat() -> ExerciseId {
    ExerciseId { category: 28, name: Some(1) }
}

fn bench() -> ExerciseId {
    ExerciseId { category: 0, name: Some(2) }
}

fn recorded(message_index: u16, reps: Option<u32>, kg: Option<f64>, duration_ms: u32, exercise: Option<ExerciseId>) -> RecordedSet {
    RecordedSet {
        message_index,
        start: at(20, 8) + chrono::Duration::seconds(i64::from(message_index) * 90),
        duration_ms,
        reps: reps.map(|n| Reps::new(n).unwrap()),
        weight: kg.map(|kg| Mass::from_kg(kg).unwrap()),
        exercise,
        step_index: None,
        plan: None,
        hr_avg: None,
        hr_max: None,
        rest_after_ms: None,
    }
}

/// An ordinary working set: 8 reps with 50 kg, 40 s long.
fn work(message_index: u16, exercise: Option<ExerciseId>) -> RecordedSet {
    recorded(message_index, Some(8), Some(50.0), 40_000, exercise)
}

/// What the watch writes for a set nobody did: no reps, no weight, over in two seconds.
fn blank(message_index: u16) -> RecordedSet {
    recorded(message_index, None, None, 2_000, Some(deadlift()))
}

fn record(sets: Vec<RecordedSet>) -> StrengthRecord {
    StrengthRecord { workout_name: Some("Legs".to_owned()), sets, titles: Vec::new() }
}

fn parsed(zip: Option<&str>, record: StrengthRecord, parser_version: u32) -> FileOutcome {
    FileOutcome::Parsed { zip: zip.map(|zip| zip.as_bytes().to_vec()), record, parser_version }
}

fn unreadable(zip: &str, parser_version: u32) -> FileOutcome {
    FileOutcome::Unreadable { zip: zip.as_bytes().to_vec(), parser_version }
}

fn parser_version(store: &Store, id: i64) -> Option<i64> {
    store.conn.query_row("SELECT parser_version FROM activity WHERE id = ?1", [id], |row| row.get(0)).unwrap()
}

fn file_state(store: &Store, id: i64) -> FileState {
    let text: String = store.conn.query_row("SELECT file_state FROM activity WHERE id = ?1", [id], |row| row.get(0)).unwrap();
    FileState::parse(&text).unwrap()
}

fn counts(store: &Store, id: i64) -> (i64, i64) {
    store.conn.query_row("SELECT counted_sets, exercise_count FROM activity WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
}

fn stored_zip(store: &Store, id: i64) -> Option<Vec<u8>> {
    store.conn.query_row("SELECT zip FROM activity_file WHERE activity_id = ?1", [id], |row| row.get(0)).optional().unwrap()
}

fn column_time(store: &Store, id: i64, column: &str) -> Option<DateTime<Utc>> {
    let text: Option<String> = store.conn.query_row(&format!("SELECT {column} FROM activity WHERE id = ?1"), [id], |row| row.get(0)).unwrap();
    text.map(|text| parse_time(column, &text).unwrap())
}

/// One row of integers (`NULL` as `None`).
fn integers(store: &Store, sql: &str) -> Vec<Option<i64>> {
    let mut statement = store.conn.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement.query_row([], |row| (0..columns).map(|index| row.get(index)).collect()).unwrap()
}

fn count_rows(store: &Store, table: &str) -> i64 {
    store.conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap()
}

fn recorded_indices(store: &Store, id: i64) -> Vec<i64> {
    let mut statement = store.conn.prepare("SELECT message_index FROM recorded_set WHERE activity_id = ?1 ORDER BY message_index").unwrap();
    statement.query_map([id], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

/// The fields of a correction row; `None` is "as recorded".
#[derive(Default)]
struct Fix {
    weight_g: Option<i64>,
    reps: Option<i64>,
    exercise: Option<ExerciseId>,
    counted: Option<bool>,
}

/// What `activity.editSets` will do later: a correction row with the given fields.
fn correct(store: &Store, id: i64, message_index: u16, fix: Fix) {
    store
        .conn
        .execute(
            "INSERT INTO set_correction (activity_id, message_index, weight_g, reps, exercise_category, exercise_name, counted, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '2026-10-25T12:00:00Z')",
            rusqlite::params![id, message_index, fix.weight_g, fix.reps, fix.exercise.map(|e| e.category), fix.exercise.and_then(|e| e.name), fix.counted],
        )
        .unwrap();
}

fn add_set(store: &Store, id: i64, exercise: (i64, Option<i64>), counted: Option<bool>, deleted: bool) {
    store
        .conn
        .execute(
            "INSERT INTO added_set (activity_id, exercise_category, exercise_name, counted, created_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, '2026-10-25T12:00:00Z', ?5)",
            rusqlite::params![id, exercise.0, exercise.1, counted, deleted.then_some("2026-10-25T13:00:00Z")],
        )
        .unwrap();
}

fn recount_now(store: &Store, id: i64) -> Result<(), StoreError> {
    let tx = store.write_tx()?;
    recount(&tx, ActivityId(id))?;
    tx.commit()?;
    Ok(())
}

fn store_with(items: &[NewActivity]) -> Store {
    let store = Store::open_in_memory().unwrap();
    store.upsert_activities(items, at(25, 10)).unwrap();
    store
}

#[test]
fn upsert_marks_and_keeps() {
    let store = Store::open_in_memory().unwrap();
    store.upsert_activities(&[activity(1, "strength_training", at(20, 8), true)], at(25, 10)).unwrap();
    assert_eq!(column_time(&store, 1, "seen_at"), None, "a new activity has not been seen");
    assert_eq!(column_time(&store, 1, "first_seen_at"), Some(at(25, 10)));
    assert_eq!(file_state(&store, 1), FileState::Pending);

    // The same activity again, renamed, no longer "new" in this page: the summary follows Garmin, the bookkeeping stays.
    let mut renamed = activity(1, "strength_training", at(20, 8), false);
    renamed.summary.name = Some("Renamed".to_owned());
    renamed.summary.calories = Some(999);
    renamed.raw_json = "{\"activityId\":1,\"changed\":true}".to_owned();
    store.conn.execute("UPDATE activity SET excluded = 1, file_state = 'downloaded' WHERE id = 1", []).unwrap();
    store.upsert_activities(&[renamed], at(26, 10)).unwrap();
    let (name, calories, raw): (String, i64, String) =
        store.conn.query_row("SELECT name, calories, raw_json FROM activity WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!((name.as_str(), calories, raw.as_str()), ("Renamed", 999, "{\"activityId\":1,\"changed\":true}"));
    assert_eq!(column_time(&store, 1, "seen_at"), None);
    assert_eq!(column_time(&store, 1, "first_seen_at"), Some(at(25, 10)));
    assert_eq!(file_state(&store, 1), FileState::Downloaded, "the file state is the download's, not the list's");
    let excluded: i64 = store.conn.query_row("SELECT excluded FROM activity WHERE id = 1", [], |row| row.get(0)).unwrap();
    assert_eq!(excluded, 1, "the user's exclusion survives a refresh of the list");
    assert_eq!(count_rows(&store, "activity"), 1);
}

#[test]
fn upsert_keeps_seen_at_once_set() {
    let store = Store::open_in_memory().unwrap();
    // Not new at insert: seen right away, at the time of the import.
    store.upsert_activities(&[activity(1, "running", at(20, 8), false)], at(25, 10)).unwrap();
    assert_eq!(column_time(&store, 1, "seen_at"), Some(at(25, 10)));
    // A later page that says "new" does not un-see it.
    store.upsert_activities(&[activity(1, "running", at(20, 8), true)], at(26, 10)).unwrap();
    assert_eq!(column_time(&store, 1, "seen_at"), Some(at(25, 10)));
}

#[test]
fn upsert_same_id_twice_in_one_page() {
    let store = Store::open_in_memory().unwrap();
    let mut second = strength(1, 20);
    second.summary.name = Some("Second".to_owned());
    store.upsert_activities(&[strength(1, 20), second, strength(2, 21)], at(25, 10)).unwrap();
    assert_eq!(count_rows(&store, "activity"), 2);
    let name: String = store.conn.query_row("SELECT name FROM activity WHERE id = 1", [], |row| row.get(0)).unwrap();
    assert_eq!(name, "Second", "the later duplicate wins");
}

#[test]
fn upsert_of_nothing_is_fine() {
    let store = Store::open_in_memory().unwrap();
    store.upsert_activities(&[], at(25, 10)).unwrap();
    assert_eq!(count_rows(&store, "activity"), 0);
}

#[test]
fn upsert_with_an_unstorable_value_writes_nothing() {
    let store = Store::open_in_memory().unwrap();
    let mut bad = strength(2, 21);
    // RFC 3339 cannot spell a five-digit year.
    bad.summary.start = DateTime::<Utc>::MAX_UTC;
    let result = store.upsert_activities(&[strength(1, 20), bad], at(25, 10));
    assert!(matches!(result, Err(StoreError::Sql(_))), "{result:?}");
    assert_eq!(count_rows(&store, "activity"), 0, "the page is all or nothing");

    let mut huge = strength(3, 22);
    huge.summary.duration_ms = Some(u64::MAX);
    assert!(matches!(store.upsert_activities(&[huge], at(25, 10)), Err(StoreError::Sql(_))));
    assert_eq!(count_rows(&store, "activity"), 0);
}

#[test]
fn upsert_stores_what_the_list_says() {
    let store = Store::open_in_memory().unwrap();
    let mut item = strength(1, 20);
    item.summary.distance_m = Some(1234.5);
    item.summary.name = None;
    item.summary.avg_hr = None;
    item.summary.manual = true;
    store.upsert_activities(&[item], at(25, 10)).unwrap();
    type Row = (String, Option<String>, String, Option<i64>, Option<f64>, Option<i64>, Option<i64>, bool);
    let row: Row = store
        .conn
        .query_row("SELECT type_key, name, start_at, duration_ms, distance_m, avg_hr, calories, manual FROM activity WHERE id = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
        })
        .unwrap();
    assert_eq!(row, ("strength_training".to_owned(), None, "2026-10-20T08:00:00.000000000Z".to_owned(), Some(3_600_000), Some(1234.5), None, Some(300), true));

    // A non-finite distance is not a distance.
    let mut odd = strength(2, 21);
    odd.summary.distance_m = Some(f64::INFINITY);
    store.upsert_activities(&[odd], at(25, 10)).unwrap();
    let distance: Option<f64> = store.conn.query_row("SELECT distance_m FROM activity WHERE id = 2", [], |row| row.get(0)).unwrap();
    assert_eq!(distance, None);
}

#[test]
fn file_states_on_insert() {
    let store = Store::open_in_memory().unwrap();
    let mut manual = strength(2, 21);
    manual.summary.manual = true;
    let mut manual_run = activity(4, "running", at(23, 8), false);
    manual_run.summary.manual = true;
    store.upsert_activities(&[strength(1, 20), manual, activity(3, "running", at(22, 8), false), manual_run], at(25, 10)).unwrap();
    assert_eq!(file_state(&store, 1), FileState::Pending);
    assert_eq!(file_state(&store, 2), FileState::Missing, "a manual entry has no file to wait for");
    assert_eq!(file_state(&store, 3), FileState::NotNeeded);
    assert_eq!(file_state(&store, 4), FileState::NotNeeded);
}

#[test]
fn snapshot_of_an_empty_database() {
    let store = Store::open_in_memory().unwrap();
    let snapshot = store.sync_snapshot().unwrap();
    assert!(snapshot.known.is_empty());
    assert!(!snapshot.history_complete);
    assert_eq!(snapshot.newest_start, None);
    assert!(snapshot.pending_files.is_empty());
}

#[test]
fn snapshot_lists_pending_newest_first() {
    let mut manual = strength(5, 25);
    manual.summary.manual = true;
    // Two files started at the same moment come out in a fixed order: the higher id first.
    let tie = activity(6, "strength_training", at(22, 8), false);
    let store = store_with(&[strength(1, 20), strength(2, 22), activity(3, "running", at(24, 8), false), strength(4, 21), manual, tie]);

    let snapshot = store.sync_snapshot().unwrap();
    let known: HashSet<ActivityId> = (1..=6).map(ActivityId).collect();
    assert_eq!(snapshot.known, known);
    assert_eq!(snapshot.newest_start, Some(at(25, 8)));
    assert_eq!(snapshot.pending_files, [6, 2, 4, 1].map(ActivityId));
    assert!(!snapshot.history_complete);

    store.apply_file(ActivityId(2), FileOutcome::Missing, at(25, 11)).unwrap();
    assert_eq!(store.sync_snapshot().unwrap().pending_files, [6, 4, 1].map(ActivityId));

    store.set_history_complete(true).unwrap();
    assert!(store.sync_snapshot().unwrap().history_complete);
}

#[test]
fn apply_parsed_stores_sets_and_counts_them() {
    let store = store_with(&[strength(1, 20)]);
    let mut set = work(0, Some(deadlift()));
    set.step_index = Some(4);
    set.plan = Some(PlannedTarget { reps: Some(Reps::new(10).unwrap()), weight: Some(Mass::from_kg(70.0).unwrap()) });
    set.hr_avg = Some(110);
    set.hr_max = Some(131);
    set.rest_after_ms = Some(60_000);
    let sets = vec![set, work(1, Some(deadlift())), work(2, Some(squat())), blank(3), work(4, None)];
    store.apply_file(ActivityId(1), parsed(Some("zip-1"), record(sets), 7), at(25, 11)).unwrap();

    assert_eq!(file_state(&store, 1), FileState::Downloaded);
    assert_eq!(stored_zip(&store, 1), Some(b"zip-1".to_vec()));
    assert_eq!(recorded_indices(&store, 1), [0, 1, 2, 3, 4]);
    // The blank set (3) does not count, the set without an exercise counts but is not an exercise.
    assert_eq!(counts(&store, 1), (4, 2));
    let (name, version): (String, i64) =
        store.conn.query_row("SELECT workout_name, parser_version FROM activity WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!((name.as_str(), version), ("Legs", 7));

    let columns =
        "duration_ms, reps, weight_g, exercise_category, exercise_name, step_index, has_plan, plan_reps, plan_weight_g, hr_avg, hr_max, rest_after_ms";
    let full = [Some(40_000), Some(8), Some(50_000), Some(8), Some(23), Some(4), Some(1), Some(10), Some(70_000), Some(110), Some(131), Some(60_000)];
    assert_eq!(integers(&store, &format!("SELECT {columns} FROM recorded_set WHERE message_index = 0")), full);
    let bare = [Some(40_000), Some(8), Some(50_000), Some(8), Some(23), None, Some(0), None, None, None, None, None];
    assert_eq!(integers(&store, &format!("SELECT {columns} FROM recorded_set WHERE message_index = 1")), bare);
    // "No exercise" is a NULL category, and a set with no weight keeps that as NULL rather than 0.
    assert_eq!(integers(&store, "SELECT exercise_category, exercise_name FROM recorded_set WHERE message_index = 4"), [None, None]);
    assert_eq!(integers(&store, "SELECT reps, weight_g FROM recorded_set WHERE message_index = 3"), [None, None]);
}

#[test]
fn apply_parsed_then_reparse_keeps_corrections() {
    let store = store_with(&[strength(1, 20)]);
    let first = vec![work(0, Some(deadlift())), work(1, Some(deadlift())), work(2, Some(deadlift()))];
    store.apply_file(ActivityId(1), parsed(Some("zip-1"), record(first), 1), at(25, 11)).unwrap();
    assert_eq!(counts(&store, 1), (3, 1));

    // The user says the second set did count, and corrects the third (which the next parse will no longer have).
    correct(&store, 1, 1, Fix { counted: Some(true), ..Fix::default() });
    correct(&store, 1, 2, Fix { weight_g: Some(60_000), ..Fix::default() });

    // The new parse finds different sets: the second is now blank, the third is gone, a fourth appeared.
    let again = vec![work(0, Some(deadlift())), blank(1), work(3, Some(deadlift()))];
    store.apply_file(ActivityId(1), parsed(None, record(again), 2), at(25, 11)).unwrap();

    assert_eq!(recorded_indices(&store, 1), [0, 1, 3], "the recorded sets are rebuilt as a whole");
    assert_eq!(count_rows(&store, "set_correction"), 2, "corrections are never touched, even for a set the file no longer has");
    assert_eq!(counts(&store, 1), (3, 1), "the blank set counts because the user said so");
    assert_eq!(stored_zip(&store, 1), Some(b"zip-1".to_vec()), "a re-parse does not lose the stored file");
    let version: i64 = store.conn.query_row("SELECT parser_version FROM activity WHERE id = 1", [], |row| row.get(0)).unwrap();
    assert_eq!(version, 2);

    // Without the user's say-so the blank set is blank again.
    store.conn.execute("DELETE FROM set_correction WHERE message_index = 1", []).unwrap();
    recount_now(&store, 1).unwrap();
    assert_eq!(counts(&store, 1), (2, 1));
}

/// Every column of every `added_set` row, as SQLite quotes it (so that NULL and 0 differ).
fn added_rows(store: &Store) -> Vec<Vec<String>> {
    let mut statement = store
        .conn
        .prepare(
            "SELECT quote(id), quote(activity_id), quote(after_kind), quote(after_value), quote(exercise_category), quote(exercise_name),
                    quote(weight_g), quote(reps), quote(counted), quote(created_at), quote(deleted_at)
             FROM added_set ORDER BY id",
        )
        .unwrap();
    statement.query_map([], |row| (0..11).map(|index| row.get::<_, String>(index)).collect::<Result<Vec<_>, _>>()).unwrap().collect::<Result<_, _>>().unwrap()
}

#[test]
fn reparse_leaves_added_sets_alone_and_counts_them() {
    let store = store_with(&[strength(1, 20)]);
    store.apply_file(ActivityId(1), parsed(Some("zip-1"), record(vec![work(0, Some(deadlift()))]), 1), at(25, 11)).unwrap();
    let kept =
        store.add_set(ActivityId(1), Some(SetRef::Recorded(0)), squat(), Some(Mass::from_kg(20.0).unwrap()), Some(Reps::new(10).unwrap()), at(25, 12)).unwrap();
    let removed = store.add_set(ActivityId(1), None, bench(), None, None, at(25, 12)).unwrap();
    store.remove_added_set(ActivityId(1), removed, at(25, 13)).unwrap();
    assert_eq!(counts(&store, 1), (2, 2));
    let before = added_rows(&store);
    assert_eq!(before.len(), 2);

    // The file is read again and now has other sets: the recorded sets are rebuilt, the added ones (and the removed one) stay as they were.
    let again = vec![work(0, Some(deadlift())), work(1, Some(deadlift())), blank(2)];
    store.apply_file(ActivityId(1), parsed(None, record(again), 2), at(25, 14)).unwrap();
    assert_eq!(recorded_indices(&store, 1), [0, 1, 2]);
    assert_eq!(added_rows(&store), before, "an added set is the user's, a re-parse does not rebuild it");
    assert_eq!(counts(&store, 1), (3, 2), "two recorded sets that count and the added squat; the removed bench is still out");
    let (live,): (i64,) =
        store.conn.query_row("SELECT COUNT(*) FROM added_set WHERE deleted_at IS NULL AND id = ?1", [kept], |row| Ok((row.get(0)?,))).unwrap();
    assert_eq!(live, 1);
}

#[test]
fn apply_parsed_replaces_the_stored_zip_only_when_given_one() {
    let store = store_with(&[strength(1, 20)]);
    store.apply_file(ActivityId(1), parsed(Some("old"), record(vec![work(0, Some(deadlift()))]), 1), at(25, 11)).unwrap();
    store.apply_file(ActivityId(1), parsed(Some("new"), record(vec![work(0, Some(deadlift()))]), 1), at(25, 11)).unwrap();
    assert_eq!(stored_zip(&store, 1), Some(b"new".to_vec()));
    assert_eq!(count_rows(&store, "activity_file"), 1);
}

#[test]
fn apply_unreadable_and_missing() {
    let store = store_with(&[strength(1, 20), strength(2, 21), strength(3, 22)]);
    store.apply_file(ActivityId(1), unreadable("not a fit", 1), at(25, 11)).unwrap();
    assert_eq!(file_state(&store, 1), FileState::Unreadable);
    assert_eq!(parser_version(&store, 1), Some(1), "the version that failed to read it is recorded");
    assert_eq!(stored_zip(&store, 1), Some(b"not a fit".to_vec()), "the bytes are kept so that a newer parser can try again");
    assert_eq!(count_rows(&store, "recorded_set"), 0);

    store.apply_file(ActivityId(2), FileOutcome::Missing, at(25, 11)).unwrap();
    assert_eq!(file_state(&store, 2), FileState::Missing);
    assert_eq!(stored_zip(&store, 2), None);
    assert_eq!(parser_version(&store, 2), None, "a missing file was never parsed");
    assert_eq!(file_state(&store, 3), FileState::Pending);

    // The unreadable file becomes readable with a later parser.
    store.apply_file(ActivityId(1), parsed(None, record(vec![work(0, Some(deadlift()))]), 2), at(25, 11)).unwrap();
    assert_eq!(file_state(&store, 1), FileState::Downloaded);
    assert_eq!(stored_zip(&store, 1), Some(b"not a fit".to_vec()));
    assert_eq!(counts(&store, 1), (1, 1));

    // And an unreadable result never touches the sets and corrections of an earlier good parse.
    correct(&store, 1, 0, Fix { weight_g: Some(55_000), ..Fix::default() });
    store.apply_file(ActivityId(1), unreadable("still bad", 3), at(25, 11)).unwrap();
    assert_eq!(file_state(&store, 1), FileState::Unreadable);
    assert_eq!(parser_version(&store, 1), Some(3));
    assert_eq!(recorded_indices(&store, 1), [0]);
    assert_eq!(count_rows(&store, "set_correction"), 1);
}

#[test]
fn apply_to_an_unknown_activity_is_not_found_and_changes_nothing() {
    let store = store_with(&[strength(1, 20)]);
    let mut with_titles = record(vec![work(0, Some(deadlift()))]);
    with_titles.titles = vec![(deadlift(), "Deadlift".to_owned())];
    for outcome in [parsed(Some("zip"), with_titles, 1), unreadable("zip", 1), FileOutcome::Missing] {
        let result = store.apply_file(ActivityId(99), outcome, at(25, 11));
        assert!(matches!(result, Err(StoreError::NotFound(_))), "{result:?}");
    }
    for table in ["recorded_set", "activity_file", "exercise_title"] {
        assert_eq!(count_rows(&store, table), 0, "{table}");
    }
    assert!(matches!(recount_now(&store, 99), Err(StoreError::NotFound(_))));
}

#[test]
fn a_failing_apply_changes_nothing() {
    let store = store_with(&[strength(1, 20)]);
    store.apply_file(ActivityId(1), parsed(Some("zip-1"), record(vec![work(0, Some(deadlift()))]), 1), at(25, 11)).unwrap();
    let mut bad = work(1, Some(deadlift()));
    // The second set cannot be stored: the whole file is refused and the first parse stays.
    bad.start = DateTime::<Utc>::MAX_UTC;
    let result = store.apply_file(ActivityId(1), parsed(Some("zip-2"), record(vec![work(0, Some(squat())), bad]), 2), at(25, 11));
    assert!(matches!(result, Err(StoreError::Sql(_))), "{result:?}");
    assert_eq!(recorded_indices(&store, 1), [0]);
    assert_eq!(stored_zip(&store, 1), Some(b"zip-1".to_vec()));
    let (category, version): (i64, i64) = store
        .conn
        .query_row("SELECT (SELECT exercise_category FROM recorded_set), parser_version FROM activity WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    assert_eq!((category, version), (8, 1));
}

#[test]
fn duplicate_message_indices_do_not_fail_the_file() {
    let store = store_with(&[strength(1, 20)]);
    let sets = vec![work(0, Some(deadlift())), recorded(0, Some(3), Some(10.0), 20_000, Some(squat()))];
    store.apply_file(ActivityId(1), parsed(Some("zip"), record(sets), 1), at(25, 11)).unwrap();
    assert_eq!(recorded_indices(&store, 1), [0]);
    let reps: i64 = store.conn.query_row("SELECT reps FROM recorded_set", [], |row| row.get(0)).unwrap();
    assert_eq!(reps, 3, "the later one wins");
}

#[test]
fn exercise_titles_are_upserted_and_the_last_one_wins() {
    let store = store_with(&[strength(1, 20), strength(2, 21)]);
    let warm_up = ExerciseId { category: 21, name: None };
    let mut first = record(vec![work(0, Some(deadlift()))]);
    // The same exercise twice in one file (two program steps name it) must not fail the transaction.
    first.titles =
        vec![(deadlift(), "Deadlift".to_owned()), (squat(), "Squat".to_owned()), (deadlift(), "Становая тяга".to_owned()), (warm_up, "Warm-up".to_owned())];
    store.apply_file(ActivityId(1), parsed(Some("zip"), first, 1), at(25, 11)).unwrap();
    assert_eq!(count_rows(&store, "exercise_title"), 3);
    let seen_at = |category: i64, name: i64| -> DateTime<Utc> {
        let text: String =
            store.conn.query_row("SELECT seen_at FROM exercise_title WHERE category = ?1 AND name = ?2", [category, name], |row| row.get(0)).unwrap();
        parse_time("seen_at", &text).unwrap()
    };
    let title = |category: i64, name: i64| -> String {
        store.conn.query_row("SELECT title FROM exercise_title WHERE category = ?1 AND name = ?2", [category, name], |row| row.get(0)).unwrap()
    };
    assert_eq!(title(8, 23), "Становая тяга");
    assert_eq!(title(28, 1), "Squat");
    assert_eq!(title(21, -1), "Warm-up", "a missing exercise number is stored as -1 so that the key works");

    // A later file renames one exercise and keeps the others.
    let mut second = record(vec![work(0, Some(squat()))]);
    second.titles = vec![(squat(), "Приседания".to_owned())];
    store.apply_file(ActivityId(2), parsed(Some("zip"), second, 1), at(26, 9)).unwrap();
    assert_eq!(count_rows(&store, "exercise_title"), 3);
    assert_eq!(title(28, 1), "Приседания");
    assert_eq!(title(8, 23), "Становая тяга");
    // `seen_at` is the time the caller gave, per title: renamed squat moved on, the others stayed.
    assert_eq!(seen_at(28, 1), at(26, 9));
    assert_eq!(seen_at(8, 23), at(25, 11));
    assert_eq!(seen_at(21, -1), at(25, 11));
}

#[test]
fn an_unstorable_now_changes_nothing() {
    let store = store_with(&[strength(1, 20)]);
    let outcome = parsed(Some("zip"), record(vec![work(0, Some(deadlift()))]), 1);
    assert!(matches!(store.apply_file(ActivityId(1), outcome, DateTime::<Utc>::MAX_UTC), Err(StoreError::Sql(_))));
    assert_eq!(file_state(&store, 1), FileState::Pending);
    assert_eq!(count_rows(&store, "recorded_set"), 0);
}

#[test]
fn counts_use_the_effective_values_of_each_set() {
    let store = store_with(&[strength(1, 20)]);
    let sets = vec![
        blank(0),                                                  // blank, stays blank
        blank(1),                                                  // blank, the user gives it reps
        work(2, Some(deadlift())),                                 // counted, the user excludes it
        recorded(3, Some(8), Some(50.0), 5_000, Some(deadlift())), // counted, the user zeroes reps and weight: a short set with neither is blank
        recorded(4, None, None, 278_350, Some(deadlift())),        // a long warm-up counts without reps and weight
        recorded(5, Some(0), Some(0.0), 10_000, None),             // exactly at the threshold: not blank
        recorded(6, Some(0), Some(0.0), 9_999, None),              // just below it: blank
        blank(7),                                                  // blank, the user gives it a weight
    ];
    store.apply_file(ActivityId(1), parsed(Some("zip"), record(sets), 1), at(25, 11)).unwrap();
    // Counted so far: sets 2, 3, 4 and 5.
    assert_eq!(counts(&store, 1), (4, 1));

    // One correction at a time, so that each field of the rule is seen on its own.
    let step = |message_index: u16, fix: Fix, expected: i64| {
        correct(&store, 1, message_index, fix);
        recount_now(&store, 1).unwrap();
        assert_eq!(counts(&store, 1), (expected, 1), "after correcting set {message_index}");
    };
    step(1, Fix { reps: Some(5), ..Fix::default() }, 5);
    step(7, Fix { weight_g: Some(1_000), ..Fix::default() }, 6);
    step(2, Fix { counted: Some(false), ..Fix::default() }, 5);
    // Zeroing both reps and weight of a short set makes it blank; either one alone would not.
    step(3, Fix { weight_g: Some(0), reps: Some(0), ..Fix::default() }, 4);
}

#[test]
fn exercise_count_is_the_distinct_effective_exercises_of_counted_sets() {
    let store = store_with(&[strength(1, 20)]);
    let sets = vec![
        work(0, Some(deadlift())),
        work(1, Some(deadlift())),
        work(2, Some(squat())),                                                           // the user turns it into a bench press
        work(3, None),                                                                    // counts as a set, is no exercise
        work(4, Some(ExerciseId { category: 0, name: Some(9) })),                         // excluded by the user: its exercise does not count
        work(5, None),                                                                    // the user names it a squat
        work(6, Some(ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: None })), // "unknown" is no exercise either
    ];
    store.apply_file(ActivityId(1), parsed(Some("zip"), record(sets), 1), at(25, 11)).unwrap();
    correct(&store, 1, 2, Fix { exercise: Some(bench()), ..Fix::default() });
    correct(&store, 1, 4, Fix { counted: Some(false), ..Fix::default() });
    correct(&store, 1, 5, Fix { exercise: Some(squat()), ..Fix::default() });
    recount_now(&store, 1).unwrap();
    // Seven sets, one excluded. Exercises: deadlift, bench (set 2), squat (set 5).
    assert_eq!(counts(&store, 1), (6, 3));
}

#[test]
fn added_sets_count_unless_removed_or_excluded() {
    let store = store_with(&[strength(1, 20)]);
    store.apply_file(ActivityId(1), parsed(Some("zip"), record(vec![work(0, Some(deadlift()))]), 1), at(25, 11)).unwrap();
    assert_eq!(counts(&store, 1), (1, 1));

    add_set(&store, 1, (28, Some(1)), None, false); // no flag: counts (a NULL flag is not "excluded")
    add_set(&store, 1, (28, Some(1)), Some(true), false); // counts
    add_set(&store, 1, (5, Some(3)), Some(false), false); // excluded by the user
    add_set(&store, 1, (6, None), None, true); // removed
    add_set(&store, 1, (7, None), None, false); // a second new exercise
    recount_now(&store, 1).unwrap();
    assert_eq!(counts(&store, 1), (4, 3), "deadlift, squat and the exercise without a number; the removed and excluded sets are out");

    // An excluded activity counts nothing, added sets included.
    store.conn.execute("UPDATE activity SET excluded = 1 WHERE id = 1", []).unwrap();
    recount_now(&store, 1).unwrap();
    assert_eq!(counts(&store, 1), (0, 0));
    store.conn.execute("UPDATE activity SET excluded = 0 WHERE id = 1", []).unwrap();
    recount_now(&store, 1).unwrap();
    assert_eq!(counts(&store, 1), (4, 3));
}

#[test]
fn added_set_ids_are_positive_and_assigned_by_sqlite() {
    let store = store_with(&[strength(1, 20)]);
    add_set(&store, 1, (8, None), None, false);
    add_set(&store, 1, (8, None), None, false);
    let ids: Vec<i64> =
        store.conn.prepare("SELECT id FROM added_set ORDER BY id").unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(ids, [1, 2]);
    let result = store.conn.execute("INSERT INTO added_set (id, activity_id, exercise_category, created_at) VALUES (-5, 1, 8, '2026-10-25T12:00:00Z')", []);
    assert!(result.is_err(), "a non-positive id is refused by the schema");
}

#[test]
fn recount_runs_inside_the_callers_transaction() {
    let store = store_with(&[strength(1, 20)]);
    store.apply_file(ActivityId(1), parsed(Some("zip"), record(vec![work(0, Some(deadlift())), blank(1)]), 1), at(25, 11)).unwrap();
    assert_eq!(counts(&store, 1), (1, 1));

    // A correction and its recount belong to one transaction: rolled back together.
    {
        let tx = store.write_tx().unwrap();
        tx.execute("INSERT INTO set_correction (activity_id, message_index, counted, updated_at) VALUES (1, 1, 1, '2026-10-25T12:00:00Z')", []).unwrap();
        recount(&tx, ActivityId(1)).unwrap();
        assert_eq!(counts_in(&tx, 1), (2, 1));
    }
    assert_eq!(counts(&store, 1), (1, 1));
    assert_eq!(count_rows(&store, "set_correction"), 0);
}

fn counts_in(conn: &Connection, id: i64) -> (i64, i64) {
    conn.query_row("SELECT counted_sets, exercise_count FROM activity WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
}

#[test]
fn recount_reports_unusable_stored_values() {
    let store = store_with(&[strength(1, 20)]);
    store.apply_file(ActivityId(1), parsed(Some("zip"), record(vec![work(0, Some(deadlift()))]), 1), at(25, 11)).unwrap();
    for (column, value) in [
        ("reps", "5000"),
        ("reps", "-1"),
        ("weight_g", "-5"),
        ("weight_g", "2000000"),
        ("exercise_category", "70000"),
        ("duration_ms", "-1"),
        ("reps", "'many'"),
    ] {
        store.conn.execute(&format!("UPDATE recorded_set SET {column} = {value}"), []).unwrap();
        let result = recount_now(&store, 1);
        assert!(matches!(result, Err(StoreError::Corrupt(_))), "{column} = {value}: {result:?}");
        // Put the row back: re-applying rebuilds the sets whatever was in them.
        store.apply_file(ActivityId(1), parsed(None, record(vec![work(0, Some(deadlift()))]), 1), at(25, 11)).unwrap();
    }
}

#[test]
fn reparse_lists_only_old_versions() {
    let mut manual = strength(6, 25);
    manual.summary.manual = true;
    let store =
        store_with(&[strength(1, 20), strength(2, 21), strength(3, 22), strength(4, 23), strength(5, 24), manual, activity(7, "running", at(26, 8), false)]);
    let one = || record(vec![work(0, Some(deadlift()))]);
    store.apply_file(ActivityId(1), parsed(Some("zip-1"), one(), 1), at(25, 11)).unwrap();
    store.apply_file(ActivityId(2), parsed(Some("zip-2"), one(), 2), at(25, 11)).unwrap();
    store.apply_file(ActivityId(3), parsed(Some("zip-3"), one(), 3), at(25, 11)).unwrap();
    store.apply_file(ActivityId(4), unreadable("zip-4", 1), at(25, 11)).unwrap();
    // 5 is still pending, 6 has no file, 7 needs none.
    store.apply_file(ActivityId(6), FileOutcome::Missing, at(25, 11)).unwrap();

    let listed = |version: u32| -> Vec<(i64, Vec<u8>)> { store.files_to_reparse(version).unwrap().into_iter().map(|(id, zip)| (id.0, zip)).collect() };
    assert_eq!(listed(0), []);
    assert_eq!(listed(1), []);
    assert_eq!(listed(2), [(4, b"zip-4".to_vec()), (1, b"zip-1".to_vec())], "newest first");
    assert_eq!(listed(3), [(4, b"zip-4".to_vec()), (2, b"zip-2".to_vec()), (1, b"zip-1".to_vec())]);
    assert_eq!(listed(4).len(), 4);
}

#[test]
fn an_unreadable_file_is_retried_only_by_a_newer_parser() {
    let store = store_with(&[strength(1, 20)]);
    store.apply_file(ActivityId(1), unreadable("broken", 1), at(25, 11)).unwrap();
    assert_eq!(store.files_to_reparse(1).unwrap(), [], "version 1 already failed on it");
    assert_eq!(store.files_to_reparse(2).unwrap(), [(ActivityId(1), b"broken".to_vec())]);

    // Still unreadable for version 2: it is not offered to version 2 again, only to version 3.
    store.apply_file(ActivityId(1), unreadable("broken", 2), at(25, 11)).unwrap();
    assert_eq!(store.files_to_reparse(2).unwrap(), []);
    assert_eq!(store.files_to_reparse(3).unwrap().len(), 1);
}

#[test]
fn a_row_without_a_parser_version_is_not_listed_for_reparse() {
    let store = store_with(&[strength(1, 20)]);
    store.apply_file(ActivityId(1), parsed(Some("zip"), record(vec![work(0, Some(deadlift()))]), 1), at(25, 11)).unwrap();
    store.conn.execute("UPDATE activity SET parser_version = NULL", []).unwrap();
    assert_eq!(store.files_to_reparse(9).unwrap(), []);
}

#[test]
fn sync_state_round_trip() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.sync_state().unwrap(), SyncState { last_success_at: None, last_error: None, history_complete: false });

    store.record_sync(Some(at(25, 10)), None).unwrap();
    assert_eq!(store.sync_state().unwrap(), SyncState { last_success_at: Some(at(25, 10)), last_error: None, history_complete: false });

    // A failure keeps the time of the last success and says what went wrong.
    store.record_sync(None, Some(SyncErrorKind::RateLimited)).unwrap();
    assert_eq!(
        store.sync_state().unwrap(),
        SyncState { last_success_at: Some(at(25, 10)), last_error: Some(SyncErrorKind::RateLimited), history_complete: false }
    );

    store.set_history_complete(true).unwrap();
    store.record_sync(Some(at(26, 9)), None).unwrap();
    assert_eq!(
        store.sync_state().unwrap(),
        SyncState { last_success_at: Some(at(26, 9)), last_error: None, history_complete: true },
        "a success clears the error"
    );

    store.set_history_complete(false).unwrap();
    assert!(!store.sync_state().unwrap().history_complete);
    for error in [
        SyncErrorKind::KeyRejected,
        SyncErrorKind::KeyMissing,
        SyncErrorKind::RateLimited,
        SyncErrorKind::Network,
        SyncErrorKind::Unavailable,
        SyncErrorKind::Storage,
    ] {
        store.record_sync(None, Some(error)).unwrap();
        assert_eq!(store.sync_state().unwrap().last_error, Some(error));
    }
    assert_eq!(count_rows(&store, "garmin_sync"), 1);
}

#[test]
fn history_flag_alone_creates_the_sync_row() {
    let store = Store::open_in_memory().unwrap();
    store.set_history_complete(true).unwrap();
    assert_eq!(store.sync_state().unwrap(), SyncState { last_success_at: None, last_error: None, history_complete: true });
}

#[test]
fn bad_stored_sync_values_are_errors() {
    let store = Store::open_in_memory().unwrap();
    store.record_sync(Some(at(25, 10)), Some(SyncErrorKind::Network)).unwrap();
    for (column, value) in [("last_error", "'meltdown'"), ("last_success_at", "'yesterday'")] {
        store.conn.execute(&format!("UPDATE garmin_sync SET {column} = {value}"), []).unwrap();
        let result = store.sync_state();
        assert!(matches!(result, Err(StoreError::Corrupt(_))), "{column}: {result:?}");
        store.record_sync(Some(at(25, 10)), Some(SyncErrorKind::Network)).unwrap();
    }
}

#[test]
fn a_bad_stored_start_is_an_error_not_a_panic() {
    let store = store_with(&[strength(1, 20)]);
    store.conn.execute("UPDATE activity SET start_at = 'soon'", []).unwrap();
    assert!(matches!(store.sync_snapshot(), Err(StoreError::Corrupt(_))));
}

#[test]
fn write_transactions_roll_back_when_dropped() {
    let store = Store::open_in_memory().unwrap();
    {
        let tx = store.write_tx().unwrap();
        tx.execute("INSERT INTO app_settings (key, value) VALUES ('k', 'v')", []).unwrap();
    }
    assert_eq!(count_rows(&store, "app_settings"), 0);
}
