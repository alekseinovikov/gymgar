use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use gymgar_domain::{ActivityId, ConnectionStatus, Language, SecretBackend};
use gymgar_engine::testing::{FakeGarminSession, RequestKind, TestConfig, connection_at, save_connection, test_session_on, test_session_with};
use gymgar_engine::{Caller, EngineError, GarminAuthError, Outcome, SecretStore};
use gymgar_store::Store;
use secrecy::SecretString;
use serde_json::{Value, json};

use super::*;
use crate::Section;
use crate::i18n::{self, Lang};
use crate::test_support::{TEST_LOCALE, app, app_seeded, harness, listed, store_activities};
use crate::theme::metrics;

fn config(garmin: Result<(), GarminAuthError>, deferred: bool) -> TestConfig {
    TestConfig { garmin, deferred, ..TestConfig::default() }
}

fn status_of(app: &GymgarApp) -> ConnectionStatus {
    app.status().expect("the status was read").status
}

/// An app whose database already holds a connection (its key kept in the file store, which is empty).
fn connected_app(cfg: TestConfig) -> (GymgarApp, gymgar_engine::testing::TestHandles) {
    let now = cfg.now;
    let (session, handles) = test_session_with(cfg);
    save_connection(&session, &connection_at(now, SecretBackend::File));
    (GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned())), handles)
}

#[test]
fn rail_shows_summary_and_settings() {
    let (app, _handles) = app(TestConfig::default());
    let mut harness = harness(app);
    harness.get_by_role_and_label(Role::Button, "Summary");
    harness.get_by_role_and_label(Role::Button, "Settings");
    assert_eq!(harness.state().ui.section, Section::Summary);

    harness.get_by_role_and_label(Role::Button, "Settings").click();
    harness.run();
    assert_eq!(harness.state().ui.section, Section::Settings);

    harness.get_by_role_and_label(Role::Button, "Summary").click();
    harness.run();
    assert_eq!(harness.state().ui.section, Section::Summary);
}

#[test]
fn the_rail_has_summary_and_activities_on_top_and_settings_at_the_bottom() {
    let (app, _handles) = app(TestConfig::default());
    let harness = harness(app);
    // The buttons of the rail only: the screen beside it has buttons of its own.
    let rail_edge = metrics::RAIL_WIDTH + 16.0;
    let buttons: Vec<_> = harness
        .get_all_by_role(Role::Button)
        .filter(|button| button.rect().right() < rail_edge)
        .map(|button| (button.accesskit_node().label().unwrap_or_default(), button.rect()))
        .collect();
    let labels: Vec<_> = buttons.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, ["Summary", "Activities", "Settings"], "{buttons:?}");
    let (summary, activities, settings) = (buttons[0].1, buttons[1].1, buttons[2].1);
    assert!(summary.top() < activities.top() && activities.top() < settings.top(), "Summary, then Activities, then Settings");
    assert!(summary.top() < 100.0 && activities.top() < 150.0, "Summary and Activities sit at the top: {summary:?} {activities:?}");
    assert!(settings.bottom() > 700.0, "Settings sits at the bottom of the 800 px window: {settings:?}");
    assert_eq!((summary.left(), activities.left()), (settings.left(), settings.left()), "one column");
    assert!(summary.right() < 100.0, "a narrow rail on the left: {summary:?}");
}

#[test]
fn rail_shows_activities_with_badge() {
    // Two activities came with the last sync and are not opened yet: their number is on the rail.
    let now = TestConfig::default().now;
    let two_new = [
        listed(9_000_000_000_001, "strength_training", Some("Upper A"), now - chrono::TimeDelta::hours(2), true),
        listed(9_000_000_000_002, "running", None, now - chrono::TimeDelta::hours(5), true),
    ];
    let (app, _handles) = app_seeded(Lang::En, TestConfig::default(), true, |store| store_activities(store, &two_new));
    let mut harness = harness(app);
    let badge = |harness: &egui_kittest::Harness<'_, GymgarApp>| harness.get_by_role_and_label(Role::Button, "Activities").accesskit_node().value();
    assert_eq!(badge(&harness).as_deref(), Some("2"));

    // The user opens one of them (task 13 does it from the list; here through the engine): one is left.
    harness.state_mut().run("activity.markSeen", json!({ "id": 9_000_000_000_001_i64 })).expect("mark it seen");
    harness.run();
    assert_eq!(badge(&harness).as_deref(), Some("1"));
    harness.state_mut().run("activity.markSeen", json!({ "id": 9_000_000_000_002_i64 })).expect("mark it seen");
    harness.run();
    assert_eq!(badge(&harness), None, "no badge without new activities");
    // The section's name does not change with the count.
    harness.get_by_role_and_label(Role::Button, "Activities");
}

