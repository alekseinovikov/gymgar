use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use gymgar_domain::{ActivityId, ExerciseId, Mass, PlannedTarget, RecordedSet, Reps, STRENGTH_TYPE_KEY};
use gymgar_engine::testing::{FIXTURE_STRENGTH_ZIP, FakeGarminSession, TestConfig, connection_at, save_connection, seed_strength, test_session_on};
use gymgar_engine::{Caller, ExerciseView, SecretStore};
use gymgar_store::{FileOutcome, Store};
use secrecy::SecretString;
use serde_json::{Value, json};

use super::*;
use crate::app::Startup;
use crate::i18n::{self, Lang};
use crate::state::Section;
use crate::test_support::{TEST_LOCALE, app_seeded, app_seeded_with, click, harness_sized, listed, pick, store_activities, tick};
use crate::{GymgarApp, UiState};

/// The Romanian deadlift: the fixture's exercise with a title in Russian.
const DEADLIFT: ExerciseId = ExerciseId { category: 8, name: Some(23) };

/// A window tall enough for a whole workout.
const TALL: egui::Vec2 = egui::vec2(1200.0, 1400.0);

fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, minute, 0).single().expect("a valid time")
}

/// Set `index` of a workout that began on Thursday 22 October at 17:31, three minutes after the one before.
fn set(index: u16, reps: Option<u32>, kg: Option<f64>, duration_ms: u32, exercise: Option<ExerciseId>) -> RecordedSet {
    RecordedSet {
        message_index: index,
        start: at(22, 17, 31) + TimeDelta::minutes(i64::from(index) * 3),
        duration_ms,
        reps: reps.map(|n| Reps::new(n).expect("reps")),
        weight: kg.map(|kg| Mass::from_kg(kg).expect("a weight")),
        exercise,
        step_index: None,
        plan: None,
        hr_avg: Some(120 + index),
        hr_max: None,
        rest_after_ms: Some(125_000),
    }
}

/// `set`, made by a step of the workout program that asked for `reps` × `kg`.
fn planned(mut set: RecordedSet, reps: u32, kg: f64) -> RecordedSet {
    set.step_index = Some(0);
    set.plan = Some(PlannedTarget { reps: Some(Reps::new(reps).expect("reps")), weight: Some(Mass::from_kg(kg).expect("a weight")) });
    set
}

/// A workout like the approved mock-up: four Romanian deadlifts planned as 8 × 80 kg, the last of them empty (no reps, no
/// weight, three seconds), and two sets the watch could not name.
fn workout() -> Vec<RecordedSet> {
    vec![
        planned(set(0, Some(8), Some(80.0), 31_000, Some(DEADLIFT)), 8, 80.0),
        planned(set(1, Some(7), Some(80.0), 34_000, Some(DEADLIFT)), 8, 80.0),
        planned(set(2, Some(7), Some(80.0), 33_000, Some(DEADLIFT)), 8, 80.0),
        planned(set(3, None, None, 3_000, Some(DEADLIFT)), 8, 80.0),
        set(4, None, None, 42_000, None),
        set(5, None, None, 40_000, None),
    ]
}

/// The Activities screen on the fixtures and `sets` as a strength workout of their own, which is open beside the list.
fn opened(lang: Lang, sets: Vec<RecordedSet>) -> (Harness<'static, GymgarApp>, ActivityId) {
    let (app, _handles, id) = app_seeded_with(lang, TestConfig::default(), true, |_| {}, |session| seed_strength(session, sets));
    (open_at(app, id.0), id)
}

/// The Activities screen of `app` with the activity `id` selected.
fn open_at(app: GymgarApp, id: i64) -> Harness<'static, GymgarApp> {
    let mut harness = harness_sized(app, TALL);
    harness.state_mut().ui.section = Section::Activities;
    harness.state_mut().ui.activities.selected = Some(id);
    harness.run();
    harness
}

fn select(harness: &mut Harness<'static, GymgarApp>, id: i64) {
    harness.state_mut().ui.activities.selected = Some(id);
    harness.run();
}

/// The id of the listed activity named `name`.
fn id_of(app: &GymgarApp, name: &str) -> i64 {
    app.activities().iter().find(|item| item.name.as_deref() == Some(name)).map(|item| item.id.0).unwrap_or_else(|| panic!("{name} is not listed"))
}

