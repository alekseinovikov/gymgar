//! The Garmin Connect card of Settings: the steps to get a key, the form that connects it, and what is known about a
//! connection that exists. Also the expiry warning, which the Summary screen shows too.
//!
//! It holds no logic of its own: what the connection is comes from `garmin.status`, what may be done comes from the
//! engine (`GymgarApp::is_enabled`), and every action is a command. The card only chooses what to show for each state.

use egui::{Grid, Id, Key, RichText, Ui, vec2};
use gymgar_domain::{ConnectionStatus, KeyTerm, SecretBackend};
use gymgar_engine::{GarminStatusView, Outcome};
use serde_json::{Map, Value};

use crate::state::Section;
use crate::theme::{Tokens, metrics};
use crate::{GymgarApp, dates, tl, widgets};

/// The page of Garmin Connect where the user creates the Personal API key.
pub(crate) const KEY_PAGE_URL: &str = "https://connect.garmin.com/app/settings/personalApiToken";

/// Where the key is kept, as the user knows the place on their system. `os` is `std::env::consts::OS`.
fn storage_line(backend: SecretBackend, os: &str) -> &'static str {
    match (backend, os) {
        (SecretBackend::File, _) => tl!("The key is stored in a file on disk"),
        (SecretBackend::System, "macos") => tl!("The key is stored in the macOS keychain"),
        (SecretBackend::System, "windows") => tl!("The key is stored in the Windows credential manager"),
        // Secret Service on Linux and the other systems that have one.
        (SecretBackend::System, _) => tl!("The key is stored in the system password store"),
    }
}

/// The line about where the key is kept, for the system this runs on.
fn storage_hint(ui: &mut Ui, backend: SecretBackend) {
    widgets::hint(ui, storage_line(backend, std::env::consts::OS));
}

/// The warning before the key goes to a file (spec section 7): it names the file, so the user knows what they agree to. A
/// store that cannot say where its file is gets the same warning without a place.
fn file_consent_text(location: Option<&str>) -> String {
    match location {
        Some(path) => tl!("The system password store is unavailable. Save the key to the file {}? Any program running as you will be able to read it.")
            .replace("{}", path),
        None => tl!("The system password store is unavailable. Save the key to a file instead? Any program running as you will be able to read it.").to_owned(),
    }
}

/// Draws the Garmin Connect card.
pub(crate) fn show(app: &mut GymgarApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let status = app.status().cloned();
    widgets::card(ui, |ui| {
        widgets::card_title(ui, tl!("Garmin Connect"));
        ui.add_space(metrics::GAP / 2.0);
        let Some(status) = status else {
            widgets::hint(ui, tl!("The state of Garmin Connect is not available."));
            return;
        };
        let connected = matches!(status.status, ConnectionStatus::Connected | ConnectionStatus::Unverified);
        if status.status == ConnectionStatus::NotConnected {
            // Nothing is left to disconnect, so a question about it must not come back armed with the next connection.
            app.ui.connect.confirm_disconnect = false;
        }
        state_lines(app, ui, &t, &status);
        if connected && !app.ui.connect.replacing {
            actions(app, ui, &status);
        } else {
            connect_form(app, ui, &t, &status);
            if !connected && status.status != ConnectionStatus::NotConnected {
                broken_key_actions(app, ui, &status);
            }
        }
        if let Some(error) = app.ui.connect.error.clone() {
            ui.add_space(metrics::GAP);
            widgets::warning_box(ui, &error);
        }
    });
}

/// Draws the warning that the key is about to expire, with a link to the page where a new one is made and, when
/// `replace_button` is set, a button that opens the form to replace it. Settings has that button beside "Disconnect"
/// already, so it asks for none.
pub(crate) fn expiry_warning(app: &mut GymgarApp, ui: &mut Ui, replace_button: bool) {
    let Some(expires) = app.status().and_then(|status| status.key_expires_at) else { return };
    let text = tl!("The Garmin key expires on {}. Create a new one").replace("{}", &dates::date(app.local_time(expires)));
    let replace = widgets::warning_box_with(ui, &text, |ui| {
        if !replace_button {
            widgets::link(ui, tl!("Personal API Key page"), KEY_PAGE_URL);
            return false;
        }
        ui.horizontal(|ui| {
            widgets::link(ui, tl!("Personal API Key page"), KEY_PAGE_URL);
            widgets::secondary_button(ui, tl!("Replace key")).clicked()
        })
        .inner
    });
    if replace {
        replace_key(app);
    }
}

/// Opens the form that replaces the key. It is on the Settings screen, so that is where the user goes.
pub(crate) fn replace_key(app: &mut GymgarApp) {
    app.ui.connect.replacing = true;
    app.ui.connect.error = None;
    app.ui.section = Section::Settings;
}

