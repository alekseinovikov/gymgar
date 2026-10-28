use chrono::{TimeDelta, TimeZone, Utc};
use egui::Role;
use egui_kittest::kittest::Queryable;
use gymgar_domain::{ConnectionStatus, ExerciseId, GarminConnection, RecordedSet, STRENGTH_TYPE_KEY};
use gymgar_engine::testing::{TestConfig, strength_set};
use serde_json::json;

use crate::Section;
use crate::activity_detail_ui::exercise_name;
use crate::i18n::Lang;
use crate::state::{ListFilter, ProgressFilter};
use crate::test_support::{
    app_connected, app_in, app_seeded, app_with_workouts, click, harness, harness_sized, listed, pick, seed_titled_workout, store_activities,
    working_connection,
};
use crate::theme::metrics;

/// The state line of the Garmin card on a working connection.
const CONNECTED_TEXT: (&str, &str) = ("Connected", "Подключено");

/// The text the Summary had before workouts were loaded: it must be gone (spec §2, item 15).
const NEXT_UPDATE: (&str, &str) =
    ("Garmin is connected. Loading workouts will arrive in the next update.", "Garmin подключён. Загрузка тренировок появится в следующем обновлении.");

const NEW_STRENGTH: i64 = 9_000_000_000_001;

fn expiring_in(cfg: &TestConfig, days: i64) -> GarminConnection {
    GarminConnection { key_expires_at: cfg.now + TimeDelta::days(days), ..working_connection(cfg) }
}

fn with_status(cfg: &TestConfig, status: ConnectionStatus) -> GarminConnection {
    GarminConnection { last_status: status, ..working_connection(cfg) }
}

#[test]
fn first_run_summary_has_connect_card() {
    for lang in Lang::ALL {
        let (app, _handles) = app_in(lang, TestConfig::default());
        let mut harness = harness(app);
        assert_eq!(harness.state().ui.section, Section::Summary);
        harness.get_by_label(pick(lang, "Workouts, progress and recovery will appear here", "Здесь появятся тренировки, прогресс и восстановление"));
        harness.get_by_label(pick(lang, "Connect to Garmin Connect", "Подключите Garmin Connect"));
        harness.get_by_label(pick(
            lang,
            "Gymgar takes your workouts, sleep and HRV from Garmin. You need a personal API key, which takes a couple of minutes.",
            "Gymgar берёт ваши тренировки, сон и ВСР из Garmin. Нужен личный ключ API — это займёт пару минут.",
        ));

        click(&mut harness, pick(lang, "Connect Garmin", "Подключить Garmin"));
        assert_eq!(harness.state().ui.section, Section::Settings, "{lang:?}: the button leads to Settings");
        harness.get_by_label(pick(lang, "Garmin Connect", "Garmin Connect"));
        assert!(harness.query_by_label(pick(lang, "Connect Garmin", "Подключить Garmin")).is_none(), "{lang:?}: the card is not on Settings");
    }
}

#[test]
fn the_first_run_has_one_card_and_no_connected_text() {
    for lang in Lang::ALL {
        let (app, _handles) = app_in(lang, TestConfig::default());
        let harness = harness(app);
        assert!(harness.query_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1)).is_none());
        assert_eq!(harness.get_all_by_role(Role::Button).count(), 5, "{lang:?}: the four rail buttons and the one on the card");
    }
}

#[test]
fn connected_summary_text() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        let harness = harness(app);
        harness.get_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1));
        assert!(harness.query_by_label(pick(lang, NEXT_UPDATE.0, NEXT_UPDATE.1)).is_none(), "{lang:?}: the promise is kept, so it goes");
        // Connected: no invitation, and no warning while the key has a year to live.
        assert!(harness.query_by_label(pick(lang, "Connect to Garmin Connect", "Подключите Garmin Connect")).is_none(), "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "Connect Garmin", "Подключить Garmin")).is_none(), "{lang:?}");
        assert!(harness.query_by_label_contains(pick(lang, "expires", "истекает")).is_none(), "{lang:?}");
    }
}

#[test]
fn a_key_that_could_not_be_checked_shows_the_connected_text_marked() {
    // Design spec §2, item 8: the last known state, with the mark that the check got no answer.
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &with_status(&cfg, ConnectionStatus::Unverified));
        let harness = harness(app);
        harness.get_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1));
        harness.get_by_label(pick(lang, "Could not verify", "Не удалось проверить"));
    }
}

#[test]
fn a_key_checked_with_success_has_no_mark() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        assert!(harness(app).query_by_label(pick(lang, "Could not verify", "Не удалось проверить")).is_none(), "{lang:?}");
    }
}