#[test]
fn the_rail_marks_the_current_section() {
    let (app, _handles) = app(TestConfig::default());
    let mut harness = harness(app);
    let toggled = |harness: &egui_kittest::Harness<'_, GymgarApp>, label: &str| {
        format!("{:?}", harness.get_by_role_and_label(Role::Button, label).accesskit_node().toggled())
    };
    assert_eq!((toggled(&harness, "Summary").as_str(), toggled(&harness, "Settings").as_str()), ("Some(True)", "Some(False)"));
    harness.get_by_role_and_label(Role::Button, "Settings").click();
    harness.run();
    assert_eq!((toggled(&harness, "Summary").as_str(), toggled(&harness, "Settings").as_str()), ("Some(False)", "Some(True)"));
}

#[test]
fn language_switch_relabels() {
    let (app, _handles) = app(TestConfig::default());
    let mut harness = harness(app);
    harness.get_by_role_and_label(Role::Button, "Summary");

    let outcome = harness.state_mut().run("settings.setLanguage", json!({ "language": "ru" }));
    assert!(matches!(outcome, Ok(Outcome::Done(_))), "{outcome:?}");
    let ctx = harness.ctx.clone();
    harness.state_mut().tick(&ctx);
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Сводка");
    harness.get_by_role_and_label(Role::Button, "Настройки");
    assert!(harness.query_by_label("Summary").is_none());

    harness.state_mut().run("settings.setLanguage", json!({ "language": "en" })).expect("switch back");
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Summary");
    assert!(harness.query_by_label("Сводка").is_none());
}

#[test]
fn the_language_applies_at_once_without_waiting_for_a_tick() {
    let (app, _handles) = app(TestConfig::default());
    let mut harness = harness(app);
    harness.state_mut().run("settings.setLanguage", json!({ "language": "ru" })).expect("set the language");
    assert_eq!(i18n::current(), Lang::Ru);
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Сводка");
}

#[test]
fn the_system_language_follows_the_system_locale() {
    for (locale, label) in [(Some("ru-RU"), "Сводка"), (Some("de-DE"), "Summary"), (None, "Summary")] {
        let (session, _handles) = test_session_with(TestConfig::default());
        let app = GymgarApp::with_system_locale(session, UiState::default(), locale.map(str::to_owned));
        let harness = harness(app);
        harness.get_by_role_and_label(Role::Button, label);
    }
}

#[test]
fn a_stored_language_is_in_effect_from_the_first_frame() {
    let (mut session, _handles) = test_session_with(TestConfig::default());
    session.execute("settings.setLanguage", json!({ "language": "ru" }), Caller::User).expect("set the language");
    let harness = harness(GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned())));
    harness.get_by_role_and_label(Role::Button, "Сводка");
    // An explicit choice beats the locale, in both directions.
    let (mut session, _handles) = test_session_with(TestConfig::default());
    session.execute("settings.setLanguage", json!({ "language": "en" }), Caller::User).expect("set the language");
    let harness = harness_with_locale(session, "ru-RU");
    harness.get_by_role_and_label(Role::Button, "Summary");
}

fn harness_with_locale(session: Session, locale: &str) -> egui_kittest::Harness<'static, GymgarApp> {
    harness(GymgarApp::with_system_locale(session, UiState::default(), Some(locale.to_owned())))
}

#[test]
fn another_process_changing_the_language_is_seen_within_a_second() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("gymgar.sqlite3");
    let other_process = Store::open(&path).expect("open the database");
    let (session, _handles) = test_session_on(Store::open(&path).expect("open the database"), TestConfig::default());
    let mut app = GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned()));
    let ctx = egui::Context::default();

    app.tick(&ctx);
    assert_eq!(i18n::current(), Lang::En);

    other_process.set_language(Language::Ru).expect("write the language");
    // The poll ran just now, so the next one is a second away...
    app.tick(&ctx);
    assert_eq!(i18n::current(), Lang::En, "the store was polled twice within the interval");
    // ...and when it is due, the change shows.
    app.next_store_poll = Instant::now();
    app.tick(&ctx);
    assert_eq!(i18n::current(), Lang::Ru);
}

#[test]
fn tick_asks_to_run_again_within_a_second_so_an_idle_window_keeps_polling() {
    let (mut app, _handles) = app(TestConfig::default());
    let ctx = egui::Context::default();
    let requested = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requested);
    ctx.set_request_repaint_callback(move |info| log.lock().expect("lock").push(info.delay));
    for _ in 0..3 {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.tick(ui.ctx()));
        output.textures_delta.clear();
    }
    assert!(requested.lock().expect("lock").contains(&Duration::from_secs(1)), "{requested:?}");
}

