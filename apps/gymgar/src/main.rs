//! Gymgar desktop app: opens the window and hands it to the UI shell.

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gymgar")
            .with_app_id("gymgar")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native("Gymgar", options, Box::new(|cc| Ok(Box::new(gymgar_ui_egui::GymgarApp::new(cc)))))
}