#[test]
fn expiry_warning_on_summary() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        // Three days left, from 2026-10-23 12:00 UTC.
        let (app, _handles) = app_connected(lang, cfg.clone(), &expiring_in(&cfg, 3));
        let mut harness = harness(app);
        harness.get_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1));
        harness.get_by_label(pick(
            lang,
            "The Garmin key expires on 26 October 2026. Create a new one",
            "Ключ Garmin истекает 26 октября 2026. Выпустите новый",
        ));
        harness.get_by_role_and_label(Role::Link, pick(lang, "Personal API Key page", "Страница Personal API Key"));

        // The button takes the user to the form that replaces the key.
        click(&mut harness, pick(lang, "Replace key", "Заменить ключ"));
        assert_eq!((harness.state().ui.section, harness.state().ui.connect.replacing), (Section::Settings, true), "{lang:?}");
        harness.get_by_role(Role::PasswordInput);
    }
}

#[test]
fn no_expiry_warning_until_the_last_week() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        for days in [8, 30, 365] {
            let (app, _handles) = app_connected(lang, cfg.clone(), &expiring_in(&cfg, days));
            let harness = harness(app);
            assert!(harness.query_by_label_contains(pick(lang, "expires", "истекает")).is_none(), "{lang:?}: {days} days left");
        }
        let (app, _handles) = app_connected(lang, cfg.clone(), &expiring_in(&cfg, 7));
        harness(app).get_by_label_contains(pick(lang, "expires on", "истекает"));
    }
}

#[test]
fn a_rejected_or_missing_key_asks_to_fix_it_on_summary() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &with_status(&cfg, ConnectionStatus::KeyRejected));
        let mut rejected = harness(app);
        rejected.get_by_label(pick(lang, "Key not valid", "Ключ не действует"));
        assert!(rejected.query_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1)).is_none(), "{lang:?}: a key that does not work is not a connection");
        click(&mut rejected, pick(lang, "Replace key", "Заменить ключ"));
        assert_eq!(rejected.state().ui.section, Section::Settings, "{lang:?}");

        let (app, _handles) = app_connected(lang, cfg.clone(), &with_status(&cfg, ConnectionStatus::KeyMissing));
        let mut missing = harness(app);
        missing.get_by_label(pick(lang, "Key not found", "Ключ не найден"));
        missing.get_by_label(pick(lang, "Connect Garmin again", "Подключите Garmin заново"));
        click(&mut missing, pick(lang, "Connect Garmin", "Подключить Garmin"));
        assert_eq!(missing.state().ui.section, Section::Settings, "{lang:?}");
    }
}

/// The Summary on the fixtures plus two activities the user has not opened yet: a strength one and a run.
fn with_two_new(lang: Lang) -> egui_kittest::Harness<'static, crate::GymgarApp> {
    let cfg = TestConfig::default();
    let new = [
        listed(NEW_STRENGTH, STRENGTH_TYPE_KEY, Some("Upper A"), cfg.now - TimeDelta::hours(4), true),
        listed(NEW_STRENGTH + 1, "running", None, cfg.now - TimeDelta::hours(18), true),
    ];
    let (app, _handles) = app_seeded(lang, cfg, true, |store| store_activities(store, &new));
    harness(app)
}

#[test]
fn summary_new_card_opens_filtered_list() {
    for lang in Lang::ALL {
        let mut harness = with_two_new(lang);
        harness.get_by_label(pick(lang, "2 new workouts", "2 новые тренировки"));
        click(&mut harness, pick(lang, "Show new", "Показать новые"));
        let ui = &harness.state().ui;
        assert_eq!((ui.section, ui.activities.filter), (Section::Activities, ListFilter::New), "{lang:?}");
        // The list shows the two, and only them.
        harness.get_by_role_and_label(Role::Button, "Upper A");
        harness.get_by_role_and_label(Role::Button, pick(lang, "Running", "Бег"));
        assert!(harness.query_by_role_and_label(Role::Button, "strength_training 1").is_none(), "{lang:?}");
    }
}

