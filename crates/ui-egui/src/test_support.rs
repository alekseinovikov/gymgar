//! Helpers the UI tests share: a headless harness with the Sport theme and the icons installed, and an app on the
//! engine's test doubles. Compiled for tests only.

use egui_kittest::Harness;
use gymgar_engine::testing::{TestConfig, TestHandles, test_session_with};

use crate::theme::{self, Tokens};
use crate::{GymgarApp, UiState, icons};

/// The size of the test window: the app's start size.
pub(crate) const WINDOW: egui::Vec2 = egui::vec2(1200.0, 800.0);

/// The locale the test apps report as the system's. Tests must not depend on the language of the machine they run on.
pub(crate) const TEST_LOCALE: &str = "en-US";

/// A headless harness running `add` with `state`, styled and ready for icons as the real window is.
pub(crate) fn ui_harness<'a, S>(add: impl FnMut(&mut egui::Ui, &mut S) + 'a, state: S) -> Harness<'a, S> {
    Harness::builder()
        .with_size(WINDOW)
        .with_context_setup(|ctx| {
            theme::apply(ctx, &Tokens::SPORT);
            icons::install(ctx);
        })
        .build_ui_state(add, state)
}

/// A harness showing the whole app.
pub(crate) fn harness(app: GymgarApp) -> Harness<'static, GymgarApp> {
    ui_harness(|ui, app: &mut GymgarApp| app.show(ui), app)
}

/// An app on a fresh in-memory database and the doubles from `cfg`, with the interface language "system" on an
/// English machine.
pub(crate) fn app(cfg: TestConfig) -> (GymgarApp, TestHandles) {
    let (session, handles) = test_session_with(cfg);
    (GymgarApp::with_system_locale(session, UiState::default(), Some(TEST_LOCALE.to_owned())), handles)
}
