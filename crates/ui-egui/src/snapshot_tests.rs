//! A manual test that renders the screens to PNG so a person can look at them (CLAUDE.md, section 2, rule 8).
//!
//! It needs a GPU (wgpu) and writes files, so it is `#[ignore]`d. Run it by hand:
//!
//! ```sh
//! GYMGAR_SNAPSHOTS=/tmp/gymgar-shots cargo test -p gymgar-ui-egui -- --ignored render_snapshots
//! ```
//!
//! Without `GYMGAR_SNAPSHOTS` it writes nothing. It draws the real app (its `logic` and `ui`), 1200x800 points at a
//! scale of 2, in English and in Russian: `<state>-en.png` and `<state>-ru.png` (`progress-*` is the Progress section: the
//! tiles of every status, at both sizes, with a chip chosen, and with nothing to show; `progress-detail-*` the page of an
//! exercise). The Activities screen and the Summary
//! with activities are drawn at the smallest window too, 800x500: `<state>-800-<lang>.png`; so is the open activity
//! (`detail-*`): a strength workout with corrections, another activity, and the three states without sets; and the
//! correcting of it (`edit-*`): the line of a set open in the editor, its error, and the exercise picker.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gymgar_domain::{
    ActivityId, ActivitySummary, ConnectionStatus, ExerciseId, GarminConnection, Mass, PlannedTarget, RecordedSet, Reps, STRENGTH_TYPE_KEY, StrengthRecord,
    SyncErrorKind,
};
use gymgar_engine::GarminAuthError;
use gymgar_engine::testing::{FakeGarminSession, TestConfig, strength_set};
use gymgar_store::{FileOutcome, NewActivity, Store};
use serde_json::{Value, json};

use crate::activity_detail_ui::exercise_name;
use crate::i18n::Lang;
use crate::test_support::{
    WINDOW, app_connected, app_in, app_on_store, app_seeded, app_with_workouts, builder, click, listed, pick, progress_now, seed_titled_workout,
    store_activities, tick, type_key, working_connection,
};
use crate::{GymgarApp, ProgressFilter, Section, StartupErrorApp};

/// The environment variable that names the directory the PNGs go to.
const DIRECTORY_VARIABLE: &str = "GYMGAR_SNAPSHOTS";

/// Points to pixels, so the text in the PNGs is as sharp as on a retina screen.
const SCALE: f32 = 2.0;

/// The window of the "cannot start" screen, the size the app opens it with.
fn error_window(app: StartupErrorApp) -> Harness<'static, StartupErrorApp> {
    let mut harness: Harness<'static, StartupErrorApp> =
        builder().with_size(egui::vec2(720.0, 400.0)).with_pixels_per_point(SCALE).wgpu().build_eframe(move |_| app);
    harness.run();
    harness
}

/// A made-up key to fill the field with; it is hidden in the picture.
const KEY: &str = "zq-abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz";

type Shot = Harness<'static, GymgarApp>;

/// A window running the real app, the size of the app's start window.
fn window(app: GymgarApp, section: Section) -> Shot {
    window_sized(app, section, WINDOW)
}

/// A window running the real app, `size` points large.
fn window_sized(app: GymgarApp, section: Section, size: egui::Vec2) -> Shot {
    let mut harness = eframe_window(app, size);
    harness.state_mut().ui.section = section;
    harness.run();
    harness
}

/// The real app in a window of `size` points, not yet drawn.
fn eframe_window(app: GymgarApp, size: egui::Vec2) -> Shot {
    builder().with_size(size).with_pixels_per_point(SCALE).wgpu().build_eframe(move |_| app)
}

/// The two window sizes of the Activities screen: the start window and the smallest one.
const SIZES: [(egui::Vec2, &str); 2] = [(WINDOW, ""), (egui::vec2(800.0, 500.0), "-800")];

