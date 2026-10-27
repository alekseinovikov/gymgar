//! The Summary screen: what the app has for the user. Before a connection, the invitation to connect Garmin. With one,
//! the activities Garmin sent that the user has not opened yet, the last strength workout, the connection and how its
//! import stands (activity import spec §2, item 15). A connection that needs attention (a key that stopped working, or
//! went missing) is said so here too, with a way to the fix.

use egui::{RichText, Ui};
use gymgar_domain::{ConnectionStatus, STRENGTH_TYPE_KEY};
use gymgar_engine::{ActivityListItemView, GarminStatusView};

use crate::activities_ui;
use crate::i18n::trn;
use crate::state::{ListFilter, Section};
use crate::theme::{Tokens, metrics};
use crate::{GymgarApp, garmin_ui, tl, widgets};

/// Draws the Summary screen.
pub(crate) fn show(app: &mut GymgarApp, ui: &mut Ui) {
    widgets::page_title(ui, tl!("Summary"), Some(tl!("Workouts, progress and recovery will appear here")));
    ui.add_space(metrics::GAP);
    let Some(status) = app.status().cloned() else { return };
    if status.status == ConnectionStatus::NotConnected {
        invitation(app, ui);
        return;
    }
    // What was loaded is shown whatever the key's state: it stays the user's.
    if status.sync.new_count > 0 {
        new_card(app, ui, status.sync.new_count);
        ui.add_space(metrics::GAP);
    }
    if let Some(item) = app.activities().iter().find(|item| item.type_key == STRENGTH_TYPE_KEY && !item.excluded).cloned() {
        last_strength_card(app, ui, &item);
        ui.add_space(metrics::GAP);
    }
    match status.status {
        ConnectionStatus::NotConnected => {}
        ConnectionStatus::Connected | ConnectionStatus::Unverified => {
            if status.expiry_warning {
                garmin_ui::expiry_warning(app, ui, true);
                ui.add_space(metrics::GAP);
            }
            connected(app, ui, &status);
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

/// "N new workouts", with the button that opens the list on them.
fn new_card(app: &mut GymgarApp, ui: &mut Ui, count: u32) {
    // English never shows the middle form; its line in the catalog is the Russian one for 2 to 4.
    let title = trn(u64::from(count), tl!("{} new workout"), tl!("{} new workouts (2 to 4)"), tl!("{} new workouts"));
    let open = widgets::card(ui, |ui| {
        widgets::card_title(ui, &title);
        ui.add_space(metrics::GAP / 2.0);
        widgets::primary_button(ui, tl!("Show new")).clicked()
    });
    if open {
        app.ui.section = Section::Activities;
        app.ui.activities.filter = ListFilter::New;
    }
}

/// The newest strength workout that counts: its name, day and sets, and the button that opens it in the list.
fn last_strength_card(app: &mut GymgarApp, ui: &mut Ui, item: &ActivityListItemView) {
    let t = Tokens::get(ui.ctx());
    let details = activities_ui::details(app, item);
    let open = widgets::card(ui, |ui| {
        widgets::card_title(ui, tl!("Last strength workout"));
        ui.add_space(metrics::GAP / 2.0);
        ui.label(RichText::new(activities_ui::title(item)).color(t.text));
        widgets::hint(ui, &details);
        ui.add_space(metrics::GAP / 2.0);
        widgets::secondary_button(ui, tl!("Open")).clicked()
    });
    if open {
        let activities = &mut app.ui.activities;
        activities.selected = Some(item.id.0);
        // A filter that would hide it gives way to the whole list.
        if !activities_ui::filter_shows(activities.filter, item) {
            activities.filter = ListFilter::All;
        }
        app.ui.section = Section::Activities;
    }
}

/// A connection that works, and how its import stands. `Unverified`: the last check got no answer from Garmin, so this is
/// the last known state, and it is marked as such (spec section 2, item 8).
fn connected(app: &mut GymgarApp, ui: &mut Ui, status: &GarminStatusView) {
    let t = Tokens::get(ui.ctx());
    widgets::card(ui, |ui| {
        widgets::card_title(ui, tl!("Garmin Connect"));
        ui.add_space(metrics::GAP / 2.0);
        ui.horizontal(|ui| {
            widgets::status_dot(ui, t.accent, tl!("Connected"));
            if status.status == ConnectionStatus::Unverified {
                ui.colored_label(t.warning, tl!("Could not verify"));
            }
        });
        activities_ui::sync_line(app, ui, &status.sync);
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