/// What the header says under the name of the number `label`: the topmost text right under the topmost `label`.
fn stat(harness: &Harness<'_, GymgarApp>, label: &str) -> String {
    let name = harness.get_all_by_label(label).map(|node| node.rect()).min_by(|a, b| a.top().total_cmp(&b.top())).expect("the name of the number");
    harness
        .get_all_by(|node| node.role() == Role::Label)
        .filter(|node| (node.rect().left() - name.left()).abs() < 1.0 && node.rect().top() >= name.bottom() - 0.5)
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .and_then(|node| node.accesskit_node().value())
        .unwrap_or_else(|| panic!("nothing under {label}"))
}

fn badge(harness: &Harness<'_, GymgarApp>) -> Option<String> {
    harness.get_by_role_and_label(Role::Button, "Activities").accesskit_node().value()
}

#[test]
fn opening_marks_seen() {
    // Spec §2, item 4: only the user opening an activity takes its "new" mark off.
    let new = [
        listed(9_000_000_000_001, STRENGTH_TYPE_KEY, Some("Upper A"), at(23, 8, 0), true),
        listed(9_000_000_000_002, "running", Some("Evening run"), at(22, 18, 0), true),
    ];
    let (app, _handles) = app_seeded(Lang::En, TestConfig::default(), true, |store| store_activities(store, &new));
    let mut harness = harness_sized(app, TALL);
    harness.state_mut().ui.section = Section::Activities;
    harness.run();
    assert_eq!(badge(&harness).as_deref(), Some("2"));
    harness.get_by_role_and_label(Role::Button, "New · 2");
    harness.get_by_label("Select an activity");

    click(&mut harness, "Upper A");
    harness.get_by_role_and_label(Role::Heading, "Upper A");
    let item = harness.state().activities().iter().find(|item| item.id.0 == 9_000_000_000_001).cloned().expect("listed");
    assert!(!item.is_new, "the dot is drawn from this flag, and it is off");
    assert_eq!(badge(&harness).as_deref(), Some("1"));
    harness.get_by_role_and_label(Role::Button, "New · 1");

    // An activity seen before changes nothing.
    click(&mut harness, "strength_training 1");
    assert_eq!(badge(&harness).as_deref(), Some("1"));
    click(&mut harness, "Evening run");
    assert_eq!(badge(&harness), None);
    harness.get_by_role_and_label(Role::Button, "New · 0");
}

#[test]
fn strength_detail_shows_groups_and_corrections() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, workout());
        harness.state_mut().run("activity.editSets", json!({ "id": id.0, "sets": [{ "recorded": 2 }], "weightKg": 77.5 })).expect("correct a weight");
        harness.state_mut().run("activity.editSets", json!({ "id": id.0, "sets": [{ "recorded": 1 }], "reps": 8 })).expect("correct the reps");
        harness.run();

        // The header: the name (the type's, as the workout has none), the day and the hours, and the numbers.
        harness.get_by_role_and_label(Role::Heading, pick(lang, "Strength training", "Силовая тренировка"));
        harness.get_by_label(pick(lang, "Thursday, 22 October 2026 · 17:31–18:31", "Четверг, 22 октября 2026 · 17:31–18:31"));
        assert_eq!(stat(&harness, pick(lang, "Time", "Время")), "1:00");
        assert_eq!(stat(&harness, pick(lang, "Sets", "Подходов")), "5", "{lang:?}: the empty set does not count");
        assert_eq!(stat(&harness, pick(lang, "Avg HR", "Пульс ср.")), "—");
        assert_eq!(stat(&harness, pick(lang, "Calories", "Калории")), "—");

        // The card of the exercise with the plan its sets followed, and its table. The fixture's watch named the exercise
        // in Russian.
        let deadlift = pick(lang, "Romanian deadlift", "Румынская становая тяга");
        harness.get_by_label(deadlift);
        harness.get_by_label(pick(lang, "plan 4 × 8 · 80 kg", "план 4 × 8 · 80 кг"));
        for (en, ru) in [("Weight", "Вес"), ("Reps", "Повторы"), ("HR", "Пульс"), ("Rest", "Отдых")] {
            assert_eq!(harness.query_all_by_label(pick(lang, en, ru)).count(), 2, "{lang:?}: {en} heads both tables");
        }
        assert!(harness.query_all_by_label("2:05").count() > 1, "{lang:?}: the rest after a set");

        // A correction in the accent colour, and next to it, struck through, what the watch recorded.
        harness.get_by_label(pick(lang, "77.5 kg", "77,5 кг"));
        harness.get_by_label(pick(lang, "was 80", "было 80"));
        harness.get_by_label(pick(lang, "was 7", "было 7"));
        assert_eq!(harness.query_all_by_label(pick(lang, "80 kg", "80 кг")).count(), 2, "{lang:?}: the sets left as recorded");

        // The sets the watch could not name: a card of their own, and the way to name them (`set_edit_ui` tests it).
        harness.get_by_label(pick(lang, "No exercise", "Без упражнения"));
        let choose = harness.get_by_role_and_label(Role::Button, pick(lang, "Choose exercise", "Указать упражнение"));
        assert!(!choose.accesskit_node().is_disabled(), "{lang:?}");
        // Only an exercise's card has "+ Add set".
        assert_eq!(harness.query_all_by_role_and_label(Role::Button, pick(lang, "+ Add set", "+ Добавить подход")).count(), 1, "{lang:?}");
        // The cards come in the order of the workout.
        let none = harness.get_by_label(pick(lang, "No exercise", "Без упражнения")).rect().top();
        assert!(harness.get_by_label(deadlift).rect().top() < none, "{lang:?}");
    }
}