#[test]
fn the_new_card_counts_in_the_language_and_goes_when_nothing_is_new() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let one = [listed(NEW_STRENGTH, STRENGTH_TYPE_KEY, Some("Upper A"), cfg.now - TimeDelta::hours(4), true)];
        let (app, _handles) = app_seeded(lang, cfg.clone(), true, |store| store_activities(store, &one));
        harness(app).get_by_label(pick(lang, "1 new workout", "1 новая тренировка"));
        let five: Vec<_> = (0..5).map(|n| listed(NEW_STRENGTH + n, STRENGTH_TYPE_KEY, None, cfg.now - TimeDelta::hours(n + 1), true)).collect();
        let (app, _handles) = app_seeded(lang, cfg.clone(), true, |store| store_activities(store, &five));
        harness(app).get_by_label(pick(lang, "5 new workouts", "5 новых тренировок"));

        let (app, _handles) = app_seeded(lang, cfg, true, |_| {});
        let harness = harness(app);
        assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Show new", "Показать новые")).is_none(), "{lang:?}");
    }
}

#[test]
fn the_last_strength_workout_has_a_card_that_opens_it() {
    for lang in Lang::ALL {
        let mut summary = with_two_new(lang);
        summary.get_by_label(pick(lang, "Last strength workout", "Последняя силовая"));
        summary.get_by_label("Upper A");
        summary.get_by_label(pick(lang, "Fri, 23 Oct · 1:00", "Пт, 23 окт · 1:00"));
        summary.state_mut().ui.activities.filter = ListFilter::New;
        click(&mut summary, pick(lang, "Open", "Открыть"));
        let ui = &summary.state().ui;
        assert_eq!((ui.section, ui.activities.selected), (Section::Activities, Some(NEW_STRENGTH)), "{lang:?}");
        assert_eq!(ui.activities.filter, ListFilter::New, "{lang:?}: a filter that shows it stays");

        // From a filter that would hide it (an old workout is not under "New"), the list goes back to all.
        let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |_| {});
        let mut harness = harness(app);
        harness.get_by_label("strength_training 1");
        harness.state_mut().ui.activities.filter = ListFilter::New;
        click(&mut harness, pick(lang, "Open", "Открыть"));
        assert_eq!(harness.state().ui.activities.filter, ListFilter::All, "{lang:?}");
    }
}

#[test]
fn a_summary_without_strength_workouts_has_no_card_for_them() {
    let (app, _handles) = app_connected(Lang::En, TestConfig::default(), &working_connection(&TestConfig::default()));
    let harness = harness(app);
    assert!(harness.query_by_label("Last strength workout").is_none());
    harness.get_by_label("Connected");
}

const SQUAT: ExerciseId = ExerciseId { category: 28, name: None };
const BENCH: ExerciseId = ExerciseId { category: 0, name: None };
const DEADLIFT: ExerciseId = ExerciseId { category: 8, name: None };

type Workout = (&'static str, Option<&'static str>, Vec<RecordedSet>);

/// One working set of an exercise, planned for 10 reps.
fn lift(exercise: ExerciseId, kg: f64, reps: u32) -> RecordedSet {
    strength_set(0, exercise, kg, reps, Some(10))
}

/// Two exercises that can add weight and one to hold. The deadlift was done a day after the squat each time, so its advice
/// is the newer one and comes first. The first workout is the activity 7,100,001 and so on.
fn two_can_add() -> Vec<Workout> {
    vec![
        ("2026-10-20 07:55:00", Some("Legs"), vec![lift(SQUAT, 80.0, 10)]),
        ("2026-10-21 07:55:00", Some("Pull"), vec![lift(DEADLIFT, 50.0, 10)]),
        ("2026-10-24 07:55:00", Some("Legs"), vec![lift(SQUAT, 80.0, 10)]),
        ("2026-10-25 07:55:00", Some("Pull"), vec![lift(DEADLIFT, 50.0, 10)]),
        ("2026-10-26 07:55:00", Some("Push"), vec![lift(BENCH, 95.0, 8)]),
    ]
}