#[test]
fn startup_runs_reparse_then_sync_once() {
    // Ruling 4: at start, a connected app reads the stored files again (if a newer parser can), and syncs once that is
    // over. The two never overlap, and neither starts again.
    let (mut app, handles) = connected_app(config(Ok(()), true));
    handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(spawner.pending(), 1, "the re-parse, and nothing else yet");
    assert!(matches!(app.startup, Startup::Reparsing(_)), "{:?}", app.startup);
    assert!(!app.status().expect("status").sync.running, "the sync waits for the re-parse");
    assert!(!app.is_enabled("garmin.sync"), "the engine keeps them apart too");

    spawner.run_pending();
    app.tick(&ctx);
    assert!(app.status().expect("status").sync.running, "the sync starts as soon as the re-parse has ended");
    assert_eq!((spawner.pending(), app.startup), (1, Startup::Done));
    assert_eq!(handles.garmin.opens(), 0, "the key is read in the background job, not on the UI thread");

    spawner.run_pending();
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(spawner.pending(), 0, "once per run of the app");
    let status = app.status().expect("status");
    assert!(!status.sync.running);
    assert!(status.sync.last_success_at.is_some(), "{status:?}");
    // The sync checked the key itself (spec §2, item 1): there is no separate garmin.verify.
    assert_eq!((handles.session.count(RequestKind::Verify), handles.garmin.calls()), (1, 0));
    assert_eq!(handles.garmin.opened_key().as_deref(), Some("the-saved-key"));
    assert_eq!(status.status, ConnectionStatus::Connected);
    assert_eq!(app.error(), None, "nothing went wrong");

    // Nothing connected: nothing to re-parse or sync.
    let (mut empty, handles) = crate::test_support::app(config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    for _ in 0..3 {
        empty.tick(&ctx);
    }
    assert_eq!((spawner.pending(), handles.garmin.opens(), empty.startup), (0, 0, Startup::Done));
}

#[test]
fn startup_runs_the_sync_inline_too() {
    // The real runtime may finish the re-parse before the next frame: the sync still follows, once.
    let (mut app, handles) = connected_app(config(Ok(()), false));
    handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
    let ctx = egui::Context::default();
    for _ in 0..5 {
        app.tick(&ctx);
    }
    assert_eq!(handles.session.count(RequestKind::Verify), 1);
    assert!(app.status().expect("status").sync.last_success_at.is_some());
}

#[test]
fn a_startup_that_cannot_start_is_logged_not_shown() {
    // The re-parse cannot start while a sync the user asked for runs: the start-up goes on to its sync, which cannot
    // start either. Neither is the user's error.
    let (mut app, handles) = connected_app(config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    app.startup = Startup::Pending;
    let Ok(Outcome::Started(_)) = app.run("garmin.sync", Value::Null) else { panic!("garmin.sync did not start") };
    app.tick(&egui::Context::default());
    assert_eq!((spawner.pending(), app.startup), (1, Startup::Done));
    assert_eq!(app.error(), None);
}

/// A database file holding a saved connection, with the table of the connection renamed away so the status cannot be
/// read; `restore` puts it back. Returns the app, the doubles, the open directory and the path of the file.
fn app_with_an_unreadable_status() -> (GymgarApp, gymgar_engine::testing::TestHandles, tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("gymgar.sqlite3");
    let cfg = config(Ok(()), true);
    let (session, handles) = test_session_on(Store::open(&path).expect("open the database"), cfg.clone());
    save_connection(&session, &connection_at(cfg.now, SecretBackend::File));
    rusqlite::Connection::open(&path).expect("open the file").execute("ALTER TABLE garmin_connection RENAME TO hidden_connection", []).expect("rename");
    let app = GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned()));
    (app, handles, dir, path)
}

fn restore(path: &std::path::Path) {
    rusqlite::Connection::open(path).expect("open the file").execute("ALTER TABLE hidden_connection RENAME TO garmin_connection", []).expect("rename back");
}

