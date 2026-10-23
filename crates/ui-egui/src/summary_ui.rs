//! The Summary screen: what the app has for the user. In project 1 that is the invitation to connect Garmin, and once
//! connected, a note about what comes next. A connection that needs attention (a key that stopped working, or went
//! missing) is said so here too, with a way to the fix.

use egui::Ui;
use gymgar_domain::ConnectionStatus;

use crate::state::Section;
use crate::theme::{Tokens, metrics};
use crate::{GymgarApp, garmin_ui, tl, widgets};

/// Draws the Summary screen.
pub(crate) fn show(app: &mut GymgarApp, ui: &mut Ui) {
    widgets::page_title(ui, tl!("Summary"), Some(tl!("Workouts, progress and recovery will appear here")));
    ui.add_space(metrics::GAP);
    let Some(status) = app.status().cloned() else { return };
    match status.status {
        ConnectionStatus::NotConnected => invitation(app, ui),
        ConnectionStatus::Connected | ConnectionStatus::Unverified => {
            connected(ui);
            if status.expiry_warning {
                ui.add_space(metrics::GAP);
                garmin_ui::expiry_warning(app, ui, true);
            }
        }
        ConnectionStatus::KeyRejected => needs_attention(app, ui, tl!("Key not valid"), None, tl!("Replace key")),
        ConnectionStatus::KeyMissing => needs_attention(app, ui, tl!("Key not found"), Some(tl!("Connect Garmin again")), tl!("Connect Garmin")),
    }
}

/// The first run: one card that leads to Settings.
fn invitation(app: &mut GymgarApp, ui: &mut Ui) {
    ui.scope(|ui| {
        ui.set_max_width(metrics::INVITATION_WIDTH);
        widgets::card(ui, |ui| {
            widgets::card_title(ui, tl!("Connect to Garmin Connect"));
            ui.add_space(metrics::GAP / 2.0);
            widgets::hint(ui, tl!("Gymgar takes your workouts, sleep and HRV from Garmin. You need a personal API key, which takes a couple of minutes."));
            ui.add_space(metrics::GAP);
            if widgets::primary_button(ui, tl!("Connect Garmin")).clicked() {
                app.ui.section = Section::Settings;
            }
        });
    });
}

/// A connection that works. Loading the workouts is the next project, and the card says so.
fn connected(ui: &mut Ui) {
    widgets::card(ui, |ui| {
        widgets::card_title(ui, tl!("Garmin Connect"));
        ui.add_space(metrics::GAP / 2.0);
        ui.label(tl!("Garmin is connected. Loading workouts will arrive in the next update."));
    });
}

/// A connection whose key does not work or is gone: the state and a button to the place where it is fixed.
fn needs_attention(app: &mut GymgarApp, ui: &mut Ui, state: &str, hint: Option<&str>, button: &str) {
    let t = Tokens::get(ui.ctx());
    ui.scope(|ui| {
        ui.set_max_width(metrics::INVITATION_WIDTH);
        widgets::card(ui, |ui| {
            widgets::card_title(ui, tl!("Garmin Connect"));
            ui.add_space(metrics::GAP / 2.0);
            widgets::status_dot(ui, t.warning, state);
            if let Some(hint) = hint {
                widgets::hint(ui, hint);
            }
            ui.add_space(metrics::GAP);
            if widgets::primary_button(ui, button).clicked() {
                garmin_ui::replace_key(app);
            }
        });
    });
}

#[cfg(test)]
#[path = "summary_ui_tests.rs"]
mod tests;
