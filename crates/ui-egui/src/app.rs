//! The window frame: the icon rail, the current section and the glue to the engine.
//!
//! [`GymgarApp`] owns the engine session and the visual state. It holds no business logic: every action is a command
//! sent through [`GymgarApp::run`], the only place the UI calls the engine, and what the screens show about the
//! outside world (the language, the Garmin connection) is read back from the engine after anything that may have
//! changed it. Engine errors reach the user as translated text chosen by the kind of error; the engine's own English
//! text is for agents and logs.

use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, Utc};
use egui::{CentralPanel, Frame, Margin, ScrollArea};
use gymgar_domain::{ConnectionStatus, Language};
use gymgar_engine::{Caller, CommandKind, EngineError, GarminAuthError, GarminStatusView, JobFinished, Outcome, Session, commands};
use serde_json::Value;

use crate::i18n;
use crate::state::{Section, UiState};
use crate::theme::{Tokens, metrics};
use crate::{rail, settings_ui, summary_ui, tl, widgets};

/// How often the window looks at the database for changes another process (the CLI, an agent) has made.
const STORE_POLL: Duration = Duration::from_secs(1);

/// The desktop application: owns the engine session and the visual state.
pub struct GymgarApp {
    session: Session,
    /// What the window shows: the section and the connect form. The screens read and change it freely.
    pub ui: UiState,
    /// The Garmin connection as the engine last reported it; `None` until it could be read.
    status: Option<GarminStatusView>,
    /// What went wrong that the user should see, in their language: a refresh that failed, or a command with no better
    /// place to show its failure. The next refresh clears it.
    error: Option<String>,
    /// The interface language the user chose (which may be "system").
    language: Language,
    system_locale: Option<String>,
    /// The language the window was last drawn in, to notice a change.
    shown_lang: Option<i18n::Lang>,
    /// Dates are shown in this offset from UTC; the system's time zone when `None`.
    utc_offset: Option<FixedOffset>,
    /// Not before this time is the database looked at again.
    pub(crate) next_store_poll: Instant,
    /// The start-up check of a saved connection has been considered. (`pub(crate)` so the screen tests can start from a
    /// window that has been open a while.)
    pub(crate) verify_started: bool,
}

impl GymgarApp {
    /// An app on `session` showing `ui`, in the language of the settings (and of the operating system for "system").
    pub fn new(session: Session, ui: UiState) -> GymgarApp {
        GymgarApp::with_system_locale(session, ui, sys_locale::get_locale())
    }

    /// Like [`GymgarApp::new`], with the locale the operating system is taken to have: `None` when it is unknown.
    /// For tests, which must not depend on the machine they run on.
    pub fn with_system_locale(session: Session, ui: UiState, system_locale: Option<String>) -> GymgarApp {
        let mut app = GymgarApp {
            session,
            ui,
            status: None,
            error: None,
            language: Language::System,
            system_locale,
            shown_lang: None,
            utc_offset: None,
            next_store_poll: Instant::now(),
            verify_started: false,
        };
        app.refresh();
        app
    }

    /// Shows dates in the time zone with this offset from UTC instead of the system's. For tests, which must not depend on
    /// the machine they run on.
    pub fn with_utc_offset(mut self, offset: FixedOffset) -> GymgarApp {
        self.utc_offset = Some(offset);
        self
    }