#[test]
fn empty_set_line_and_count_again() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, workout());
        let sets = pick(lang, "Sets", "Подходов");
        assert_eq!(stat(&harness, sets), "5");
        // Spec §2, item 12: the empty set is shown, struck through, and can be counted after all.
        harness.get_by_label(pick(lang, "empty, not counted", "пустой, не учитывается"));
        harness.get_by_role_and_label(Role::Link, pick(lang, "count it", "учитывать")).click();
        harness.run();
        assert_eq!(stat(&harness, sets), "6", "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "empty, not counted", "пустой, не учитывается")).is_none(), "{lang:?}");

        // A set the user left out comes back with "restore": to the rule, which counts it.
        harness.state_mut().run("activity.editSets", json!({ "id": id.0, "sets": [{ "recorded": 0 }], "counted": false })).expect("leave a set out");
        harness.run();
        assert_eq!(stat(&harness, sets), "5", "{lang:?}");
        harness.get_by_label(pick(lang, "not counted", "не учитывается"));
        harness.get_by_role_and_label(Role::Link, pick(lang, "restore", "вернуть")).click();
        harness.run();
        assert_eq!(stat(&harness, sets), "6", "{lang:?}");
        assert!(harness.query_by_role_and_label(Role::Link, pick(lang, "restore", "вернуть")).is_none(), "{lang:?}");
    }
}

#[test]
fn file_states_texts() {
    // Spec §2, item 16, and Review Focus 3: a file still to come, no file, a file that cannot be read, and a file with no
    // sets in it.
    const PENDING: i64 = 9_000_000_000_001;
    const MISSING: i64 = 9_000_000_000_002;
    const UNREADABLE: i64 = 9_000_000_000_003;
    for lang in Lang::ALL {
        let (app, _handles, empty) = app_seeded_with(
            lang,
            TestConfig::default(),
            true,
            |store: &Store| {
                let strength = |id, hours| listed(id, STRENGTH_TYPE_KEY, Some(&format!("Workout {id}")), at(23, 8, 0) - TimeDelta::hours(hours), false);
                store_activities(store, &[strength(PENDING, 1), strength(MISSING, 2), strength(UNREADABLE, 3)]);
                let now = TestConfig::default().now;
                store.apply_file(ActivityId(MISSING), FileOutcome::Missing, now).expect("no file");
                store.apply_file(ActivityId(UNREADABLE), FileOutcome::Unreadable { zip: b"not a zip".to_vec(), parser_version: 1 }, now).expect("a bad file");
            },
            |session| seed_strength(session, Vec::new()),
        );
        let mut harness = open_at(app, PENDING);
        for (id, en, ru) in [
            (PENDING, "Sets are loading…", "Подходы загружаются…"),
            (MISSING, "The recording has no sets", "В записи нет подходов"),
            (UNREADABLE, "Could not read the watch recording", "Не удалось прочитать запись часов"),
            (empty.0, "The recording has no sets", "В записи нет подходов"),
        ] {
            select(&mut harness, id);
            harness.get_by_label(pick(lang, en, ru));
            assert!(harness.query_by_label(pick(lang, "Weight", "Вес")).is_none(), "{lang:?} {id}: no table without sets");
            assert_eq!(stat(&harness, pick(lang, "Sets", "Подходов")), if id == empty.0 { "0" } else { "—" }, "{lang:?} {id}");
        }
    }
}

