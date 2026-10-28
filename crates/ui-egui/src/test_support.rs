//! Helpers the UI tests share: a headless harness with the Sport theme and the icons installed, an app on the engine's
//! test doubles, and the few gestures every screen test makes. Compiled for tests only.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use egui::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use gymgar_domain::{ActivityId, ActivitySummary, ExerciseId, GarminConnection, RecordedSet, STRENGTH_TYPE_KEY, SecretBackend, StrengthRecord};
use gymgar_engine::testing::{TestConfig, TestHandles, connection_at, save_connection, seed_fixture_activities, seed_workout, test_session_on};
use gymgar_engine::{Outcome, SecretStore, Session};
use gymgar_store::{FileOutcome, NewActivity, Store};
use secrecy::SecretString;
use serde_json::json;

use crate::app::Startup;
use crate::i18n::Lang;
use crate::theme::{self, Tokens};
use crate::{GymgarApp, Section, UiState, icons};

/// The size of the test window: the app's start size.
pub(crate) const WINDOW: egui::Vec2 = egui::vec2(1200.0, 800.0);

/// The locale the test apps report as the system's. Tests must not depend on the language of the machine they run on.
pub(crate) const TEST_LOCALE: &str = "en-US";

/// A harness builder with the test window's size, styled and ready for icons as the real window is.
///
/// A character no bundled font has panics the test (egui_kittest's default), so the interface keeps to the glyphs the
/// bundled fonts have and looks the same on every operating system.
pub(crate) fn builder<S>() -> HarnessBuilder<S> {
    Harness::builder().with_size(WINDOW).with_context_setup(|ctx| {
        theme::apply(ctx, &Tokens::SPORT);
        icons::install(ctx);
    })
}

/// A headless harness running `add` with `state`, styled and ready for icons as the real window is.
pub(crate) fn ui_harness<'a, S>(add: impl FnMut(&mut egui::Ui, &mut S) + 'a, state: S) -> Harness<'a, S> {
    builder().build_ui_state(add, state)
}

/// A harness showing the whole app.
pub(crate) fn harness(app: GymgarApp) -> Harness<'static, GymgarApp> {
    ui_harness(|ui, app: &mut GymgarApp| app.show(ui), app)
}

/// A harness showing the whole app in a window of `size`: a tall one shows a long list whole.
pub(crate) fn harness_sized(app: GymgarApp, size: egui::Vec2) -> Harness<'static, GymgarApp> {
    builder().with_size(size).build_ui_state(|ui, app: &mut GymgarApp| app.show(ui), app)
}

/// An app on a fresh in-memory database and the doubles from `cfg`, with the interface language "system" on an
/// English machine. Dates are shown in UTC, at the config's time. It has not ticked yet, so a connection it finds starts the
/// import at the first tick.
pub(crate) fn app(cfg: TestConfig) -> (GymgarApp, TestHandles) {
    app_on_machine(TEST_LOCALE, cfg, None, false)
}

/// Like [`app`], with the interface language "system" on a machine in `lang`, and a window that has been open a while:
/// the import that the app starts when it opens is over (there was nothing to import from).
pub(crate) fn app_in(lang: Lang, cfg: TestConfig) -> (GymgarApp, TestHandles) {
    app_on_machine(locale(lang), cfg, None, true)
}

/// Like [`app_in`], with `connection` already saved in the database, as an earlier run of the app would have left it.
/// The start-up import is over: this window has been open a while, and its ticks start nothing by themselves.
pub(crate) fn app_connected(lang: Lang, cfg: TestConfig, connection: &GarminConnection) -> (GymgarApp, TestHandles) {
    app_on_machine(locale(lang), cfg, Some(connection), true)
}

/// A window open a while (the start-up import is over) on a database that holds a working connection when `connected`,
/// whatever `prepare` writes to it before the window opens, and the activities of the engine's fixtures: eight of five
/// types, none new, the newest strength one with its sets. The key of the connection is in the file store, so a sync
/// can run.
pub(crate) fn app_seeded(lang: Lang, cfg: TestConfig, connected: bool, prepare: impl FnOnce(&Store)) -> (GymgarApp, TestHandles) {
    let (app, handles, ()) = app_seeded_with(lang, cfg, connected, prepare, |_| ());
    (app, handles)
}