    /// One frame of the whole interface: the icon rail and the current section. [`eframe::App::ui`] and the UI tests
    /// both call this.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        rail::show(&mut self.ui.section, ui);
        let t = Tokens::get(ui.ctx());
        CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::same(metrics::SCREEN_MARGIN))).show(ui, |ui| {
            if let Some(error) = &self.error {
                widgets::warning_box(ui, error);
                ui.add_space(metrics::GAP);
            }
            // The window may be low (the minimum is 500 points), so a long screen scrolls.
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.set_max_width(metrics::CONTENT_MAX_WIDTH);
                match self.ui.section {
                    Section::Summary => summary_ui::show(self, ui),
                    Section::Settings => settings_ui::show(self, ui),
                }
            });
        });
    }

    /// Runs the command `id` for the user. The one place the UI calls the engine. A failure comes back as text for the
    /// user, in their language; after a command that changes something, the language and the connection are read again.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Outcome, String> {
        let outcome = self.execute(id, params).map_err(|error| error_text(&error).to_owned())?;
        if commands::find(id).is_some_and(|spec| spec.kind != CommandKind::Query) {
            self.refresh();
        }
        Ok(outcome)
    }

    /// Work to do once per frame, before drawing ([`eframe::App::logic`]): applies the results of finished background
    /// jobs, notices what other processes wrote to the database (once a second), and checks a saved connection once
    /// at start.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        for job in self.session.poll_jobs() {
            self.job_finished(job);
            // A check that failed has still recorded its verdict, so the status is read again whatever the result was.
            changed = true;
        }
        let now = Instant::now();
        if now >= self.next_store_poll {
            self.next_store_poll = now + STORE_POLL;
            changed |= self.session.store_changed();
            // A status that could not be read stays unknown until it is asked for again, and the database may not have
            // changed in between (a lock that lasted too long, say), so nothing else would prompt it.
            changed |= self.status.is_none();
        }
        if changed {
            self.refresh();
        }
        self.verify_once();
        // An idle window is not redrawn, and so not ticked: ask for the next poll.
        ctx.request_repaint_after(STORE_POLL);
    }

    /// The Garmin connection as of the last refresh; `None` while it cannot be read.
    pub fn status(&self) -> Option<&GarminStatusView> {
        self.status.as_ref()
    }

    /// Why the window could not refresh itself, as text for the user.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The interface language the user chose, which may be "system".
    pub fn language(&self) -> Language {
        self.language
    }

    /// Whether the engine lets the command `id` run right now. A screen greys the command's button out when it does not,
    /// and says why in its own words: the engine's reason is English text for agents and logs.
    pub fn is_enabled(&self, id: &str) -> bool {
        commands::find(id).is_some_and(|spec| (spec.enabled)(&self.session).is_ok())
    }

    /// `at` as a date and time on the user's wall clock.
    pub(crate) fn local_time(&self, at: DateTime<Utc>) -> NaiveDateTime {
        match self.utc_offset {
            Some(offset) => at.with_timezone(&offset).naive_local(),
            None => at.with_timezone(&Local).naive_local(),
        }
    }

    /// Tells the user something went wrong that no screen has a place for, in the banner at the top.
    pub(crate) fn report(&mut self, text: &str) {
        self.error = Some(text.to_owned());
    }

    fn execute(&mut self, id: &str, params: Value) -> Result<Outcome, EngineError> {
        self.session.execute(id, params, Caller::User).inspect_err(|error| match error {
            // The rejected params may hold what the user typed, so only the command is logged.
            EngineError::BadParams { cmd, .. } => log::warn!("{cmd} refused its params"),
            error => log::warn!("{id} failed: {error}"),
        })
    }

    /// Reads the language and the connection again.
    fn refresh(&mut self) {
        self.error = None;
        match self.session.language() {
            Ok(language) => {
                self.language = language;
                let lang = i18n::resolve(language, self.system_locale.as_deref());
                // The error of the connect form is kept as text in the language it was made in: in another language
                // it would be a stale sentence, so it goes.
                if self.shown_lang.is_some_and(|shown| shown != lang) {
                    self.ui.connect.error = None;
                }
                self.shown_lang = Some(lang);
                i18n::set_current(lang);
            }
            Err(error) => {
                log::warn!("cannot read the interface language: {error}");
                self.error = Some(error_text(&error).to_owned());
            }
        }
        match self.read_status() {
            Ok(status) => self.status = Some(status),
            Err(text) => self.error = Some(text),
        }
    }

    fn read_status(&mut self) -> Result<GarminStatusView, String> {
        match self.run("garmin.status", Value::Null)? {
            Outcome::Done(value) => serde_json::from_value(value).map_err(|error| {
                log::error!("cannot read the answer of garmin.status: {error}");
                tl!("Something went wrong. Please try again.").to_owned()
            }),
            Outcome::Started(_) => Err(tl!("Something went wrong. Please try again.").to_owned()),
        }
    }

    /// The form of the connect screen learns how its key check ended.
    fn job_finished(&mut self, job: JobFinished) {
        let form = &mut self.ui.connect;
        if form.pending != Some(job.id) {
            return;
        }
        form.pending = None;
        match &job.result {
            Ok(_) => {
                form.key.clear();
                form.error = None;
                form.replacing = false;
            }
            // The key stays in the field so the user can fix it.
            Err(error) => form.error = Some(error_text(error).to_owned()),
        }
    }

    /// Checks a connection saved by an earlier run, once: one exchange of the key for a token, in the background. What
    /// it finds is shown through the status; a failure here is not an error of the user's.
    fn verify_once(&mut self) {
        // Without a status there is nothing to decide on: the check is neither done nor forgotten, and the next tick
        // looks again.
        let Some(status) = &self.status else { return };
        if std::mem::replace(&mut self.verify_started, true) {
            return;
        }
        if status.status != ConnectionStatus::NotConnected
            && let Err(reason) = self.run("garmin.verify", Value::Null)
        {
            log::info!("the start-up check of the Garmin key did not start: {reason}");
        }
    }
}

impl eframe::App for GymgarApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.ui.save(storage);
    }
}

/// What to tell the user about a failed command. Chosen by the kind of error, never by the engine's English text,
/// which may echo what the user typed. Every kind is listed, so a new one cannot be forgotten.
fn error_text(error: &EngineError) -> &'static str {
    match error {
        EngineError::Garmin(GarminAuthError::KeyRejected) => tl!("Garmin did not accept the key. Check that it was copied in full, or create a new one."),
        EngineError::Garmin(GarminAuthError::Network) => tl!("Could not reach Garmin. Check your internet connection."),
        EngineError::Garmin(GarminAuthError::RateLimited) => tl!("Garmin has temporarily limited requests. Try again in a few minutes."),
        EngineError::Garmin(GarminAuthError::Unavailable | GarminAuthError::Unexpected) => tl!("Garmin is unavailable right now. Try again later."),
        EngineError::FileStoreConsentRequired => tl!("The system password store is unavailable. Allow saving the key to a file to continue."),
        EngineError::NotConnected => tl!("Garmin Connect is not connected."),
        EngineError::KeyMissing => tl!("The Garmin key was not found in the password store. Connect Garmin again."),
        EngineError::Secrets(_) => tl!("Gymgar could not use the password store. Check that it is unlocked and try again."),
        EngineError::Store(_) => tl!("Gymgar could not access its data. Check that the disk is available and try again."),
        EngineError::BadParams { .. } => tl!("The value is not valid. Check it and try again."),
        EngineError::Disabled { .. } => tl!("This is not available right now."),
        EngineError::UnknownCommand(_) | EngineError::AgentNotAllowed(_) | EngineError::JobPanicked(_) | EngineError::CommandPanicked(_) => {
            tl!("Something went wrong. Please try again.")
        }
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