#[test]
fn exclude_activity_toggle() {
    // Spec §2, item 13: a whole workout can be left out, and taken back.
    for lang in Lang::ALL {
        let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |_| {});
        let id = id_of(&app, "strength_training 1");
        let mut harness = open_at(app, id);
        let sets = pick(lang, "Sets", "Подходов");
        assert_eq!(stat(&harness, sets), "31");
        let leave_out = pick(lang, "Don't count this workout", "Не учитывать тренировку");
        let take_back = pick(lang, "Count this workout", "Учитывать тренировку");
        assert!(harness.query_by_role_and_label(Role::Button, take_back).is_none());

        click(&mut harness, leave_out);
        assert!(harness.state().detail().is_some_and(|detail| detail.summary.excluded), "{lang:?}");
        assert!(harness.state().activities().iter().any(|item| item.id.0 == id && item.excluded), "{lang:?}: the list says so too");
        // The list's line and the header's both say so.
        assert_eq!(harness.query_all_by_label_contains(pick(lang, " · not counted", " · не учитывается")).count(), 2, "{lang:?}");
        assert_eq!(stat(&harness, sets), "—", "{lang:?}: nothing of it counts");

        click(&mut harness, take_back);
        assert!(harness.state().detail().is_some_and(|detail| !detail.summary.excluded), "{lang:?}");
        assert_eq!(stat(&harness, sets), "31", "{lang:?}");
        harness.get_by_role_and_label(Role::Button, leave_out);
    }
}

#[test]
fn russian_watch_title_used_only_in_ru() {
    // Spec §7, «Упражнения и названия»: the fixture's watch calls the Romanian deadlift «Румынская становая тяга».
    for (lang, shown, hidden) in [(Lang::En, "Romanian deadlift", "Румынская становая тяга"), (Lang::Ru, "Румынская становая тяга", "Romanian deadlift")]
    {
        let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |_| {});
        let id = id_of(&app, "strength_training 1");
        let harness = open_at(app, id);
        harness.get_by_label(shown);
        assert!(harness.query_by_label(hidden).is_none(), "{lang:?}");
        harness.get_by_label_contains(pick(lang, " · following the program “FB2”", " · по программе «FB2»"));
    }

    // A title the watch gave in another alphabet is not Russian, and English never takes the watch's title.
    let exercise =
        |title: Option<&str>| ExerciseView { id: "bench_press".to_owned(), english: "Bench press".to_owned(), watch_title: title.map(str::to_owned) };
    for (lang, title, name) in [
        (Lang::Ru, Some("Жим лёжа"), "Жим лёжа"),
        (Lang::Ru, Some("Bankdrücken"), "Bench press"),
        (Lang::Ru, None, "Bench press"),
        (Lang::Ru, Some("  "), "Bench press"),
        (Lang::En, Some("Жим лёжа"), "Bench press"),
    ] {
        i18n::set_current(lang);
        assert_eq!(exercise_name(&exercise(title)), name, "{lang:?} {title:?}");
    }
}

#[test]
fn a_non_strength_activity_shows_only_its_summary() {
    for lang in Lang::ALL {
        let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |_| {});
        let id = id_of(&app, "cycling 3");
        let harness = open_at(app, id);
        harness.get_by_role_and_label(Role::Heading, "cycling 3");
        stat(&harness, pick(lang, "Distance", "Дистанция"));
        stat(&harness, pick(lang, "Time", "Время"));
        for (en, ru) in
            [("Sets", "Подходов"), ("Weight", "Вес"), ("Sets are loading…", "Подходы загружаются…"), ("The recording has no sets", "В записи нет подходов")]
        {
            assert!(harness.query_by_label(pick(lang, en, ru)).is_none(), "{lang:?}: {en}");
        }
        assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Don't count this workout", "Не учитывать тренировку")).is_none(), "{lang:?}");
    }
}