/// Activities that look like a real history: two weeks of strength workouts and runs, two of them not opened yet, on top
/// of the engine's fixtures (whose names are scrubbed to their types). The last sync ended an hour ago.
fn history(store: &Store, now: DateTime<Utc>) {
    let at = |hours: i64| now - TimeDelta::hours(hours);
    let mut activities: Vec<NewActivity> = vec![
        listed(9_000_000_000_010, STRENGTH_TYPE_KEY, Some("Upper A"), at(3), true),
        listed(9_000_000_000_009, "running", None, at(20), true),
        listed(9_000_000_000_008, STRENGTH_TYPE_KEY, Some("Lower B — heavy squats, Romanian deadlifts and a long finisher on the sled"), at(52), false),
        listed(9_000_000_000_007, STRENGTH_TYPE_KEY, None, at(150), false),
        listed(9_000_000_000_006, STRENGTH_TYPE_KEY, Some("Upper B"), at(198), false),
        listed(9_000_000_000_005, "cycling", Some("Evening ride"), at(220), false),
    ];
    for activity in &mut activities {
        // The workouts are of different lengths, as real ones are.
        activity.summary.duration_ms = Some(1_721_000 + u64::try_from(activity.summary.id.0 % 7).unwrap_or(0) * 1_000_000);
    }
    store_activities(store, &activities);
    store.record_sync(Some(now - TimeDelta::hours(1)), None).expect("record the sync");
}

/// The strength workout of the approved mock-ups, "Upper A" of three hours ago (its file read): a bench press the watch
/// named in Russian, planned as 4 × 8 at 80 kg, its last set empty; two sets the watch could not name; and Romanian
/// deadlifts whose plan changed from set to set.
const UPPER_A: i64 = 9_000_000_000_020;

fn upper_a(store: &Store, now: DateTime<Utc>) {
    let start = now - TimeDelta::hours(3);
    let bench = ExerciseId { category: 0, name: Some(1) };
    let deadlift = ExerciseId { category: 8, name: Some(23) };
    let set = |index: u16, reps: Option<u32>, kg: Option<f64>, seconds: u32, exercise: Option<ExerciseId>, plan: Option<(u32, f64)>| RecordedSet {
        message_index: index,
        start: start + TimeDelta::minutes(i64::from(index) * 4),
        duration_ms: seconds * 1000,
        reps: reps.map(|n| Reps::new(n).expect("reps")),
        weight: kg.map(|kg| Mass::from_kg(kg).expect("a weight")),
        exercise,
        step_index: plan.map(|_| index),
        plan: plan.map(|(reps, kg)| PlannedTarget { reps: Some(Reps::new(reps).expect("reps")), weight: Some(Mass::from_kg(kg).expect("a weight")) }),
        hr_avg: Some(118 + index * 3),
        hr_max: None,
        rest_after_ms: Some(120_000 + u32::from(index) * 9_000),
    };
    let sets = vec![
        set(0, Some(8), Some(80.0), 31, Some(bench), Some((8, 80.0))),
        set(1, Some(7), Some(80.0), 34, Some(bench), Some((8, 80.0))),
        set(2, Some(7), Some(80.0), 33, Some(bench), Some((8, 80.0))),
        set(3, None, None, 3, Some(bench), Some((8, 80.0))),
        set(4, None, None, 42, None, None),
        set(5, None, None, 40, None, None),
        set(6, Some(10), Some(70.0), 38, Some(deadlift), Some((10, 70.0))),
        set(7, Some(8), Some(80.0), 36, Some(deadlift), Some((8, 80.0))),
        set(8, Some(6), Some(90.0), 35, Some(deadlift), Some((6, 90.0))),
    ];
    let summary = ActivitySummary {
        id: ActivityId(UPPER_A),
        type_key: STRENGTH_TYPE_KEY.to_owned(),
        name: Some("Upper A".to_owned()),
        start,
        duration_ms: Some(5_940_000),
        distance_m: None,
        avg_hr: Some(112),
        calories: Some(486),
        manual: false,
    };
    store_activities(store, &[NewActivity { summary, raw_json: "{}".to_owned(), is_new: false }]);
    let record = StrengthRecord { workout_name: Some("Upper A".to_owned()), sets, titles: vec![(bench, "Жим штанги лёжа".to_owned())] };
    store.apply_file(ActivityId(UPPER_A), FileOutcome::Parsed { zip: None, record, parser_version: 1 }, now).expect("store the sets");
}

/// Three strength workouts without sets to show: one whose file is still to come, one Garmin has no file for and one whose
/// file cannot be read.
const PENDING: i64 = 9_000_000_000_021;
const MISSING: i64 = 9_000_000_000_022;
const UNREADABLE: i64 = 9_000_000_000_023;