/// Six exercises, the first of them the squat, each of which can add 5 kg to 80: the exercise `n` was done on 2 + `n` and
/// 9 + `n` October, so the last one listed is the newest.
fn six_can_add(count: usize) -> Vec<Workout> {
    const EXERCISES: [ExerciseId; 6] =
        [SQUAT, BENCH, DEADLIFT, ExerciseId { category: 23, name: None }, ExerciseId { category: 7, name: None }, ExerciseId { category: 24, name: None }];
    const FIRST: [&str; 6] =
        ["2026-10-02 07:55:00", "2026-10-03 07:55:00", "2026-10-04 07:55:00", "2026-10-05 07:55:00", "2026-10-06 07:55:00", "2026-10-07 07:55:00"];
    const SECOND: [&str; 6] =
        ["2026-10-09 07:55:00", "2026-10-10 07:55:00", "2026-10-11 07:55:00", "2026-10-12 07:55:00", "2026-10-13 07:55:00", "2026-10-14 07:55:00"];
    let sessions = |days: [&'static str; 6]| EXERCISES.into_iter().zip(days).take(count).map(|(exercise, day)| (day, None, vec![lift(exercise, 80.0, 10)]));
    sessions(FIRST).chain(sessions(SECOND)).collect()
}

/// Where the label `text` is drawn: its top edge.
fn label_top(harness: &egui_kittest::Harness<'_, crate::GymgarApp>, text: &str) -> f32 {
    harness.get_by_label(text).rect().top()
}

#[test]
fn a_card_lists_the_exercises_that_can_take_more_weight() {
    for lang in Lang::ALL {
        let (app, _handles) = app_with_workouts(lang, &two_can_add());
        let harness = harness(app);
        harness.get_by_label(pick(lang, "Can add weight: 2 exercises", "Можно добавлять: 2 упражнения"));
        // From the weight the exercise is at to the weight it can go to; the step is 2.5 kg, and 50 kg + 5 % is 52.5.
        harness.get_by_label(pick(lang, "50 to 52.5 kg", "с 50 до 52,5 кг"));
        harness.get_by_label(pick(lang, "80 to 85 kg", "с 80 до 85 кг"));
        // In the order of the tiles, the newest advice first.
        assert!(label_top(&harness, "Deadlift") < label_top(&harness, "Squat"), "{lang:?}");
        // The exercise that holds is not on the card.
        assert!(harness.query_by_label("Bench press").is_none(), "{lang:?}");
        harness.get_by_role_and_label(Role::Button, pick(lang, "Open progress", "Открыть прогресс"));
    }
}

/// Where the label `text` is drawn.
fn label_rect(harness: &egui_kittest::Harness<'_, crate::GymgarApp>, text: &str) -> egui::Rect {
    harness.get_by_label(text).rect()
}

#[test]
fn a_range_follows_its_name_on_one_line() {
    for lang in Lang::ALL {
        let (app, _handles) = app_with_workouts(lang, &two_can_add());
        let harness = harness(app);
        // The window is 1200 points wide: a range at the card's far edge would be about a thousand points from its name.
        for (name, range) in [("Squat", pick(lang, "80 to 85 kg", "с 80 до 85 кг")), ("Deadlift", pick(lang, "50 to 52.5 kg", "с 50 до 52,5 кг"))] {
            let (name, range) = (label_rect(&harness, name), label_rect(&harness, range));
            assert!((name.center().y - range.center().y).abs() < 1.0, "{lang:?}: one line: {name:?} and {range:?}");
            let between = range.left() - name.right();
            assert!((0.0..=metrics::GAP + 1.0).contains(&between), "{lang:?}: the range is {between} points from its name");
        }
    }
}

#[test]
fn a_long_name_is_cut_and_the_range_stays_whole_on_its_line() {
    // The watch's own title for a squat, as long as the longest names Garmin's programs have.
    let title = "Приседания со штангой на спине с паузой внизу и подъёмом на носки, узкая постановка ног".to_owned();
    let (app, _handles) = app_seeded(Lang::Ru, TestConfig::default(), true, |store| {
        for (id, day) in [(7_200_001, 17), (7_200_002, 24)] {
            let start = Utc.with_ymd_and_hms(2026, 10, day, 6, 0, 0).single().expect("a valid time");
            seed_titled_workout(store, id, start, vec![lift(SQUAT, 80.0, 10)], vec![(SQUAT, title.clone())]);
            let start = start + TimeDelta::hours(1);
            seed_titled_workout(store, id + 10, start, vec![lift(DEADLIFT, 50.0, 10)], Vec::new());
        }
    });
    let window = egui::vec2(800.0, 500.0);
    let mut harness = harness_sized(app, window);
    harness.run();
    let range = harness.get_by_role_and_label(Role::Label, "с 80 до 85 кг").rect();
    let name = harness.get_by_role_and_label(Role::Label, &title).rect();
    assert!(name.right() <= range.left(), "the name runs into the range: {name:?} and {range:?}");
    assert!(range.right() <= window.x, "the range runs out of the window: {range:?}");
    // The name takes what the range leaves: it is cut, not wrapped, and the range is just after it.
    assert!(range.left() - name.right() <= metrics::GAP + 1.0, "{name:?} and {range:?}");
    let short = harness.get_by_role_and_label(Role::Label, "Deadlift").rect();
    assert_eq!(name.height().round(), short.height().round(), "the name wraps");
    assert!((name.center().y - range.center().y).abs() < 1.0, "one line: {name:?} and {range:?}");
}

#[test]
fn the_card_is_not_there_when_no_exercise_can_add() {
    for lang in Lang::ALL {
        // One session each: nothing to base the advice on yet; and a bench press that holds.
        let hold = [("2026-10-26 07:55:00", Some("Push"), vec![lift(BENCH, 95.0, 8)]), ("2026-10-25 07:55:00", None, vec![lift(SQUAT, 80.0, 10)])];
        for workouts in [Vec::new(), hold.to_vec()] {
            let (app, _handles) = app_with_workouts(lang, &workouts);
            let harness = harness(app);
            assert!(harness.query_by_label_contains(pick(lang, "Can add weight", "Можно добавлять")).is_none(), "{lang:?}");
            assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Open progress", "Открыть прогресс")).is_none(), "{lang:?}");
        }
    }
}

#[test]
fn the_card_counts_its_exercises_in_the_language() {
    for lang in Lang::ALL {
        // One, two and six: the three forms of the Russian noun.
        for (count, en, ru) in [
            (1, "Can add weight: 1 exercise", "Можно добавлять: 1 упражнение"),
            (2, "Can add weight: 2 exercises", "Можно добавлять: 2 упражнения"),
            (6, "Can add weight: 6 exercises", "Можно добавлять: 6 упражнений"),
        ] {
            let (app, _handles) = app_with_workouts(lang, &six_can_add(count));
            harness(app).get_by_label(pick(lang, en, ru));
        }
    }
}

#[test]
fn the_card_lists_five_exercises_and_counts_the_rest() {
    for lang in Lang::ALL {
        let (app, _handles) = app_with_workouts(lang, &six_can_add(6));
        let six = harness(app);
        // The tiles' order: the newest first, so the squat, done first, is the sixth.
        let names: Vec<String> = six.state().progress().iter().map(|item| exercise_name(&item.exercise).to_owned()).collect();
        assert_eq!(names.len(), 6);
        assert_eq!(names.last().map(String::as_str), Some("Squat"), "{lang:?}");
        for name in &names[..5] {
            six.get_by_label(name);
        }
        assert!(six.query_by_label("Squat").is_none(), "{lang:?}: the sixth is counted, not listed");
        assert_eq!(six.get_all_by_label(pick(lang, "80 to 85 kg", "с 80 до 85 кг")).count(), 5, "{lang:?}");
        six.get_by_label(pick(lang, "and 1 more", "и ещё 1"));

        // Five fit; the card then says nothing of the rest.
        let (app, _handles) = app_with_workouts(lang, &six_can_add(5));
        let five = harness(app);
        assert_eq!(five.get_all_by_label(pick(lang, "80 to 85 kg", "с 80 до 85 кг")).count(), 5, "{lang:?}");
        assert!(five.query_by_label_contains(pick(lang, "more", "ещё")).is_none(), "{lang:?}");
    }
}

#[test]
fn the_button_opens_progress_on_the_exercises_that_can_add() {
    for lang in Lang::ALL {
        let (mut app, _handles) = app_with_workouts(lang, &two_can_add());
        // An exercise left open on Progress, and the chip of another filter, give way.
        app.open_exercise(Some("squat".to_owned()));
        app.ui.progress.filter = ProgressFilter::Break;
        let mut harness = harness(app);
        click(&mut harness, pick(lang, "Open progress", "Открыть прогресс"));
        let ui = &harness.state().ui;
        assert_eq!((ui.section, ui.progress.filter, ui.progress.open.as_deref()), (Section::Progress, ProgressFilter::Add, None), "{lang:?}");
        // The tiles of the two, and no other.
        harness.get_by_role_and_label(Role::Button, "Squat");
        harness.get_by_role_and_label(Role::Button, "Deadlift");
        assert!(harness.query_by_role_and_label(Role::Button, "Bench press").is_none(), "{lang:?}");
    }
}

#[test]
fn the_card_follows_the_data() {
    let (app, _handles) = app_with_workouts(Lang::En, &two_can_add());
    let mut harness = harness(app);
    harness.get_by_label("Can add weight: 2 exercises");
    // The last squat is left out of the statistics: one session is no advice, and the card counts one exercise.
    harness.state_mut().run("activity.setExcluded", json!({ "id": 7_100_003, "excluded": true })).expect("leave the workout out");
    harness.run();
    harness.get_by_label("Can add weight: 1 exercise");
    assert!(harness.query_by_label("Squat").is_none());
    harness.get_by_label("Deadlift");
}