#[test]
fn an_added_set_is_marked_added() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, workout());
        let params = json!({ "id": id.0, "exercise": "deadlift/romanian_deadlift", "weightKg": 82.5, "reps": 6, "after": { "recorded": 2 } });
        harness.state_mut().run("activity.addSet", params).expect("add a set");
        harness.run();
        harness.get_by_label(pick(lang, "added", "добавлен"));
        harness.get_by_label(pick(lang, "82.5 kg", "82,5 кг"));
        assert_eq!(stat(&harness, pick(lang, "Sets", "Подходов")), "6", "{lang:?}: an added set counts");
    }
}

#[test]
fn a_plan_that_differs_by_set_is_shown_at_each_set() {
    // Spec §7, «План подхода»: no plan on the card when the sets were asked for different things; each set shows its own.
    for lang in Lang::ALL {
        let sets =
            vec![planned(set(0, Some(8), Some(80.0), 31_000, Some(DEADLIFT)), 8, 80.0), planned(set(1, Some(6), Some(90.0), 31_000, Some(DEADLIFT)), 6, 90.0)];
        let (harness, _id) = opened(lang, sets);
        assert!(harness.query_by_label_contains(pick(lang, "plan ", "план ")).is_none(), "{lang:?}");
        harness.get_by_label(pick(lang, "Plan", "План"));
        harness.get_by_label(pick(lang, "8 · 80 kg", "8 · 80 кг"));
        harness.get_by_label(pick(lang, "6 · 90 kg", "6 · 90 кг"));
    }
}

#[test]
fn the_detail_fits_the_smallest_window() {
    // The screen works in the minimal window, 800 × 500 (spec §7, «Интерфейс»): nothing runs past its right edge.
    for lang in Lang::ALL {
        let (app, _handles, id) = app_seeded_with(lang, TestConfig::default(), true, |_| {}, |session| seed_strength(session, workout()));
        let mut harness = harness_sized(app, egui::vec2(800.0, 500.0));
        harness.state_mut().ui.section = Section::Activities;
        harness.state_mut().ui.activities.selected = Some(id.0);
        harness.run();
        harness.state_mut().run("activity.editSets", json!({ "id": id.0, "sets": [{ "recorded": 0 }], "counted": false })).expect("leave a set out");
        harness.run();
        harness.get_by_role_and_label(Role::Link, pick(lang, "count it", "учитывать"));
        harness.get_by_role_and_label(Role::Link, pick(lang, "restore", "вернуть"));
        for node in harness.root().children_recursive() {
            let role = node.accesskit_node().role();
            if matches!(role, Role::Label | Role::Link | Role::Button | Role::Heading) {
                let right = node.rect().right();
                assert!(right <= 800.5, "{lang:?}: {role:?} {:?} runs to {right}", node.accesskit_node().value().or(node.accesskit_node().label()));
            }
        }
    }
}

#[test]
fn the_open_activity_is_read_again_only_when_it_may_have_changed() {
    // Ruling 5: never on every frame. A frame with nothing new reads nothing; the user's own change, a job's end and the
    // arrival of the open workout's file read it again, the files of other workouts do not.
    const OPEN: i64 = 9_000_000_000_001;
    let garmin = Arc::new(FakeGarminSession::new());
    let mut items =
        vec![json!({ "activityId": OPEN, "activityName": "Upper A", "activityType": { "typeKey": STRENGTH_TYPE_KEY }, "startTimeGMT": "2026-10-23 08:00:00" })];
    for n in 1..=60 {
        items.push(json!({ "activityId": n, "activityName": format!("Workout {n}"), "activityType": { "typeKey": STRENGTH_TYPE_KEY }, "startTimeGMT": format!("2026-09-{:02} 10:00:00", n % 28 + 1) }));
        garmin.set_file(ActivityId(n), b"not a zip".to_vec());
    }
    garmin.set_pages(vec![Value::Array(items).to_string().into_bytes()]);
    garmin.set_file(ActivityId(OPEN), FIXTURE_STRENGTH_ZIP.to_vec());
    let cfg = TestConfig { session: Some(garmin), deferred: true, ..TestConfig::default() };
    let (app, handles) =
        app_seeded(Lang::En, cfg, true, |store| store_activities(store, &[listed(OPEN, STRENGTH_TYPE_KEY, Some("Upper A"), at(23, 8, 0), false)]));
    let spawner = handles.spawner.expect("a deferred spawner");
    let mut harness = open_at(app, OPEN);
    harness.get_by_label("Sets are loading…");
    let reads = |harness: &Harness<'_, GymgarApp>| harness.state().detail_reads;
    assert_eq!(reads(&harness), 1, "read once when it was opened");
    // The first look at the database's version cannot tell what changed before it.
    tick(&mut harness);
    let opened = reads(&harness);
    for _ in 0..5 {
        harness.step();
        tick(&mut harness);
    }
    assert_eq!(reads(&harness), opened, "frames and ticks with nothing new read nothing");

    harness.state_mut().run("garmin.sync", Value::Null).expect("start a sync");
    let after_start = reads(&harness);
    spawner.run_pending();
    // The first tick applies the page and the open workout's file (the newest is fetched first), among others.
    tick(&mut harness);
    assert_eq!(reads(&harness), after_start + 1, "its file arrived");
    assert!(harness.state().detail().is_some_and(|detail| !detail.groups.is_empty()), "the sets are there");
    harness.get_by_label("Romanian deadlift");
    // The files of the other workouts take a few more ticks: none of them touches the open one.
    while harness.state().status().is_some_and(|status| status.sync.running) {
        tick(&mut harness);
        if harness.state().status().is_some_and(|status| status.sync.running) {
            assert_eq!(reads(&harness), after_start + 1, "another workout's file");
        }
    }
    assert_eq!(reads(&harness), after_start + 2, "the end of the sync");
    for _ in 0..3 {
        tick(&mut harness);
    }
    assert_eq!(reads(&harness), after_start + 2);
}