#[test]
fn startup_waits_until_the_status_can_be_read() {
    let (mut app, handles, _dir, path) = app_with_an_unreadable_status();
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    assert!(app.status().is_none());
    // Not being able to read the status is no reason to forget the check: nothing is queued, and it is not marked done.
    for _ in 0..3 {
        app.next_store_poll = Instant::now();
        app.tick(&ctx);
    }
    assert!(app.status().is_none());
    assert_eq!(spawner.pending(), 0);

    // Once the status can be read, the import starts with the re-parse, once.
    restore(&path);
    for _ in 0..3 {
        app.next_store_poll = Instant::now();
        app.tick(&ctx);
    }
    assert_eq!(status_of(&app), ConnectionStatus::Connected);
    assert_eq!(spawner.pending(), 1);
}

#[test]
fn a_status_that_could_not_be_read_is_read_again_without_waiting_for_a_change() {
    // The database itself did not change (a lock that lasted too long, say), so nothing tells the window to look again;
    // it looks again by itself, at the pace of the poll.
    let (mut app, _handles) = connected_app(config(Ok(()), true));
    app.status = None;
    app.tick(&egui::Context::default());
    assert_eq!(status_of(&app), ConnectionStatus::Connected);
    assert_eq!(app.error(), None);
}

#[test]
fn tick_does_not_check_when_nothing_is_connected() {
    let (mut app, handles) = app(config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(spawner.pending(), 0);
    assert_eq!(handles.garmin.calls(), 0);
    assert_eq!(status_of(&app), ConnectionStatus::NotConnected);
}

#[test]
fn a_finished_check_that_failed_still_updates_the_status() {
    // The start-up sync finds no key in the store (Err(KeyMissing)): the status it recorded is what the UI must show.
    let (mut app, handles) = connected_app(config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    app.tick(&ctx);
    assert_eq!(status_of(&app), ConnectionStatus::Connected);

    // The re-parse ends, and the sync starts.
    spawner.run_pending();
    app.tick(&ctx);
    assert_eq!(status_of(&app), ConnectionStatus::Connected);
    spawner.run_pending();
    app.tick(&ctx);
    let status = app.status().expect("status");
    assert_eq!(status.status, ConnectionStatus::KeyMissing);
    assert_eq!(status.sync.last_error, Some(gymgar_domain::SyncErrorKind::KeyMissing));
    assert!(!status.check_running && !status.sync.running);
    assert_eq!(app.error(), None, "the list says why; it is not an error of the window");
}

#[test]
fn a_connect_job_that_succeeds_clears_the_draft_and_updates_the_status() {
    let (mut app, _handles) = app(config(Ok(()), false));
    app.ui.connect.key = "the-pasted-key".to_owned();
    app.ui.connect.replacing = true;
    let Ok(Outcome::Started(job)) = app.run("garmin.connect", json!({ "apiKey": "the-pasted-key", "keyTerm": 365 })) else { panic!("connect did not start") };
    app.ui.connect.pending = Some(job);
    assert!(app.status().expect("status").check_running, "the run refreshed the status");
    assert_eq!(status_of(&app), ConnectionStatus::NotConnected);

    app.tick(&egui::Context::default());
    assert_eq!(status_of(&app), ConnectionStatus::Connected);
    let form = &app.ui.connect;
    assert_eq!((form.pending, form.error.as_deref(), form.key.as_str(), form.replacing), (None, None, "", false));
}

#[test]
fn connecting_garmin_starts_the_first_import() {
    // Ruling 19: Garmin connected in the window loads the workouts by itself (spec §2, items 1 and 16), once.
    let (mut app, handles) = crate::test_support::app_in(Lang::En, config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    let Ok(Outcome::Started(job)) = app.run("garmin.connect", json!({ "apiKey": "the-pasted-key", "keyTerm": 365 })) else { panic!("connect did not start") };
    app.ui.connect.pending = Some(job);
    spawner.run_pending();
    app.tick(&ctx);
    assert_eq!(status_of(&app), ConnectionStatus::Connected);
    assert!(app.status().expect("status").sync.running, "the sync started with the connection");
    assert_eq!(spawner.pending(), 1, "the sync, and nothing else");
    spawner.run_pending();
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(spawner.pending(), 0, "once");
    let status = app.status().expect("status");
    assert!(!status.sync.running && status.sync.last_success_at.is_some(), "{status:?}");
    assert_eq!(handles.garmin.opened_key().as_deref(), Some("the-pasted-key"), "with the key just connected");

    // A key Garmin refused starts nothing.
    let (mut app, handles) = crate::test_support::app_in(Lang::En, config(Err(GarminAuthError::KeyRejected), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    let Ok(Outcome::Started(job)) = app.run("garmin.connect", json!({ "apiKey": "a-key-with-a-typo", "keyTerm": 365 })) else {
        panic!("connect did not start")
    };
    app.ui.connect.pending = Some(job);
    spawner.run_pending();
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(status_of(&app), ConnectionStatus::NotConnected);
    assert_eq!((spawner.pending(), app.status().expect("status").sync.running), (0, false));
    assert_eq!(handles.garmin.opens(), 0);
}

#[test]
fn a_key_replaced_while_the_files_are_read_again_leads_to_one_sync_after_them() {
    // The start-up re-parse runs when the user replaces the key: its sync and the connection's are the same one, and it
    // waits for the re-parse (the two never overlap).
    let (mut app, handles) = connected_app(config(Ok(()), true));
    handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    app.tick(&ctx);
    assert!(matches!(app.startup, Startup::Reparsing(_)), "{:?}", app.startup);
    let Ok(Outcome::Started(job)) = app.run("garmin.connect", json!({ "apiKey": "the-new-key", "keyTerm": 365 })) else { panic!("connect did not start") };
    app.ui.connect.pending = Some(job);
    assert_eq!(spawner.pending(), 2, "the re-parse and the key check");
    spawner.run_pending();
    app.tick(&ctx);
    assert_eq!(app.startup, Startup::Done);
    assert!(app.status().expect("status").sync.running);
    // The old key, in another store than the new one, is deleted by a job of its own; the sync is the other one.
    spawner.run_pending();
    for _ in 0..3 {
        app.tick(&ctx);
        spawner.run_pending();
    }
    assert_eq!(spawner.pending(), 0);
    // Every sync begins with a check of the key: one check, one sync.
    assert_eq!(handles.session.count(RequestKind::Verify), 1);
    assert!(app.status().expect("status").sync.last_success_at.is_some());
}

#[test]
fn a_connect_job_that_fails_shows_why_and_keeps_the_key_to_correct() {
    let (mut app, handles) = app(config(Err(GarminAuthError::KeyRejected), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    app.ui.connect.key = "a-key-with-a-typo".to_owned();
    let Ok(Outcome::Started(job)) = app.run("garmin.connect", json!({ "apiKey": "a-key-with-a-typo", "keyTerm": 365 })) else {
        panic!("connect did not start")
    };
    app.ui.connect.pending = Some(job);
    let ctx = egui::Context::default();

    // While the job runs the form stays pending.
    app.tick(&ctx);
    assert_eq!(app.ui.connect.pending, Some(job));
    assert!(app.status().expect("status").check_running);

    spawner.run_pending();
    app.tick(&ctx);
    let form = &app.ui.connect;
    assert_eq!(form.pending, None);
    assert_eq!(form.error.as_deref(), Some("Garmin did not accept the key. Check that it was copied in full, or create a new one."));
    assert_eq!(form.key, "a-key-with-a-typo");
    assert_eq!(status_of(&app), ConnectionStatus::NotConnected);
    assert!(!app.status().expect("status").check_running);
}

#[test]
fn a_connect_job_the_runtime_dropped_frees_the_form() {
    let (mut app, handles) = app(config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    app.ui.connect.key = "the-pasted-key".to_owned();
    let Ok(Outcome::Started(job)) = app.run("garmin.connect", json!({ "apiKey": "the-pasted-key", "keyTerm": 365 })) else { panic!("connect did not start") };
    app.ui.connect.pending = Some(job);
    let ctx = egui::Context::default();
    app.tick(&ctx);
    assert!(app.status().expect("status").check_running);

    // The runtime lets the job go without running it (it shut down, say): the form must not wait for it forever.
    spawner.drop_pending();
    app.tick(&ctx);
    let form = &app.ui.connect;
    assert_eq!(form.pending, None);
    assert_eq!(form.error.as_deref(), Some("Something went wrong. Please try again."));
    assert_eq!(form.key, "the-pasted-key", "the key stays to try again");
    assert!(!app.status().expect("status").check_running);
    assert!(app.is_enabled("garmin.connect"), "connect can be tried again");
    assert_eq!(handles.garmin.calls(), 0);
}

#[test]
fn a_job_nobody_is_waiting_for_leaves_the_form_alone() {
    let (mut app, _handles) = connected_app(config(Ok(()), false));
    app.ui.connect.key = "half-typed".to_owned();
    app.ui.connect.error = Some("an old error".to_owned());
    // The start-up check finishes while the user types.
    app.tick(&egui::Context::default());
    app.tick(&egui::Context::default());
    assert_eq!(app.ui.connect.key, "half-typed");
    assert_eq!(app.ui.connect.error.as_deref(), Some("an old error"));
}

#[test]
fn a_frame_that_applied_only_partial_results_refreshes_the_status() {
    // Fifteen strength workouts whose files Garmin garbles: the sync sends more results than one frame applies.
    let garmin = Arc::new(FakeGarminSession::new());
    let items: Vec<Value> = (1..=15_i64)
        .map(|n| json!({ "activityId": n, "activityType": { "typeKey": "strength_training" }, "startTimeGMT": format!("2026-10-{n:02} 10:00:00") }))
        .collect();
    garmin.set_pages(vec![Value::Array(items).to_string().into_bytes()]);
    for n in 1..=15 {
        garmin.set_file(ActivityId(n), b"not a zip".to_vec());
    }
    let (mut app, handles) = connected_app(TestConfig { session: Some(garmin), ..config(Ok(()), true) });
    handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
    // The start-up import is over, so this sync is the only job.
    app.startup = Startup::Done;
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    let Ok(Outcome::Started(_)) = app.run("garmin.sync", Value::Null) else { panic!("garmin.sync did not start") };
    assert!(app.status().expect("status").sync.running, "the run refreshed the status");

    // The job runs to its end; the window takes what it sent over the next frames.
    spawner.run_pending();
    app.tick(&ctx);
    let sync = &app.status().expect("status").sync;
    assert!(sync.running, "the end of the sync has not been applied yet");
    assert!(sync.files_done > 0 && sync.files_done < sync.files_total, "the partial results were applied and the status read again: {sync:?}");

    app.tick(&ctx);
    let sync = &app.status().expect("status").sync;
    assert!(!sync.running);
    assert!(sync.last_success_at.is_some());
}

#[test]
fn run_refreshes_the_status_after_a_change() {
    let (mut app, _handles) = connected_app(config(Ok(()), false));
    assert!(!app.status().expect("status").check_running);
    // A query needs no refresh (and must not start one: refreshing is itself a query).
    assert!(matches!(app.run("garmin.status", Value::Null), Ok(Outcome::Done(_))));
    assert!(matches!(app.run("garmin.verify", Value::Null), Ok(Outcome::Started(_))));
    assert!(app.status().expect("status").check_running, "the running check shows without waiting for a tick");
}

/// [`connected_app`] whose start-up import is over, so the jobs a test starts are the only ones.
fn settled_app(cfg: TestConfig) -> (GymgarApp, gymgar_engine::testing::TestHandles) {
    let (mut app, handles) = connected_app(cfg);
    app.startup = Startup::Done;
    (app, handles)
}

/// A file database with a saved connection and its key, whose activity table refuses every write from the moment
/// `break_writes` is called: what a full disk or a lock that lasts too long does to a sync.
fn app_whose_writes_fail(pages: usize) -> (GymgarApp, gymgar_engine::testing::TestHandles, tempfile::TempDir, impl Fn()) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("gymgar.sqlite3");
    let garmin = Arc::new(FakeGarminSession::new());
    // One run per page: each page is a partial result of its own.
    let page = |n: usize| json!([{ "activityId": n, "activityType": { "typeKey": "running" }, "startTimeGMT": format!("2026-10-{:02} 10:00:00", n % 28 + 1) }]);
    garmin.set_pages((1..=pages).map(|n| page(n).to_string().into_bytes()).collect());
    let cfg = TestConfig { session: Some(garmin), ..config(Ok(()), true) };
    let (session, handles) = test_session_on(Store::open(&path).expect("open the database"), cfg.clone());
    save_connection(&session, &connection_at(cfg.now, SecretBackend::File));
    handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
    let mut app = GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned()));
    app.startup = Startup::Done;
    let break_writes = move || {
        rusqlite::Connection::open(&path)
            .expect("open the file")
            .execute_batch("CREATE TRIGGER no_writes BEFORE INSERT ON activity BEGIN SELECT RAISE(ABORT, 'the disk is full'); END;")
            .expect("add the trigger");
    };
    (app, handles, dir, break_writes)
}

#[test]
fn a_partial_result_that_fails_is_reported_once_per_sync() {
    // Ruling 13: a page of the sync that cannot be written is shown in the banner (with the text for its kind of error),
    // once per run however many pages fail, and the sync's own line says what it means.
    let (mut app, handles, _dir, break_writes) = app_whose_writes_fail(40);
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    let Ok(Outcome::Started(_)) = app.run("garmin.sync", Value::Null) else { panic!("garmin.sync did not start") };
    spawner.run_pending();
    break_writes();
    let store_text = "Gymgar could not access its data. Check that the disk is available and try again.";

    // The first frame applies a few dozen of the 40 pages, all refused.
    app.tick(&ctx);
    assert!(app.status().expect("status").sync.running, "more pages wait for the next frame");
    assert_eq!(app.error(), Some(store_text));
    // Whatever the next frames bring of the same run, the banner is not raised again.
    app.reported = None;
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(app.error(), None, "once per sync");
    let sync = &app.status().expect("status").sync;
    assert!(!sync.running);
    assert_eq!(sync.last_error, Some(gymgar_domain::SyncErrorKind::Storage));

    // The next run that fails is reported again.
    let Ok(Outcome::Started(_)) = app.run("garmin.sync", Value::Null) else { panic!("garmin.sync did not start") };
    spawner.run_pending();
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(app.error(), Some(store_text));
}

#[test]
fn a_reported_failure_stays_until_the_user_does_something_that_works() {
    let (mut app, _handles) = settled_app(config(Ok(()), false));
    app.report("Something went wrong. Please try again.");
    // Refreshes come and go (a job ended, another process wrote): the note stays.
    app.next_store_poll = Instant::now();
    app.status = None;
    app.tick(&egui::Context::default());
    assert_eq!(app.error(), Some("Something went wrong. Please try again."));
    // The user changes something, and it works: the note is old news.
    app.run("settings.setLanguage", json!({ "language": "en" })).expect("set the language");
    assert_eq!(app.error(), None);
}

#[test]
fn a_disconnect_job_that_succeeds_clears_the_card() {
    let (mut app, handles) = settled_app(config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    app.ui.connect.replacing = true;
    app.ui.connect.error = Some("an old error".to_owned());
    let Ok(Outcome::Started(job)) = app.run("garmin.disconnect", Value::Null) else { panic!("disconnect did not start") };
    app.ui.connect.disconnecting = Some(job);
    let ctx = egui::Context::default();

    // While the job runs, the card waits for it.
    app.tick(&ctx);
    assert_eq!(app.ui.connect.disconnecting, Some(job));
    assert_eq!(status_of(&app), ConnectionStatus::Connected);

    spawner.run_pending();
    app.tick(&ctx);
    let form = &app.ui.connect;
    assert_eq!((form.disconnecting, form.error.as_deref(), form.replacing), (None, None, false));
    assert_eq!(status_of(&app), ConnectionStatus::NotConnected);
}

#[test]
fn a_disconnect_job_that_fails_or_is_dropped_says_so_and_can_be_tried_again() {
    for (dropped, text) in
        [(false, "Gymgar could not use the password store. Check that it is unlocked and try again."), (true, "Something went wrong. Please try again.")]
    {
        let (mut app, handles) = settled_app(config(Ok(()), true));
        let spawner = handles.spawner.expect("a deferred spawner");
        handles.file.set_failing(true);
        let Ok(Outcome::Started(job)) = app.run("garmin.disconnect", Value::Null) else { panic!("disconnect did not start") };
        app.ui.connect.disconnecting = Some(job);
        if dropped {
            spawner.drop_pending();
        } else {
            spawner.run_pending();
        }
        app.tick(&egui::Context::default());

        assert_eq!(app.ui.connect.disconnecting, None, "dropped: {dropped}");
        assert_eq!(app.ui.connect.error.as_deref(), Some(text));
        assert_eq!(status_of(&app), ConnectionStatus::Connected, "dropped: {dropped}");
        handles.file.set_failing(false);
        assert!(app.is_enabled("garmin.disconnect"), "dropped: {dropped}");
    }
}

#[test]
fn run_turns_an_engine_error_into_text_for_the_user() {
    let (mut app, _handles) = app(TestConfig::default());
    let unknown = app.run("garmin.teleport", Value::Null).expect_err("no such command");
    assert_eq!(unknown, "Something went wrong. Please try again.");
    let bad = app.run("settings.setLanguage", json!({ "language": "de" })).expect_err("bad params");
    assert_eq!(bad, "The value is not valid. Check it and try again.");
    let disabled = app.run("garmin.disconnect", Value::Null).expect_err("nothing to disconnect");
    assert_eq!(disabled, "This is not available right now.");

    i18n::set_current(Lang::Ru);
    let unknown = app.run("garmin.teleport", Value::Null).expect_err("no such command");
    assert_eq!(unknown, "Что-то пошло не так. Попробуйте ещё раз.");
}

fn every_error() -> Vec<EngineError> {
    vec![
        EngineError::UnknownCommand("x.y".into()),
        EngineError::BadParams { cmd: "garmin.connect".into(), msg: "RAW-ENGINE-TEXT".into() },
        EngineError::AgentNotAllowed("garmin.connect".into()),
        EngineError::Disabled { cmd: "garmin.verify".into(), reason: "RAW-ENGINE-TEXT".into() },
        EngineError::Store("RAW-ENGINE-TEXT".into()),
        EngineError::Secrets("RAW-ENGINE-TEXT".into()),
        EngineError::Garmin(GarminAuthError::KeyRejected),
        EngineError::Garmin(GarminAuthError::RateLimited),
        EngineError::Garmin(GarminAuthError::Unavailable),
        EngineError::Garmin(GarminAuthError::Network),
        EngineError::Garmin(GarminAuthError::Unexpected),
        EngineError::Garmin(GarminAuthError::NotFound),
        EngineError::FileStoreConsentRequired,
        EngineError::NotConnected,
        EngineError::KeyMissing,
        EngineError::JobPanicked("garmin.connect".into()),
        EngineError::JobDropped("garmin.connect".into()),
        EngineError::CommandPanicked("garmin.status".into()),
    ]
}

#[test]
fn the_texts_for_the_garmin_failures_are_the_ones_from_the_spec() {
    let texts = |lang| {
        i18n::set_current(lang);
        [GarminAuthError::KeyRejected, GarminAuthError::Network, GarminAuthError::RateLimited, GarminAuthError::Unavailable, GarminAuthError::Unexpected]
            .map(|error| error_text(&EngineError::Garmin(error)))
    };
    assert_eq!(
        texts(Lang::En),
        [
            "Garmin did not accept the key. Check that it was copied in full, or create a new one.",
            "Could not reach Garmin. Check your internet connection.",
            "Garmin has temporarily limited requests. Try again in a few minutes.",
            "Garmin is unavailable right now. Try again later.",
            "Garmin is unavailable right now. Try again later.",
        ]
    );
    assert_eq!(
        texts(Lang::Ru),
        [
            "Garmin не принял ключ. Проверьте, что он скопирован целиком, или создайте новый.",
            "Не удалось связаться с Garmin. Проверьте подключение к интернету.",
            "Garmin временно ограничил запросы. Попробуйте через несколько минут.",
            "Garmin сейчас недоступен. Попробуйте позже.",
            "Garmin сейчас недоступен. Попробуйте позже.",
        ]
    );
}

#[test]
fn every_error_has_a_translated_text_that_leaks_no_engine_detail() {
    for lang in Lang::ALL {
        i18n::set_current(lang);
        for error in every_error() {
            let text = error_text(&error);
            assert!(!text.is_empty(), "{error:?}");
            // The engine's English text (and anything it echoes, such as rejected params) never reaches the user.
            assert!(!text.contains("RAW-ENGINE-TEXT"), "{error:?}: {text}");
            if lang == Lang::Ru {
                assert!(text.chars().any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap_or(' '))), "{error:?} has no Russian text: {text}");
            }
        }
    }
}

#[test]
fn a_database_that_cannot_be_read_is_reported_in_the_window() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("gymgar.sqlite3");
    let session = test_session_on(Store::open(&path).expect("open the database"), TestConfig::default()).0;
    let mut harness = harness(GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned())));
    assert_eq!(harness.state().error(), None);
    assert!(harness.query_by_label_contains("could not access").is_none());

    // Another process damages the database (the table of the connection disappears).
    rusqlite::Connection::open(&path).expect("open the file").execute("DROP TABLE garmin_connection", []).expect("drop the table");
    harness.state_mut().next_store_poll = Instant::now();
    let ctx = harness.ctx.clone();
    harness.state_mut().tick(&ctx);
    harness.run();

    assert_eq!(harness.state().error(), Some("Gymgar could not access its data. Check that the disk is available and try again."));
    harness.get_by_label("Gymgar could not access its data. Check that the disk is available and try again.");
    // The last known status stays, and the rail still works.
    assert!(harness.state().status().is_some());
    harness.get_by_role_and_label(Role::Button, "Settings").click();
    harness.run();
    assert_eq!(harness.state().ui.section, Section::Settings);
}

#[test]
fn a_screen_is_never_wider_than_a_small_window() {
    // The smallest window, 800 points: the content stops at its right edge, not at the width a large window allows. A
    // long text that wraps shows where that is: the warning before the key goes to a file.
    let (app, _handles) = app(TestConfig { system_store: false, ..TestConfig::default() });
    let mut harness = crate::test_support::harness_sized(app, egui::vec2(800.0, 500.0));
    harness.state_mut().ui.section = Section::Settings;
    harness.run();
    let warning = harness.get_by_label_contains("Save the key to the file").rect();
    assert!(warning.right() <= 800.0 - f32::from(metrics::SCREEN_MARGIN), "the warning runs past the window: {warning:?}");
}