/// What is known about the connection: its state, and for a connection that exists, until when its key is good, when
/// it was last checked and where it is kept.
fn state_lines(app: &mut GymgarApp, ui: &mut Ui, t: &Tokens, status: &GarminStatusView) {
    match status.status {
        ConnectionStatus::NotConnected => {}
        ConnectionStatus::Connected | ConnectionStatus::Unverified => {
            ui.horizontal(|ui| {
                widgets::status_dot(ui, t.accent, tl!("Connected"));
                if status.status == ConnectionStatus::Unverified {
                    // The state shown is the last one known; the check since then did not get an answer.
                    ui.colored_label(t.warning, tl!("Could not verify"));
                }
            });
            if let Some(expires) = status.key_expires_at {
                widgets::hint(ui, &tl!("Key valid until {}").replace("{}", &dates::date(app.local_time(expires))));
            }
            if let Some(verified) = status.last_verified_at {
                widgets::hint(ui, &tl!("Verified {}").replace("{}", &dates::date_time(app.local_time(verified))));
            }
            if let Some(backend) = status.secret_backend {
                storage_hint(ui, backend);
            }
            if status.expiry_warning {
                ui.add_space(metrics::GAP);
                expiry_warning(app, ui, false);
            }
        }
        ConnectionStatus::KeyRejected => {
            widgets::status_dot(ui, t.warning, tl!("Key not valid"));
            widgets::hint(ui, tl!("Garmin no longer accepts the saved key: it may have expired or been deleted. Create a new key and enter it below."));
        }
        ConnectionStatus::KeyMissing => {
            widgets::status_dot(ui, t.warning, tl!("Key not found"));
            widgets::hint(ui, tl!("Connect Garmin again"));
        }
    }
}

/// The buttons of a connection that works: replace the key, or disconnect after a confirmation.
fn actions(app: &mut GymgarApp, ui: &mut Ui, status: &GarminStatusView) {
    ui.add_space(metrics::GAP);
    if app.ui.connect.confirm_disconnect {
        confirm_disconnect(app, ui);
        return;
    }
    // The engine says whether the key can be removed right now; the card says why not, in the user's language.
    let can_disconnect = app.is_enabled("garmin.disconnect");
    let (mut replace, mut ask) = (false, false);
    ui.horizontal(|ui| {
        replace = widgets::secondary_button(ui, tl!("Replace key")).clicked();
        ask = disconnect_button(ui, can_disconnect);
    });
    if !can_disconnect {
        why_disconnect_is_unavailable(ui, status);
    }
    if replace {
        replace_key(app);
    } else if ask {
        ask_to_disconnect(app);
    }
}

/// Under the form of a connection whose key does not work or is gone: the form replaces the key, and this removes the
/// connection altogether, after the same confirmation as for a connection that works.
fn broken_key_actions(app: &mut GymgarApp, ui: &mut Ui, status: &GarminStatusView) {
    ui.add_space(metrics::GAP);
    if app.ui.connect.confirm_disconnect {
        confirm_disconnect(app, ui);
        return;
    }
    let can_disconnect = app.is_enabled("garmin.disconnect");
    if disconnect_button(ui, can_disconnect) {
        ask_to_disconnect(app);
    }
    if !can_disconnect {
        why_disconnect_is_unavailable(ui, status);
    }
}

/// The "Disconnect" button, greyed out unless `enabled`; whether it was clicked.
fn disconnect_button(ui: &mut Ui, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| widgets::secondary_button(ui, tl!("Disconnect"))).inner.clicked()
}

/// Why "Disconnect" is greyed out: a key check holds the connection, or the key's store cannot be reached.
fn why_disconnect_is_unavailable(ui: &mut Ui, status: &GarminStatusView) {
    widgets::hint(
        ui,
        if status.check_running {
            tl!("A key check is running. Try again in a moment.")
        } else {
            tl!("Gymgar cannot reach the password store that holds the key, so it cannot disconnect now.")
        },
    );
}

/// The first click on "Disconnect" only asks.
fn ask_to_disconnect(app: &mut GymgarApp) {
    app.ui.connect.confirm_disconnect = true;
    app.ui.connect.error = None;
}

/// The question before the key and the connection go, with its two answers.
fn confirm_disconnect(app: &mut GymgarApp, ui: &mut Ui) {
    ui.label(tl!("Disconnect Garmin? Gymgar will delete the key and the connection details."));
    let (mut confirmed, mut cancelled) = (false, false);
    ui.horizontal(|ui| {
        confirmed = widgets::secondary_button(ui, tl!("Yes, disconnect")).clicked();
        cancelled = widgets::secondary_button(ui, tl!("Cancel")).clicked();
    });
    if confirmed {
        disconnect(app);
    } else if cancelled {
        app.ui.connect.confirm_disconnect = false;
    }
}