fn without_sets(store: &Store, now: DateTime<Utc>) {
    let workout = |id: i64, name: &str| listed(id, STRENGTH_TYPE_KEY, Some(name), now - TimeDelta::hours(id - UPPER_A), false);
    store_activities(store, &[workout(PENDING, "Lower B"), workout(MISSING, "Logged by hand"), workout(UNREADABLE, "Upper B")]);
    store.apply_file(ActivityId(MISSING), FileOutcome::Missing, now).expect("no file");
    store.apply_file(ActivityId(UNREADABLE), FileOutcome::Unreadable { zip: b"not a zip".to_vec(), parser_version: 1 }, now).expect("a bad file");
}

/// A window on the fixtures, "Upper A" with the user's corrections (reps, a weight, a set left out and one added) and the
/// workouts without sets. The last sync ended an hour ago.
fn detail_app(lang: Lang) -> (GymgarApp, gymgar_engine::testing::TestHandles) {
    let cfg = on_hold();
    let (mut app, handles) = app_seeded(lang, cfg.clone(), true, |store| {
        upper_a(store, cfg.now);
        without_sets(store, cfg.now);
        store.record_sync(Some(cfg.now - TimeDelta::hours(1)), None).expect("record the sync");
    });
    for (params, what) in [
        (json!({ "id": UPPER_A, "sets": [{ "recorded": 1 }], "reps": 8 }), "correct the reps"),
        (json!({ "id": UPPER_A, "sets": [{ "recorded": 2 }], "weightKg": 77.5 }), "correct a weight"),
        (json!({ "id": UPPER_A, "sets": [{ "recorded": 8 }], "counted": false }), "leave a set out"),
    ] {
        app.run("activity.editSets", params).expect(what);
    }
    let added = json!({ "id": UPPER_A, "exercise": "deadlift/romanian_deadlift", "weightKg": 90, "reps": 5, "after": { "recorded": 8 } });
    app.run("activity.addSet", added).expect("add a set");
    (app, handles)
}

/// One exercise of the history Progress is drawn from: what it is called on the watch, and its sessions as (days before
/// the clock, weight, the reps of each set at that weight). The program asked for `plan` reps (none: it asked for nothing).
struct Lift {
    exercise: ExerciseId,
    title: &'static str,
    plan: Option<u32>,
    sessions: &'static [(i64, f64, &'static [u32])],
}