/// Like [`app_seeded`], and `seed` runs on the session after the fixtures, for what the engine's own helpers write
/// (`seed_strength`); what it returns comes back with the app.
pub(crate) fn app_seeded_with<T>(
    lang: Lang,
    cfg: TestConfig,
    connected: bool,
    prepare: impl FnOnce(&Store),
    seed: impl FnOnce(&Session) -> T,
) -> (GymgarApp, TestHandles, T) {
    let store = Store::open_in_memory().expect("an in-memory store");
    prepare(&store);
    let connection = connected.then(|| working_connection(&cfg));
    let (session, handles) = test_session_on(store, cfg.clone());
    if let Some(connection) = &connection {
        save_connection(&session, connection);
        handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
    }
    seed_fixture_activities(&session);
    let seeded = seed(&session);
    (open(session, locale(lang), &cfg, true), handles, seeded)
}

/// A window open a while on a database that holds `workouts` and nothing else (no fixture activities, so no other exercise
/// has a tile), with Garmin connected. A workout is its local start (`2026-10-20 07:55:00`), its program's name and its
/// sets, in a zone two hours ahead of UTC; the first has the activity id 7,100,001 and so on. The session's clock is
/// 2026-10-27 12:00 UTC, which dates are shown in too, so a week-old session is fresh.
pub(crate) fn app_with_workouts(lang: Lang, workouts: &[(&str, Option<&str>, Vec<RecordedSet>)]) -> (GymgarApp, TestHandles) {
    let store = Store::open_in_memory().expect("an in-memory store");
    for (id, (local_start, name, sets)) in (7_100_001_i64..).zip(workouts) {
        seed_workout(&store, id, local_start, 2, *name, sets.clone());
    }
    app_on_store(lang, store)
}

/// The clock of the windows of [`app_on_store`]: what "today" is for the progress of an exercise.
pub(crate) fn progress_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).single().expect("a valid time")
}

/// A window open a while on `store`, with Garmin connected, at the clock of [`progress_now`].
pub(crate) fn app_on_store(lang: Lang, store: Store) -> (GymgarApp, TestHandles) {
    let cfg = TestConfig { now: progress_now(), ..TestConfig::default() };
    let (session, handles) = test_session_on(store, cfg.clone());
    save_connection(&session, &working_connection(&cfg));
    handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
    (open(session, locale(lang), &cfg, true), handles)
}

/// Stores a strength workout that began at `start` with `sets`, and `titles` the watch gave its exercises (what a Russian
/// interface shows instead of the English names, when they are in Cyrillic). The engine's `seed_workout` has none.
pub(crate) fn seed_titled_workout(store: &Store, id: i64, start: DateTime<Utc>, sets: Vec<RecordedSet>, titles: Vec<(ExerciseId, String)>) {
    let activity = listed(id, STRENGTH_TYPE_KEY, None, start, false);
    store_activities(store, &[activity]);
    let record = StrengthRecord { workout_name: None, sets, titles };
    store.apply_file(ActivityId(id), FileOutcome::Parsed { zip: None, record, parser_version: 1 }, start).expect("store the sets");
}

/// An activity as a sync stores it: an hour long, and 5.2 km unless it is a strength one.
pub(crate) fn listed(id: i64, type_key: &str, name: Option<&str>, start: DateTime<Utc>, is_new: bool) -> NewActivity {
    let summary = ActivitySummary {
        id: ActivityId(id),
        type_key: type_key.to_owned(),
        name: name.map(str::to_owned),
        start,
        duration_ms: Some(3_600_000),
        distance_m: (type_key != STRENGTH_TYPE_KEY).then_some(5_200.0),
        avg_hr: None,
        calories: None,
        manual: false,
    };
    NewActivity { summary, raw_json: json!({ "activityId": id }).to_string(), is_new }
}

/// Stores `activities` as a sync would have.
pub(crate) fn store_activities(store: &Store, activities: &[NewActivity]) {
    store.upsert_activities(activities, TestConfig::default().now).expect("store the activities");
}

