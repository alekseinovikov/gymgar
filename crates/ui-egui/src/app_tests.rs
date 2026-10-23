use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use gymgar_domain::{ConnectionStatus, Language, SecretBackend};
use gymgar_engine::testing::{TestConfig, connection_at, save_connection, test_session_on, test_session_with};
use gymgar_engine::{Caller, EngineError, GarminAuthError, Outcome};
use gymgar_store::Store;
use serde_json::{Value, json};

use super::*;
use crate::Section;
use crate::i18n::{self, Lang};
use crate::test_support::{TEST_LOCALE, app, harness};

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

    harness.get_by_label("Settings").click();
    harness.run();
    assert_eq!(harness.state().ui.section, Section::Settings);

    harness.get_by_label("Summary").click();
    harness.run();
    assert_eq!(harness.state().ui.section, Section::Summary);
}

#[test]
fn the_rail_has_only_two_buttons_summary_on_top_and_settings_at_the_bottom() {
    let (app, _handles) = app(TestConfig::default());
    let harness = harness(app);
    let buttons: Vec<_> = harness.get_all_by_role(Role::Button).map(|button| (button.accesskit_node().label().unwrap_or_default(), button.rect())).collect();
    let labels: Vec<_> = buttons.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, ["Summary", "Settings"], "{buttons:?}");
    let (summary, settings) = (buttons[0].1, buttons[1].1);
    assert!(summary.top() < settings.top(), "Summary is above Settings");
    assert!(summary.top() < 100.0, "Summary sits at the top: {summary:?}");
    assert!(settings.bottom() > 700.0, "Settings sits at the bottom of the 800 px window: {settings:?}");
    assert_eq!(summary.left(), settings.left(), "one column");
    assert!(summary.right() < 100.0, "a narrow rail on the left: {summary:?}");
}

#[test]
fn the_rail_marks_the_current_section() {
    let (app, _handles) = app(TestConfig::default());
    let mut harness = harness(app);
    let toggled = |harness: &egui_kittest::Harness<'_, GymgarApp>, label: &str| format!("{:?}", harness.get_by_label(label).accesskit_node().toggled());
    assert_eq!((toggled(&harness, "Summary").as_str(), toggled(&harness, "Settings").as_str()), ("Some(True)", "Some(False)"));
    harness.get_by_label("Settings").click();
    harness.run();
    assert_eq!((toggled(&harness, "Summary").as_str(), toggled(&harness, "Settings").as_str()), ("Some(False)", "Some(True)"));
}

#[test]
fn language_switch_relabels() {
    let (app, _handles) = app(TestConfig::default());
    let mut harness = harness(app);
    harness.get_by_label("Summary");

    let outcome = harness.state_mut().run("settings.setLanguage", json!({ "language": "ru" }));
    assert!(matches!(outcome, Ok(Outcome::Done(_))), "{outcome:?}");
    let ctx = harness.ctx.clone();
    harness.state_mut().tick(&ctx);
    harness.run();
    harness.get_by_label("Сводка");
    harness.get_by_label("Настройки");
    assert!(harness.query_by_label("Summary").is_none());

    harness.state_mut().run("settings.setLanguage", json!({ "language": "en" })).expect("switch back");
    harness.run();
    harness.get_by_label("Summary");
    assert!(harness.query_by_label("Сводка").is_none());
}

#[test]
fn the_language_applies_at_once_without_waiting_for_a_tick() {
    let (app, _handles) = app(TestConfig::default());
    let mut harness = harness(app);
    harness.state_mut().run("settings.setLanguage", json!({ "language": "ru" })).expect("set the language");
    assert_eq!(i18n::current(), Lang::Ru);
    harness.run();
    harness.get_by_label("Сводка");
}

#[test]
fn the_system_language_follows_the_system_locale() {
    for (locale, label) in [(Some("ru-RU"), "Сводка"), (Some("de-DE"), "Summary"), (None, "Summary")] {
        let (session, _handles) = test_session_with(TestConfig::default());
        let app = GymgarApp::with_system_locale(session, UiState::default(), locale.map(str::to_owned));
        let harness = harness(app);
        harness.get_by_label(label);
    }
}

#[test]
fn a_stored_language_is_in_effect_from_the_first_frame() {
    let (mut session, _handles) = test_session_with(TestConfig::default());
    session.execute("settings.setLanguage", json!({ "language": "ru" }), Caller::User).expect("set the language");
    let harness = harness(GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned())));
    harness.get_by_label("Сводка");
    // An explicit choice beats the locale, in both directions.
    let (mut session, _handles) = test_session_with(TestConfig::default());
    session.execute("settings.setLanguage", json!({ "language": "en" }), Caller::User).expect("set the language");
    let harness = harness_with_locale(session, "ru-RU");
    harness.get_by_label("Summary");
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
fn tick_checks_a_saved_connection_once_at_start() {
    let (mut app, handles) = connected_app(config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(spawner.pending(), 1, "exactly one garmin.verify job");
    assert!(app.status().expect("status").check_running);

    // Once it has run, it is not started again.
    spawner.run_pending();
    for _ in 0..3 {
        app.tick(&ctx);
    }
    assert_eq!(spawner.pending(), 0);
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
    // The job answers Err(KeyMissing) (no key in the store), yet the status it recorded is what the UI must show.
    let (mut app, handles) = connected_app(config(Ok(()), true));
    let spawner = handles.spawner.expect("a deferred spawner");
    let ctx = egui::Context::default();
    app.tick(&ctx);
    assert_eq!(status_of(&app), ConnectionStatus::Connected);

    spawner.run_pending();
    app.tick(&ctx);
    let status = app.status().expect("status");
    assert_eq!(status.status, ConnectionStatus::KeyMissing);
    assert!(!status.check_running);
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
fn run_refreshes_the_status_after_a_change() {
    let (mut app, _handles) = connected_app(config(Ok(()), false));
    assert_eq!(status_of(&app), ConnectionStatus::Connected);
    // A query needs no refresh (and must not start one: refreshing is itself a query).
    assert!(matches!(app.run("garmin.status", Value::Null), Ok(Outcome::Done(_))));
    assert!(matches!(app.run("garmin.disconnect", Value::Null), Ok(Outcome::Done(_))));
    assert_eq!(status_of(&app), ConnectionStatus::NotConnected, "disconnect shows without waiting for a tick");
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
        EngineError::FileStoreConsentRequired,
        EngineError::NotConnected,
        EngineError::KeyMissing,
        EngineError::JobPanicked("garmin.connect".into()),
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
    harness.get_by_label("Settings").click();
    harness.run();
    assert_eq!(harness.state().ui.section, Section::Settings);
}
