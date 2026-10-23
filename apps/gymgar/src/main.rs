//! Gymgar desktop app: opens the window and hands it to the UI shell.

use std::sync::Arc;

use gymgar_engine::{Services, Session};
use gymgar_store::Store;
use gymgar_ui_egui::{GymgarApp, UiState, icons, theme};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gymgar")
            .with_app_id("gymgar")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Gymgar",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx, &theme::Tokens::SPORT);
            icons::install(&cc.egui_ctx);
            // A stand-in until the app builds the real services: a database in memory and nothing that reaches outside it.
            let repaint = cc.egui_ctx.clone();
            let services =
                Services { secrets: None, garmin: None, spawner: None, notify: Arc::new(move || repaint.request_repaint()), clock: Arc::new(chrono::Utc::now) };
            let session = Session::new(Store::open_in_memory()?, services);
            Ok(Box::new(GymgarApp::new(session, UiState::load(cc.storage))))
        }),
    )
}