/// Like [`app_in`], connected the way the user does it: `key` was entered and Garmin accepted it, so the key is in the
/// secret store the rules choose, and the first import that follows a connection (Ruling 19) is over. Needs
/// `cfg.deferred` off, so the check and the import run at once.
pub(crate) fn app_with_key(lang: Lang, cfg: TestConfig, key: &str) -> (GymgarApp, TestHandles) {
    let (mut app, handles) = app_in(lang, cfg);
    let started = app.run("garmin.connect", json!({ "apiKey": key, "keyTerm": 365, "allowFileStore": true }));
    let Ok(Outcome::Started(job)) = started else { panic!("connecting did not start: {started:?}") };
    app.ui.connect.pending = Some(job);
    let ctx = egui::Context::default();
    app.tick(&ctx);
    assert!(app.status().is_some_and(|status| status.status == gymgar_domain::ConnectionStatus::Connected), "Garmin did not accept the key");
    // The import ran inline when the connection was applied; this applies its end.
    app.tick(&ctx);
    assert!(app.status().is_some_and(|status| !status.sync.running), "the first import is over");
    (app, handles)
}

/// A working connection made at the config's time, its key kept in the file store.
pub(crate) fn working_connection(cfg: &TestConfig) -> GarminConnection {
    connection_at(cfg.now, SecretBackend::File)
}

fn locale(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "en-US",
        Lang::Ru => "ru-RU",
    }
}

fn app_on_machine(locale: &str, cfg: TestConfig, connection: Option<&GarminConnection>, started: bool) -> (GymgarApp, TestHandles) {
    let (session, handles) = test_session_on(Store::open_in_memory().expect("an in-memory store"), cfg.clone());
    if let Some(connection) = connection {
        save_connection(&session, connection);
    }
    (open(session, locale, &cfg, started), handles)
}

/// The window on `session`, on a machine in `locale`, in UTC and at the config's time. `started`: the start-up import has
/// been dealt with, so ticks start nothing by themselves.
fn open(session: Session, locale: &str, cfg: &TestConfig, started: bool) -> GymgarApp {
    let utc = FixedOffset::east_opt(0).expect("UTC is a valid offset");
    let mut app = GymgarApp::with_system_locale(session, UiState::default(), Some(locale.to_owned())).with_utc_offset(utc).with_time(cfg.now);
    if started {
        app.startup = Startup::Done;
    }
    app
}

/// `en` or `ru`, whichever the test is checking: the texts of the screens in each language, written out in the test so
/// that it also checks the catalog against the spec.
pub(crate) fn pick(lang: Lang, en: &'static str, ru: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Ru => ru,
    }
}

/// A harness showing `section` of `app`.
pub(crate) fn harness_at(app: GymgarApp, section: Section) -> Harness<'static, GymgarApp> {
    let mut harness = harness(app);
    harness.state_mut().ui.section = section;
    harness.run();
    harness
}

/// Clicks the button `label`, and draws one frame. Not until the window is quiet: while a key is being checked the
/// window shows a spinner, which asks for a new frame all the time.
pub(crate) fn click(harness: &mut Harness<'_, GymgarApp>, label: &str) {
    harness.get_by_role_and_label(Role::Button, label).click();
    harness.step();
    harness.step();
}

/// Pastes `key` into the key field.
pub(crate) fn type_key(harness: &mut Harness<'_, GymgarApp>, key: &str) {
    harness.get_by_role(Role::PasswordInput).focus();
    harness.run();
    harness.get_by_role(Role::PasswordInput).type_text(key);
    harness.run();
}

/// What the main loop does each frame before drawing: applies what the finished background jobs found.
pub(crate) fn tick(harness: &mut Harness<'_, GymgarApp>) {
    let ctx = harness.ctx.clone();
    harness.state_mut().tick(&ctx);
    harness.step();
    harness.step();
}

/// Whether the button `label` is on the screen and can be clicked.
pub(crate) fn button_enabled(harness: &Harness<'_, GymgarApp>, label: &str) -> bool {
    !harness.get_by_role_and_label(Role::Button, label).accesskit_node().is_disabled()
}

/// Fails when any node of the accessibility tree (what a screen reader or a test reads) carries `secret`, or its first
/// 16 characters, in its label, value, description or placeholder.
pub(crate) fn assert_not_exposed(harness: &Harness<'_, GymgarApp>, secret: &str) {
    let probe = secret.get(..16).unwrap_or(secret);
    for node in harness.root().children_recursive() {
        let node = node.accesskit_node();
        for text in [node.label(), node.value(), node.description(), node.placeholder().map(str::to_owned)].into_iter().flatten() {
            assert!(!text.contains(probe), "{:?} exposes the key: {text}", node.role());
        }
    }
}