#[test]
fn an_agent_correcting_the_open_activity_is_seen_within_a_second() {
    // Another process (`gymgar-cli` serving an agent) corrects a set: the open workout shows it after the next poll of the
    // database (spec §2, item 21).
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("gymgar.sqlite3");
    let cfg = TestConfig::default();
    let (session, handles) = test_session_on(Store::open(&path).expect("open the database"), cfg.clone());
    save_connection(&session, &connection_at(cfg.now, gymgar_domain::SecretBackend::File));
    handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
    let id = seed_strength(&session, workout());
    let mut app = GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned()))
        .with_utc_offset(chrono::FixedOffset::east_opt(0).expect("UTC"))
        .with_time(cfg.now);
    app.startup = Startup::Done;
    let mut harness = open_at(app, id.0);
    tick(&mut harness);
    assert!(harness.query_by_label("77.5 kg").is_none());

    let (mut agent, _agent_handles) = test_session_on(Store::open(&path).expect("open the database"), TestConfig::default());
    agent
        .execute("activity.editSets", json!({ "id": id.0, "sets": [{ "recorded": 2 }], "weightKg": 77.5 }), Caller::Agent)
        .expect("the agent corrects a weight");
    harness.state_mut().next_store_poll = Instant::now();
    tick(&mut harness);
    harness.get_by_label("77.5 kg");
    harness.get_by_label("was 80");
}

#[test]
fn another_activity_opens_at_its_top() {
    // Two long workouts in a window too short for either: the detail scrolls.
    let long: Vec<RecordedSet> = (0..30).map(|index| set(index, Some(8), Some(80.0), 31_000, Some(DEADLIFT))).collect();
    let (app, _handles, (first, second)) =
        app_seeded_with(Lang::En, TestConfig::default(), true, |_| {}, |session| (seed_strength(session, long.clone()), seed_strength(session, long)));
    let mut harness = harness_sized(app, egui::vec2(1200.0, 600.0));
    harness.state_mut().ui.section = Section::Activities;
    select(&mut harness, first.0);
    let heading = |harness: &Harness<'_, GymgarApp>| harness.get_by_role_and_label(Role::Heading, "Strength training").rect();
    let top = heading(&harness);

    // Down to the last line of the first workout: its heading is out of sight.
    let last = harness
        .get_all_by(|node| node.role() == Role::Label)
        .filter(|node| node.rect().left() >= top.left() - 0.5)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("a line of the workout");
    last.scroll_to_me();
    harness.run();
    assert!(heading(&harness).top() < top.top() - 100.0, "the detail scrolled: {:?}", heading(&harness));

    // The other workout opens at its top, not where the first was left.
    select(&mut harness, second.0);
    assert!((heading(&harness).top() - top.top()).abs() < 0.5, "{:?}, expected the top at {}", heading(&harness), top.top());
}
