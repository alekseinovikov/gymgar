//! Helpers the UI tests share: a headless harness with the Sport theme and the icons installed, an app on the engine's
//! test doubles, and the few gestures every screen test makes. Compiled for tests only.

use chrono::FixedOffset;
use egui::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use gymgar_domain::{GarminConnection, SecretBackend};
use gymgar_engine::Outcome;
use gymgar_engine::testing::{TestConfig, TestHandles, connection_at, save_connection, test_session_with};
use serde_json::json;

use crate::i18n::Lang;
use crate::theme::{self, Tokens};
use crate::{GymgarApp, Section, UiState, icons};

/// The size of the test window: the app's start size.
pub(crate) const WINDOW: egui::Vec2 = egui::vec2(1200.0, 800.0);

/// The locale the test apps report as the system's. Tests must not depend on the language of the machine they run on.
pub(crate) const TEST_LOCALE: &str = "en-US";

/// A harness builder with the test window's size, styled and ready for icons as the real window is.
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

/// An app on a fresh in-memory database and the doubles from `cfg`, with the interface language "system" on an
/// English machine. Dates are shown in UTC. It has not ticked yet, so a connection it finds is checked at the first tick.
pub(crate) fn app(cfg: TestConfig) -> (GymgarApp, TestHandles) {
    app_on_machine(TEST_LOCALE, cfg, None, false)
}

/// Like [`app`], with the interface language "system" on a machine in `lang`, and a window that has been open a while:
/// the check that the app makes of a saved connection when it starts is over (there was none to check).
pub(crate) fn app_in(lang: Lang, cfg: TestConfig) -> (GymgarApp, TestHandles) {
    app_on_machine(locale(lang), cfg, None, true)
}

/// Like [`app_in`], with `connection` already saved in the database, as an earlier run of the app would have left it.
/// The start-up check is over: this window has been open a while, and its ticks start nothing by themselves.
pub(crate) fn app_connected(lang: Lang, cfg: TestConfig, connection: &GarminConnection) -> (GymgarApp, TestHandles) {
    app_on_machine(locale(lang), cfg, Some(connection), true)
}

/// Like [`app_connected`], for the moment the app has just been opened: its first tick checks the saved key.
pub(crate) fn app_just_opened(lang: Lang, cfg: TestConfig, connection: &GarminConnection) -> (GymgarApp, TestHandles) {
    app_on_machine(locale(lang), cfg, Some(connection), false)
}

/// Like [`app_in`], connected the way the user does it: `key` was entered and Garmin accepted it, so the key is in the
/// secret store the rules choose. Needs `cfg.deferred` off, so the check runs at once.
pub(crate) fn app_with_key(lang: Lang, cfg: TestConfig, key: &str) -> (GymgarApp, TestHandles) {
    let (mut app, handles) = app_in(lang, cfg);
    let started = app.run("garmin.connect", json!({ "apiKey": key, "keyTerm": 365, "allowFileStore": true }));
    let Ok(Outcome::Started(job)) = started else { panic!("connecting did not start: {started:?}") };
    app.ui.connect.pending = Some(job);
    app.tick(&egui::Context::default());
    assert!(app.status().is_some_and(|status| status.status == gymgar_domain::ConnectionStatus::Connected), "Garmin did not accept the key");
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
    let (session, handles) = test_session_with(cfg);
    if let Some(connection) = connection {
        save_connection(&session, connection);
    }
    let utc = FixedOffset::east_opt(0).expect("UTC is a valid offset");
    let mut app = GymgarApp::with_system_locale(session, UiState::default(), Some(locale.to_owned())).with_utc_offset(utc);
    app.verify_started = started;
    (app, handles)
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