/// The steps to get a key, the key, its term and the button that connects.
fn connect_form(app: &mut GymgarApp, ui: &mut Ui, t: &Tokens, status: &GarminStatusView) {
    if status.status != ConnectionStatus::NotConnected {
        ui.add_space(metrics::GAP);
    }
    steps(ui, t);
    ui.add_space(metrics::GAP);

    let pending = app.ui.connect.pending.is_some();
    // A check of the saved key (at start-up) has the same effect on the form as the check of a new one: wait for it.
    let busy = pending || status.check_running;
    let mut enter_pressed = false;
    // While Garmin is asked, the form stays as it is: the key being checked must not change under the check.
    ui.add_enabled_ui(!pending, |ui| {
        let form = &mut app.ui.connect;
        Grid::new("garmin-connect-form").num_columns(2).spacing(vec2(metrics::GAP * 2.0, metrics::GAP)).show(ui, |ui| {
            let label = ui.label(RichText::new(tl!("Key")).color(t.text_dim));
            let field = widgets::secret_field(ui, Id::unique("garmin-key"), &mut form.key, metrics::FIELD_WIDTH).labelled_by(label.id);
            enter_pressed = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            ui.end_row();

            ui.label(RichText::new(tl!("Key term")).color(t.text_dim));
            ui.horizontal(|ui| {
                let terms = [
                    (KeyTerm::Days30, tl!("30 days")),
                    (KeyTerm::Days90, tl!("90 days")),
                    (KeyTerm::Days180, tl!("180 days")),
                    (KeyTerm::Days365, tl!("1 year")),
                ];
                widgets::dropdown(ui, Id::unique("garmin-key-term"), &mut form.term, &terms);
                widgets::hint(ui, tl!("as chosen on the website; we will warn you a week before it ends"));
            });
            ui.end_row();
        });
        if status.file_store_needs_consent {
            widgets::warning_box(ui, &file_consent_text(status.file_store_location.as_deref()));
            ui.checkbox(&mut form.allow_file_store, tl!("Save the key to a file"));
        }
    });

    // The engine refuses a key that is empty once trimmed, so the button waits for one.
    let can_connect = !busy && !app.ui.connect.key.trim().is_empty();
    let (mut connect, mut cancel) = (enter_pressed && can_connect, false);
    ui.horizontal(|ui| {
        connect |= ui.add_enabled_ui(can_connect, |ui| widgets::primary_button(ui, tl!("Connect"))).inner.clicked();
        if busy {
            ui.add(egui::Spinner::new().size(metrics::SPINNER_SIZE));
        }
        // Only a working connection has a card to go back to; for a key that does not work the form is the card.
        if app.ui.connect.replacing && matches!(status.status, ConnectionStatus::Connected | ConnectionStatus::Unverified) {
            cancel = ui.add_enabled_ui(!pending, |ui| widgets::secondary_button(ui, tl!("Cancel"))).inner.clicked();
        }
        if let Some(backend) = status.new_key_backend {
            storage_hint(ui, backend);
        }
    });
    if connect {
        start_connect(app);
    } else if cancel {
        let form = &mut app.ui.connect;
        form.replacing = false;
        form.key.clear();
        form.error = None;
    }
}

/// The four steps of spec section 2.2: the first is the link to the page of the key.
fn steps(ui: &mut Ui, t: &Tokens) {
    let number = |ui: &mut Ui, n: u32| ui.label(RichText::new(format!("{n}.")).color(t.text_dim));
    ui.horizontal_top(|ui| {
        number(ui, 1);
        widgets::link(ui, tl!("Open the Personal API Key page in Garmin Connect and sign in."), KEY_PAGE_URL);
    });
    for (n, step) in [
        (2, tl!("Click Create Key and accept the terms.")),
        (3, tl!("Choose how long the key is valid; 1 year is the most convenient.")),
        (4, tl!("Click Create and copy the key right away: Garmin shows it only once.")),
    ] {
        ui.horizontal_top(|ui| {
            number(ui, n);
            ui.label(step);
        });
    }
}

/// Starts the check of the key in the form. The form waits for the answer as long as `pending` is set, and
/// [`GymgarApp::tick`] ends the wait.
fn start_connect(app: &mut GymgarApp) {
    let form = &mut app.ui.connect;
    form.error = None;
    // Built by hand, not with `json!`, which unwraps inside.
    let mut params = Map::new();
    params.insert("apiKey".to_owned(), Value::String(form.key.clone()));
    params.insert("keyTerm".to_owned(), Value::from(form.term.days()));
    params.insert("allowFileStore".to_owned(), Value::Bool(form.allow_file_store));
    match app.run("garmin.connect", Value::Object(params)) {
        Ok(Outcome::Started(job)) => app.ui.connect.pending = Some(job),
        Ok(Outcome::Done(_)) => log::warn!("garmin.connect finished at once, though it is a background job"),
        Err(text) => app.ui.connect.error = Some(text),
    }
}

/// The user confirmed: the key and the connection are removed.
fn disconnect(app: &mut GymgarApp) {
    app.ui.connect.confirm_disconnect = false;
    match app.run("garmin.disconnect", Value::Null) {
        Ok(_) => {
            app.ui.connect.replacing = false;
            app.ui.connect.error = None;
        }
        Err(text) => app.ui.connect.error = Some(text),
    }
}

#[cfg(test)]
#[path = "garmin_ui_tests.rs"]
mod tests;
