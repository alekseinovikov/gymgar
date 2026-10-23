//! The Settings screen: the Garmin connection and the interface language.

use egui::{Id, Ui};
use gymgar_domain::Language;
use serde_json::Map;
use serde_json::Value;

use crate::theme::metrics;
use crate::{GymgarApp, garmin_ui, tl, widgets};

/// The names of the languages in themselves.
const ENGLISH: &str = "English";
const RUSSIAN: &str = "Русский";

/// Draws the Settings screen.
pub(crate) fn show(app: &mut GymgarApp, ui: &mut Ui) {
    widgets::page_title(ui, tl!("Settings"), None);
    ui.add_space(metrics::GAP);
    garmin_ui::show(app, ui);
    ui.add_space(metrics::GAP);
    language_card(app, ui);
}

/// The card with the list of languages. A choice is applied at once, with no restart: the engine keeps it and the
/// window is drawn again in the new language.
fn language_card(app: &mut GymgarApp, ui: &mut Ui) {
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::card_title(ui, tl!("Interface language"));
            ui.add_space(metrics::GAP);
            let mut choice = app.language();
            // Each language is named in itself (its endonym), in every interface language, so a user who cannot read the
            // current one still finds their own. Being the same everywhere, the names are not translated.
            let choices = [(Language::System, tl!("System")), (Language::En, ENGLISH), (Language::Ru, RUSSIAN)];
            if widgets::dropdown(ui, Id::unique("interface-language"), &mut choice, &choices) {
                let params = Map::from_iter([("language".to_owned(), Value::from(choice.as_str()))]);
                match app.run("settings.setLanguage", Value::Object(params)) {
                    // Whatever was drawn after the change in this frame is in one language and the rest in the other.
                    Ok(_) => ui.ctx().request_repaint(),
                    Err(text) => app.report(&text),
                }
            }
        });
    });
}

#[cfg(test)]
#[path = "settings_ui_tests.rs"]
mod tests;