const fn lift(exercise: ExerciseId, title: &'static str, plan: Option<u32>, sessions: &'static [(i64, f64, &'static [u32])]) -> Lift {
    Lift { exercise, title, plan, sessions }
}

const fn exercise(category: u16, name: Option<u16>) -> ExerciseId {
    ExerciseId { category, name }
}

/// Eleven exercises that look like half a year of training, one for each way a tile looks: three that can add weight, two
/// to hold (the second with a name too long for its tile), one after a break, bodyweight, no target, a break, and two nobody
/// has done for months (the older one more than a year, with no chart left).
fn lifts() -> [Lift; 11] {
    [
        lift(
            exercise(8, Some(23)),
            "Румынская становая тяга",
            Some(10),
            &[
                (200, 40.0, &[10]),
                (170, 50.0, &[10]),
                (140, 55.0, &[10]),
                (110, 60.0, &[10]),
                (80, 65.0, &[10]),
                (50, 65.0, &[10]),
                (24, 70.0, &[10, 10]),
                (10, 70.0, &[10, 10]),
            ],
        ),
        lift(
            exercise(24, None),
            "Жим гантелей над головой",
            Some(10),
            &[(150, 40.0, &[10]), (120, 42.0, &[10]), (90, 44.0, &[10]), (60, 46.0, &[10]), (30, 48.0, &[10]), (9, 48.0, &[10])],
        ),
        lift(
            exercise(28, None),
            "Приседание со штангой",
            Some(10),
            &[(190, 60.0, &[10]), (160, 70.0, &[10]), (120, 80.0, &[10]), (90, 85.0, &[10]), (60, 90.0, &[10]), (22, 90.0, &[10]), (8, 90.0, &[10])],
        ),
        lift(
            exercise(0, Some(1)),
            "Жим штанги лёжа",
            Some(10),
            &[(170, 85.0, &[10]), (120, 90.0, &[10]), (70, 95.0, &[10]), (30, 95.0, &[10]), (9, 95.0, &[8, 8, 6])],
        ),
        lift(
            exercise(23, None),
            "Тяга штанги в наклоне",
            Some(10),
            &[(130, 55.0, &[10]), (90, 60.0, &[8]), (45, 60.0, &[8]), (20, 60.0, &[8]), (6, 60.0, &[10])],
        ),
        lift(exercise(21, None), "Подтягивания", Some(10), &[(100, 0.0, &[8]), (70, 0.0, &[9]), (40, 0.0, &[10]), (15, 0.0, &[12])]),
        lift(
            exercise(30, None),
            "Разгибание рук на трицепс с канатной рукоятью на верхнем блоке стоя",
            None,
            &[(60, 25.0, &[12]), (30, 27.5, &[12]), (11, 30.0, &[12])],
        ),
        lift(exercise(15, None), "Сгибание ног лёжа", Some(10), &[(130, 40.0, &[10]), (100, 40.0, &[10]), (6, 30.0, &[8])]),
        lift(exercise(9, None), "Разведение гантелей лёжа", Some(12), &[(120, 14.0, &[12]), (80, 16.0, &[12]), (44, 16.0, &[12])]),
        lift(exercise(7, None), "Сгибание рук со штангой", Some(10), &[(300, 25.0, &[10]), (260, 27.5, &[10]), (210, 30.0, &[10])]),
        lift(exercise(14, None), "Подъём гантелей в стороны", Some(15), &[(440, 8.0, &[15]), (420, 8.0, &[15])]),
    ]
}

/// A window on the history of [`lifts`]: Garmin connected, "today" 2026-10-27.
fn progress_app(lang: Lang) -> (GymgarApp, gymgar_engine::testing::TestHandles) {
    let now = progress_now();
    let store = Store::open_in_memory().expect("an in-memory store");
    for (id, (lift, days, kg, reps)) in
        (7_300_001_i64..).zip(lifts().iter().flat_map(|lift| lift.sessions.iter().map(move |(days, kg, reps)| (lift, days, kg, reps))))
    {
        let sets = reps.iter().enumerate().map(|(index, reps)| strength_set(u16::try_from(index).unwrap_or(0), lift.exercise, *kg, *reps, lift.plan)).collect();
        seed_titled_workout(&store, id, now - TimeDelta::days(*days) - TimeDelta::hours(6), sets, vec![(lift.exercise, lift.title.to_owned())]);
    }
    app_on_store(lang, store)
}

/// A Garmin that lists `count` strength workouts and sends a broken file for each: a sync with plenty of files to count.
fn garmin_with_files(count: i64) -> Arc<FakeGarminSession> {
    let garmin = Arc::new(FakeGarminSession::new());
    let items: Vec<Value> = (1..=count)
        .map(|n| {
            json!({ "activityId": n, "activityName": format!("Workout {n}"), "activityType": { "typeKey": "strength_training" }, "startTimeGMT": format!("2026-09-{:02} 10:00:00", n % 28 + 1) })
        })
        .collect();
    garmin.set_pages(vec![Value::Array(items).to_string().into_bytes()]);
    for n in 1..=count {
        garmin.set_file(ActivityId(n), b"not a zip".to_vec());
    }
    garmin
}

fn save<S>(harness: &mut Harness<'static, S>, directory: &Path, state: &str, lang: Lang) {
    // The pointer, left over a button by the last click, is not part of the screen.
    harness.event(egui::Event::PointerGone);
    harness.step();
    harness.step();
    let image = harness.render().expect("render the window (does this machine have a GPU?)");
    let path = directory.join(format!("{state}-{}.png", pick(lang, "en", "ru")));
    image.save(&path).expect("write the PNG");
    eprintln!("wrote {}", path.display());
}

fn expiring_in(cfg: &TestConfig, days: i64) -> GarminConnection {
    GarminConnection { key_expires_at: cfg.now + TimeDelta::days(days), ..working_connection(cfg) }
}

/// The start-up check of a saved connection never finishes in these windows (the spawner is deferred), so a window
/// shows the saved state as the connection was left.
fn on_hold() -> TestConfig {
    TestConfig { deferred: true, ..TestConfig::default() }
}

#[test]
#[ignore = "renders PNGs with the GPU; run by hand with GYMGAR_SNAPSHOTS=<directory>"]
fn render_snapshots() {
    let Some(directory) = std::env::var_os(DIRECTORY_VARIABLE).map(PathBuf::from) else {
        eprintln!("{DIRECTORY_VARIABLE} is not set: nothing rendered");
        return;
    };
    std::fs::create_dir_all(&directory).expect("create the snapshot directory");

    for lang in Lang::ALL {
        // 1. The first run: an empty Summary with the invitation.
        let (app, _handles) = app_in(lang, on_hold());
        save(&mut window(app, Section::Summary), &directory, "summary-empty", lang);
        // The Progress section: the tiles of every status at both sizes, with a chip chosen, and with nothing to show
        // (connected, and before the connection).
        for (size, suffix) in SIZES {
            let (app, _handles) = progress_app(lang);
            save(&mut window_sized(app, Section::Progress, size), &directory, &format!("progress-tiles{suffix}"), lang);
        }
        let (mut app, _handles) = progress_app(lang);
        app.ui.progress.filter = ProgressFilter::Add;
        save(&mut window(app, Section::Progress), &directory, "progress-add", lang);
        let (app, _handles) = app_on_store(lang, Store::open_in_memory().expect("an in-memory store"));
        save(&mut window(app, Section::Progress), &directory, "progress-empty", lang);
        let (app, _handles) = app_in(lang, on_hold());
        save(&mut window(app, Section::Progress), &directory, "progress-empty-not-connected", lang);
        // The page of an exercise: one that can add weight, one with circles and two shaded breaks (at both sizes), one that
        // came back after a break, one with no weight (a chart of reps) and one on a break that is going on.
        for (state, title, sizes) in [
            ("add", "Румынская становая тяга", &SIZES[..]),
            ("hold", "Тяга штанги в наклоне", &SIZES[..]),
            ("after-break", "Сгибание ног лёжа", &SIZES[..1]),
            ("bodyweight", "Подтягивания", &SIZES[..1]),
            ("break", "Сгибание рук со штангой", &SIZES[..1]),
        ] {
            for &(size, suffix) in sizes {
                let (mut app, _handles) = progress_app(lang);
                let key = app.progress().iter().find(|item| item.exercise.watch_title.as_deref() == Some(title)).map(|item| item.exercise.id.clone());
                app.open_exercise(Some(key.expect("a tile of this exercise")));
                save(&mut window_sized(app, Section::Progress, size), &directory, &format!("progress-detail-{state}{suffix}"), lang);
            }
        }

        // The same page with programs named and every session in view: a tall window, wide and narrow.
        for (size, suffix) in [(egui::vec2(1200.0, 1150.0), ""), (egui::vec2(800.0, 1150.0), "-800")] {
            let squat = ExerciseId { category: 28, name: None };
            let sets = |kg: f64, reps: u32| vec![strength_set(0, squat, kg, reps, Some(10))];
            let workouts = vec![
                ("2026-07-06 08:00:00", Some("Legs"), sets(70.0, 10)),
                ("2026-07-27 08:00:00", Some("Legs"), sets(70.0, 10)),
                ("2026-08-17 08:00:00", Some("Legs, heavy day with the long finisher on the sled"), sets(75.0, 10)),
                ("2026-09-07 08:00:00", Some("Legs"), sets(75.0, 10)),
                ("2026-09-28 08:00:00", Some("Legs"), vec![strength_set(0, squat, 80.0, 10, Some(10)), strength_set(1, squat, 90.0, 6, Some(10))]),
                ("2026-10-20 08:00:00", Some("Legs"), vec![strength_set(0, squat, 80.0, 10, Some(10)), strength_set(1, squat, 80.0, 10, Some(10))]),
                ("2026-10-24 08:00:00", Some("Legs"), sets(80.0, 10)),
            ];
            let (mut app, _handles) = app_with_workouts(lang, &workouts);
            app.open_exercise(Some("squat".to_owned()));
            save(&mut window_sized(app, Section::Progress, size), &directory, &format!("progress-detail-programs{suffix}"), lang);
        }

        // 2. Settings before connecting, with the key typed and Garmin's refusal under the form.
        let cfg = TestConfig { garmin: Err(GarminAuthError::KeyRejected), ..on_hold() };
        let (app, handles) = app_in(lang, cfg);
        let mut harness = window(app, Section::Settings);
        type_key(&mut harness, KEY);
        click(&mut harness, pick(lang, "Connect", "Подключить"));
        save(&mut harness, &directory, "settings-connecting", lang);
        handles.spawner.expect("a deferred spawner").run_pending();
        tick(&mut harness);
        harness.run();
        save(&mut harness, &directory, "settings-error", lang);

        // 3. Connected, with a year left.
        let cfg = on_hold();
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        save(&mut window(app, Section::Settings), &directory, "settings-connected", lang);
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        save(&mut window(app, Section::Summary), &directory, "summary-connected", lang);
        // The user confirmed "Disconnect" and the key is being deleted: the indicator is by the button.
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        let mut harness = window(app, Section::Settings);
        click(&mut harness, pick(lang, "Disconnect", "Отключить"));
        click(&mut harness, pick(lang, "Yes, disconnect", "Да, отключить"));
        save(&mut harness, &directory, "settings-disconnecting", lang);

        // 4. The key expires in three days, on both screens.
        let connection = expiring_in(&cfg, 3);
        let (app, _handles) = app_connected(lang, cfg.clone(), &connection);
        save(&mut window(app, Section::Summary), &directory, "summary-expiry-warning", lang);
        let (app, _handles) = app_connected(lang, cfg.clone(), &connection);
        save(&mut window(app, Section::Settings), &directory, "settings-expiry-warning", lang);

        // 5. The states of a key that does not work and of one that is gone.
        for (status, name) in [
            (ConnectionStatus::KeyRejected, "settings-key-rejected"),
            (ConnectionStatus::KeyMissing, "settings-key-missing"),
            (ConnectionStatus::Unverified, "settings-unverified"),
        ] {
            let connection = GarminConnection { last_status: status, ..working_connection(&cfg) };
            let (app, _handles) = app_connected(lang, cfg.clone(), &connection);
            save(&mut window(app, Section::Settings), &directory, name, lang);
        }
        let connection = GarminConnection { last_status: ConnectionStatus::Unverified, ..working_connection(&cfg) };
        let (app, _handles) = app_connected(lang, cfg.clone(), &connection);
        save(&mut window(app, Section::Summary), &directory, "summary-unverified", lang);

        // 6. No system password store: the warning that names the file, and the consent box. A path as long as a real one on
        // macOS, and a long Windows one that has no spaces to break at.
        for (path, name) in [
            ("/Users/alexandra/Library/Application Support/Gymgar/secrets/garmin-personal-api-key", "settings-file-consent"),
            (r"C:\Users\alexandra.petrovna-ivanova\AppData\Roaming\Gymgar\data\secrets\garmin-personal-api-key", "settings-file-consent-windows-path"),
        ] {
            let cfg = TestConfig { system_store: false, file_store_location: Some(path.to_owned()), ..on_hold() };
            let (app, _handles) = app_in(lang, cfg);
            save(&mut window(app, Section::Settings), &directory, name, lang);
        }

        // 7. Activities: the list with a row selected, at both sizes; a sync that runs; a sync that stopped on the key; no
        // connection; nothing loaded yet. And the Summary with new workouts.
        for (size, suffix) in SIZES {
            let cfg = on_hold();
            let (mut app, _handles) = app_seeded(lang, cfg.clone(), true, |store| history(store, cfg.now));
            app.ui.activities.selected = Some(9_000_000_000_010);
            save(&mut window_sized(app, Section::Activities, size), &directory, &format!("activities{suffix}"), lang);

            let (app, _handles) = app_seeded(lang, cfg.clone(), true, |store| history(store, cfg.now));
            save(&mut window_sized(app, Section::Summary, size), &directory, &format!("summary-new{suffix}"), lang);

            let (app, _handles) = app_seeded(lang, cfg.clone(), true, |store| {
                history(store, cfg.now);
                store.record_sync(None, Some(SyncErrorKind::KeyRejected)).expect("record the failure");
            });
            save(&mut window_sized(app, Section::Activities, size), &directory, &format!("activities-error{suffix}"), lang);

            // The sync has run in the background; the window applies what it sent a few dozen pieces a frame, so the
            // first frames show it half-way.
            let cfg = TestConfig { session: Some(garmin_with_files(150)), ..on_hold() };
            let (mut app, handles) = app_seeded(lang, cfg.clone(), true, |store| history(store, cfg.now));
            app.run("garmin.sync", Value::Null).expect("start a sync");
            handles.spawner.expect("a deferred spawner").run_pending();
            let mut harness = eframe_window(app, size);
            harness.state_mut().ui.section = Section::Activities;
            harness.step();
            save(&mut harness, &directory, &format!("activities-loading{suffix}"), lang);
        }
        let cfg = on_hold();
        let (app, _handles) = app_seeded(lang, cfg.clone(), false, |store| history(store, cfg.now));
        save(&mut window(app, Section::Activities), &directory, "activities-not-connected", lang);
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        save(&mut window(app, Section::Activities), &directory, "activities-empty", lang);

        // 8. The open activity: the strength workout with the user's corrections (a weight, reps, a set left out and one
        // added), a ride, and the three states of a workout without sets.
        for (size, suffix) in SIZES {
            for (name, which) in [("detail-strength", None), ("detail-other", Some("cycling 3")), ("detail-pending", Some("Lower B"))]
                .into_iter()
                .chain([("detail-missing", Some("Logged by hand")), ("detail-unreadable", Some("Upper B"))])
            {
                let (mut app, _handles) = detail_app(lang);
                let id = match which {
                    Some(listed) => app.activities().iter().find(|item| item.name.as_deref() == Some(listed)).map(|item| item.id.0).expect("listed"),
                    None => UPPER_A,
                };
                app.ui.activities.selected = Some(id);
                save(&mut window_sized(app, Section::Activities, size), &directory, &format!("{name}{suffix}"), lang);
            }
        }

        // 9. Correcting "Upper A" (the mock-up `workouts-edit.html`): the third bench press, whose weight the user corrected,
        // open in the editor, then with a weight that is not one; and the picker for the sets without an exercise,
        // searched, its first exercise chosen.
        for (size, suffix) in SIZES {
            let (mut app, _handles) = detail_app(lang);
            app.ui.activities.selected = Some(UPPER_A);
            let mut harness = window_sized(app, Section::Activities, size);
            let line = pick(lang, "Edit set 3", "Изменить подход 3");
            harness.get_all_by_role_and_label(Role::Button, line).next().expect("the third set").click();
            harness.run();
            save(&mut harness, &directory, &format!("edit-row{suffix}"), lang);
            // A weight that is not one: the reason under the line.
            harness.get_by_role_and_label(Role::TextInput, pick(lang, "Weight", "Вес")).set_text_value_accesskit("80,5,5");
            harness.run();
            click(&mut harness, pick(lang, "Save", "Сохранить"));
            save(&mut harness, &directory, &format!("edit-error{suffix}"), lang);

            let (mut app, _handles) = detail_app(lang);
            app.ui.activities.selected = Some(UPPER_A);
            let mut harness = window_sized(app, Section::Activities, size);
            click(&mut harness, pick(lang, "Choose exercise", "Указать упражнение"));
            // A new panel is measured in a frame of its own before it shows.
            harness.run();
            harness.get_by_role_and_label(Role::TextInput, pick(lang, "Search exercises", "Поиск упражнения")).set_text_value_accesskit("bench");
            harness.run();
            let first = harness.state().ui.activities.picker.as_ref().and_then(|picker| picker.hits.first()).map(|hit| exercise_name(&hit.exercise).to_owned());
            harness.get_by_role_and_label(Role::Button, &first.expect("the search found something")).click();
            harness.run();
            save(&mut harness, &directory, &format!("edit-picker{suffix}"), lang);
        }

        // 10. Gymgar cannot start: the database is damaged.
        let locale = Some(pick(lang, "en-US", "ru-RU").to_owned());
        let file = std::path::Path::new("/Users/alexandra/Library/Application Support/Gymgar/gymgar.sqlite3");
        let error = "the database is corrupt: /Users/alexandra/Library/Application Support/Gymgar/gymgar.sqlite3: file is not a database";
        save(&mut error_window(StartupErrorApp::with_system_locale(error, Some(file), locale.clone())), &directory, "cannot-start-database", lang);
        save(
            &mut error_window(StartupErrorApp::with_system_locale("cannot start the background runtime: out of threads", None, locale)),
            &directory,
            "cannot-start-runtime",
            lang,
        );
    }
}
