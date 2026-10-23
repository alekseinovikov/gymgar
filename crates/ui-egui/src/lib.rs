//! Gymgar's UI shell, built on egui/eframe.
//!
//! L4 and deliberately thin: screens render state and dispatch engine commands; they hold no
//! business logic. Colours, radii and spacing come from the theme, never hard-coded in a widget.

/// The desktop application: owns the engine session and the UI state.
#[derive(Debug, Default)]
pub struct GymgarApp {}

impl GymgarApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    /// One frame of the whole interface. [`eframe::App::ui`] and the UI tests both call this.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Gymgar");
        });
    }
}

impl eframe::App for GymgarApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::GymgarApp;

    #[test]
    fn shows_app_title() {
        let mut harness = Harness::new_ui_state(|ui, app: &mut GymgarApp| app.show(ui), GymgarApp::default());
        harness.run();
        harness.get_by_label("Gymgar");
    }
}
