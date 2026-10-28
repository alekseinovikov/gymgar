//! The window frame: the icon rail, the current section and the glue to the engine.
//!
//! [`GymgarApp`] owns the engine session and the visual state. It holds no business logic: every action is a command
//! sent through [`GymgarApp::run`], the only place the UI calls the engine, and what the screens show about the
//! outside world (the language, the Garmin connection, the activities) is read back from the engine after anything that
//! may have changed it. Engine errors reach the user as translated text chosen by the kind of error; the engine's own
//! English text is for agents and logs.
//!
//! At start, a connected app imports what is new in Garmin by itself (activity import spec §2, item 1): it reads the
//! stored files again if this version parses them better (`activity.reparse`), then syncs (`garmin.sync`), once. Garmin
//! connected in the window is imported from at once, the same way (Ruling 19).
//!
//! The activity open beside the list is read (`activity.get`) when the selection changes, and again only when something
//! may have changed it (Ruling 5): a command, the end of a job, another process writing to the database, or its line in
//! the list changing while a sync streams in (its file arrived). Opening a new one takes its mark off (`activity.markSeen`).
//!
//! Progress is read with the activities (`progress.list`, on every refresh), and the exercise open on it (`progress.get`)
//! when its key changes and when something may have changed it, with the same signals as the open activity, but not on
//! every refresh: a sync streaming in refreshes the tiles on every frame, and the open exercise waits for its end.

use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, Utc};
use egui::{CentralPanel, Frame, Margin, ScrollArea};
use gymgar_domain::{ConnectionStatus, Language};
use gymgar_engine::{
    ActivityListItemView, ActivityListView, ActivityView, Caller, CommandKind, EngineError, GarminAuthError, GarminStatusView, JobEvent, JobFinished, JobId,
    Outcome, ProgressItemView, ProgressListView, ProgressView, Session, commands,
};
use serde_json::{Map, Value};

use crate::i18n;
use crate::state::{ListFilter, Section, UiState};
use crate::theme::{Tokens, metrics};
use crate::{activities_ui, progress_ui, rail, settings_ui, summary_ui, tl, widgets};

/// How often the window looks at the database for changes another process (the CLI, an agent) has made.
const STORE_POLL: Duration = Duration::from_secs(1);

/// The activity list is read in pages of this many (the most `activity.list` gives at once)...
const LIST_PAGE: u32 = 1000;
/// ...and at most this many pages: a hundred thousand activities, decades of daily training. A database that holds more
/// is not one this window can show whole, and it stops there instead of reading without end.
const LIST_MAX_PAGES: u32 = 100;

/// `progress.list` gives at most this many tiles at once. The answer is not paged: an exercise list this long is not one a
/// person has, and a full answer is only noted in the log.
const PROGRESS_LIMIT: u32 = 1000;

/// Where the start-up import stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Startup {
    /// Not started: the first tick that knows the connection decides.
    Pending,
    /// The re-parse of the stored files runs; the sync starts when its end has been applied.
    Reparsing(JobId),
    /// The re-parse has ended, or Garmin was just connected: the sync starts at this tick.
    SyncDue,
    /// Started, or nothing to start.
    Done,
}

/// The desktop application: owns the engine session and the visual state.
pub struct GymgarApp {
    session: Session,
    /// What the window shows: the section and the connect form. The screens read and change it freely.
    pub ui: UiState,
    /// The Garmin connection as the engine last reported it; `None` until it could be read.
    status: Option<GarminStatusView>,
    /// Why the last refresh could not read the engine's state, in the user's language. The next refresh clears it.
    error: Option<String>,
    /// What went wrong that no screen has a place for ([`GymgarApp::report`]), in the user's language. Refreshes leave it:
    /// it stays until the user next runs a command that changes something and works.
    pub(crate) reported: Option<String>,
    /// The last job a failed partial result was reported for, so a sync is reported once however many pieces fail.
    reported_job: Option<JobId>,
    /// The activities, newest first, as `activity.list` last answered.
    activities: Vec<ActivityListItemView>,
    /// The activity open beside the list, as `activity.get` last answered for `detail_for`; `None` when nothing is open
    /// or it could not be read.
    detail: Option<ActivityView>,
    /// Why the open activity could not be read, in the user's language.
    detail_error: Option<String>,
    /// The selection `detail` was read for: when the list's selection is another, the open activity is read again.
    detail_for: Option<i64>,
    /// How many times the open activity was read: the tests check it is not read on every frame.
    #[cfg(test)]
    pub(crate) detail_reads: usize,
    /// The exercises with strength history, as `progress.list` last answered, in the order the tiles are shown.
    progress: Vec<ProgressItemView>,
    /// The open exercise, as `progress.get` last answered for `progress_detail_for`; `None` when none is open or it could
    /// not be read.
    progress_detail: Option<ProgressView>,
    /// Why the open exercise could not be read, in the user's language.
    progress_detail_error: Option<String>,
    /// The key `progress_detail` was read for: when `ui.progress.open` is another, the exercise is read again.
    progress_detail_for: Option<String>,
    /// How many times the open exercise was read: the tests check it is not read on every frame.
    #[cfg(test)]
    pub(crate) progress_detail_reads: usize,
    /// The interface language the user chose (which may be "system").
    language: Language,
    system_locale: Option<String>,
    /// The language the window was last drawn in, to notice a change.
    shown_lang: Option<i18n::Lang>,
    /// Dates are shown in this offset from UTC; the system's time zone when `None`.
    utc_offset: Option<FixedOffset>,
    /// "Now" for the screens (this week, today) instead of the system clock; for tests.
    fixed_time: Option<DateTime<Utc>>,
    /// Not before this time is the database looked at again.
    pub(crate) next_store_poll: Instant,
    /// Where the start-up import stands. (`pub(crate)` so the screen tests can start from a window that has been open a
    /// while.)
    pub(crate) startup: Startup,
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
            reported: None,
            reported_job: None,
            activities: Vec::new(),
            detail: None,
            detail_error: None,
            detail_for: None,
            #[cfg(test)]
            detail_reads: 0,
            progress: Vec::new(),
            progress_detail: None,
            progress_detail_error: None,
            progress_detail_for: None,
            #[cfg(test)]
            progress_detail_reads: 0,
            language: Language::System,
            system_locale,
            shown_lang: None,
            utc_offset: None,
            fixed_time: None,
            next_store_poll: Instant::now(),
            startup: Startup::Pending,
        };
        app.refresh();
        // An exercise left open by the last run is read now, since no screen has asked for it yet.
        app.refresh_progress_detail(false);
        app
    }

    /// Shows dates in the time zone with this offset from UTC instead of the system's. For tests, which must not depend on
    /// the machine they run on.
    pub fn with_utc_offset(mut self, offset: FixedOffset) -> GymgarApp {
        self.utc_offset = Some(offset);
        self
    }

    /// Takes `now` as the current time of the screens ("this week", "today") instead of the system clock. For tests,
    /// which must not depend on the day they run.
    pub fn with_time(mut self, now: DateTime<Utc>) -> GymgarApp {
        self.fixed_time = Some(now);
        self
    }

    /// One frame of the whole interface: the icon rail and the current section. [`eframe::App::ui`] and the UI tests
    /// both call this.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let new_count = self.status.as_ref().map_or(0, |status| status.sync.new_count);
        rail::show(&mut self.ui.section, new_count, ui);
        if self.ui.section == Section::Activities {
            // A list beside a detail, each scrolling on its own: no page-wide scroll and no width limit.
            activities_ui::show(self, ui);
            return;
        }
        let t = Tokens::get(ui.ctx());
        if self.ui.section == Section::Progress {
            // The tiles and the exercise page take the whole width, not a screen's column; the tiles scroll on their own.
            CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::same(metrics::SCREEN_MARGIN))).show(ui, |ui| {
                self.banner(ui);
                progress_ui::show(self, ui);
            });
            return;
        }
        CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::same(metrics::SCREEN_MARGIN))).show(ui, |ui| {
            self.banner(ui);
            // The window may be low (the minimum is 500 points), so a long screen scrolls.
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                // Never wider than the window: `set_max_width` would let the content run past a narrow one.
                ui.set_max_width(metrics::CONTENT_MAX_WIDTH.min(ui.available_width()));
                match self.ui.section {
                    Section::Summary => summary_ui::show(self, ui),
                    Section::Settings => settings_ui::show(self, ui),
                    // Drawn above, beside the rail.
                    Section::Activities | Section::Progress => {}
                }
            });
        });
    }

    /// The banner at the top of a screen: why the window could not refresh, and what was reported.
    pub(crate) fn banner(&self, ui: &mut egui::Ui) {
        for text in [&self.error, &self.reported].into_iter().flatten() {
            widgets::warning_box(ui, text);
            ui.add_space(metrics::GAP);
        }
    }

    /// Runs the command `id` for the user. The one place the UI calls the engine. A failure comes back as text for the
    /// user, in their language; after a command that changes something, the language, the connection and the
    /// activities are read again, and a failure reported earlier is old news.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Outcome, String> {
        let outcome = self.start(id, params)?;
        if commands::find(id).is_some_and(|spec| spec.kind != CommandKind::Query) {
            self.reported = None;
        }
        Ok(outcome)
    }

    /// [`GymgarApp::run`] for what the window starts by itself: a failure reported earlier stays, since the user has
    /// done nothing about it.
    fn start(&mut self, id: &str, params: Value) -> Result<Outcome, String> {
        let outcome = self.execute(id, params).map_err(|error| error_text(&error).to_owned())?;
        if commands::find(id).is_some_and(|spec| spec.kind != CommandKind::Query) {
            self.refresh();
            self.refresh_detail(true);
            self.refresh_progress_detail(true);
        }
        Ok(outcome)
    }

    /// Work to do once per frame, before drawing ([`eframe::App::logic`]): applies what background jobs have sent
    /// (partial results and ends), notices what other processes wrote to the database (once a second), and starts the
    /// import once at start.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        // Whether the open activity may have changed in a way its line in the list does not show.
        let mut detail_stale = false;
        let mut failure = None;
        for event in self.session.poll_jobs() {
            match event {
                JobEvent::Finished(job) => {
                    self.job_finished(job);
                    detail_stale = true;
                }
                // Ruling 13: a piece of a job that could not be applied (a page of a sync that could not be written) is
                // the user's to know, once per job: a sync can fail the same way hundreds of times.
                JobEvent::Partial { id, command, result: Err(error) } => {
                    if self.reported_job != Some(id) {
                        log::warn!("part of what job {} of {command} sent could not be applied: {error}", id.0);
                        self.reported_job = Some(id);
                        failure = Some(error_text(&error));
                    }
                }
                JobEvent::Partial { result: Ok(()), .. } => {}
            }
            // Whatever a job sent may have changed what the screens show, even when it failed: a check that failed has
            // still recorded its verdict. So the state is read again after every event.
            changed = true;
        }
        let now = Instant::now();
        if now >= self.next_store_poll {
            self.next_store_poll = now + STORE_POLL;
            // Another process (an agent) may have corrected the open activity's sets.
            let written = self.session.store_changed();
            changed |= written;
            detail_stale |= written;
            // A status that could not be read stays unknown until it is asked for again, and the database may not have
            // changed in between (a lock that lasted too long, say), so nothing else would prompt it.
            changed |= self.status.is_none();
        }
        if changed {
            self.refresh();
            self.refresh_detail(detail_stale);
            self.refresh_progress_detail(detail_stale);
        }
        if let Some(text) = failure {
            self.report(text);
        }
        self.start_once();
        // An idle window is not redrawn, and so not ticked: ask for the next poll.
        ctx.request_repaint_after(STORE_POLL);
    }

    /// The Garmin connection as of the last refresh; `None` while it cannot be read.
    pub fn status(&self) -> Option<&GarminStatusView> {
        self.status.as_ref()
    }

    /// What the banner says, as text for the user: why the window could not refresh itself, or else what was reported.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().or(self.reported.as_deref())
    }

    /// The activities, newest first, as of the last refresh.
    pub fn activities(&self) -> &[ActivityListItemView] {
        &self.activities
    }

    /// The activity open beside the list, as last read; `None` when nothing is open or it could not be read.
    pub fn detail(&self) -> Option<&ActivityView> {
        self.detail.as_ref()
    }

    /// Why the open activity could not be read, in the user's language.
    pub(crate) fn detail_error(&self) -> Option<&str> {
        self.detail_error.as_deref()
    }

    /// Opens the activity the list has selected, once per selection: reads it and, when the user has not opened it before,
    /// takes its "new" mark off (spec §2, item 4). Cheap when the selection has not changed, so the screen calls it on
    /// every frame.
    pub(crate) fn open_selected(&mut self) {
        if self.ui.activities.selected == self.detail_for {
            return;
        }
        // A set is named within its activity: an editor or a picker left open would act on another activity's set.
        self.ui.activities.editing = None;
        self.ui.activities.picker = None;
        self.read_detail();
        let Some(id) = self.detail.as_ref().filter(|detail| detail.summary.is_new).map(|detail| detail.summary.id.0) else { return };
        let mut params = Map::new();
        params.insert("id".to_owned(), Value::from(id));
        // A change: the list, the badge and the open activity are read again.
        if let Err(text) = self.run("activity.markSeen", Value::Object(params)) {
            self.report(&text);
        }
    }

    /// The exercises with strength history, as of the last refresh, in the order the tiles are shown.
    pub fn progress(&self) -> &[ProgressItemView] {
        &self.progress
    }

    /// The open exercise, as last read; `None` when none is open or it could not be read.
    pub(crate) fn progress_detail(&self) -> Option<&ProgressView> {
        self.progress_detail.as_ref()
    }

    /// Why the open exercise could not be read, in the user's language.
    pub(crate) fn progress_detail_error(&self) -> Option<&str> {
        self.progress_detail_error.as_deref()
    }

    /// Opens the page of the exercise `key` (`progress.get`) at once, or closes the open one with `None`.
    pub(crate) fn open_exercise(&mut self, key: Option<String>) {
        self.ui.progress.open = key;
        self.read_progress_detail();
    }

    /// Opens the activity `id` in Activities, from a session or a point of an exercise's page: the whole list, the activity
    /// selected and read, its "new" mark taken off as when the list selects it. The exercise open on Progress stays open.
    pub(crate) fn open_activity(&mut self, id: i64) {
        self.ui.section = Section::Activities;
        self.ui.activities.filter = ListFilter::All;
        self.ui.activities.selected = Some(id);
        self.open_selected();
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

    /// The current time: the system's, or the one the tests fixed.
    pub(crate) fn now(&self) -> DateTime<Utc> {
        self.fixed_time.unwrap_or_else(Utc::now)
    }

    /// Whether the import that starts with the window is still to come: until it has started, an empty list is waiting
    /// for it, not empty.
    pub(crate) fn startup_pending(&self) -> bool {
        self.startup != Startup::Done
    }

    /// `at` as a date and time on the user's wall clock.
    pub(crate) fn local_time(&self, at: DateTime<Utc>) -> NaiveDateTime {
        match self.utc_offset {
            Some(offset) => at.with_timezone(&offset).naive_local(),
            None => at.with_timezone(&Local).naive_local(),
        }
    }

    /// Tells the user something went wrong that no screen has a place for, in the banner at the top. It stays there until
    /// the user next changes something that works.
    pub(crate) fn report(&mut self, text: &str) {
        self.reported = Some(text.to_owned());
    }

    fn execute(&mut self, id: &str, params: Value) -> Result<Outcome, EngineError> {
        self.session.execute(id, params, Caller::User).inspect_err(|error| match error {
            // The rejected params may hold what the user typed, so only the command is logged.
            EngineError::BadParams { cmd, .. } => log::warn!("{cmd} refused its params"),
            error => log::warn!("{id} failed: {error}"),
        })
    }

    /// Reads the language, the connection and the activities again.
    fn refresh(&mut self) {
        self.error = None;
        match self.session.language() {
            Ok(language) => {
                self.language = language;
                let lang = i18n::resolve(language, self.system_locale.as_deref());
                // The errors of the connect form, of the set being edited and of the picker are kept as text in the
                // language they were made in: in another language they would be stale sentences, so they go.
                if self.shown_lang.is_some_and(|shown| shown != lang) {
                    self.ui.connect.error = None;
                    let activities = &mut self.ui.activities;
                    if let Some(edit) = activities.editing.as_mut() {
                        edit.error = None;
                    }
                    if let Some(picker) = activities.picker.as_mut() {
                        picker.error = None;
                    }
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
        // A list that cannot be read keeps what was read last: it is still the best there is to show.
        match self.read_activities() {
            Ok(activities) => self.activities = activities,
            Err(text) => self.error = Some(text),
        }
        match self.read_progress() {
            Ok(progress) => {
                self.progress = progress;
                // Only a list that was read can say the open exercise is gone; one that was not says nothing.
                if self.ui.progress.open.as_ref().is_some_and(|key| !self.progress.iter().any(|item| item.exercise.id == *key)) {
                    self.ui.progress.open = None;
                }
            }
            Err(text) => self.error = Some(text),
        }
    }

    /// Every exercise with strength history: `progress.list` in one answer.
    fn read_progress(&mut self) -> Result<Vec<ProgressItemView>, String> {
        let mut params = Map::new();
        params.insert("limit".to_owned(), Value::from(PROGRESS_LIMIT));
        let items = match self.run("progress.list", Value::Object(params))? {
            Outcome::Done(value) => {
                serde_json::from_value::<ProgressListView>(value)
                    .map_err(|error| {
                        log::error!("cannot read the answer of progress.list: {error}");
                        tl!("Something went wrong. Please try again.").to_owned()
                    })?
                    .items
            }
            Outcome::Started(_) => return Err(tl!("Something went wrong. Please try again.").to_owned()),
        };
        if items.len() >= PROGRESS_LIMIT as usize {
            log::warn!("progress.list gave all {PROGRESS_LIMIT} it can at once; the window shows these");
        }
        Ok(items)
    }

    /// Every activity, newest first: `activity.list` page by page, until a page comes back short.
    fn read_activities(&mut self) -> Result<Vec<ActivityListItemView>, String> {
        let mut activities = Vec::new();
        for page in 0..LIST_MAX_PAGES {
            // Built by hand, not with `json!`, which unwraps inside.
            let mut params = Map::new();
            params.insert("limit".to_owned(), Value::from(LIST_PAGE));
            params.insert("offset".to_owned(), Value::from(page.saturating_mul(LIST_PAGE)));
            let items = match self.run("activity.list", Value::Object(params))? {
                Outcome::Done(value) => {
                    serde_json::from_value::<ActivityListView>(value)
                        .map_err(|error| {
                            log::error!("cannot read the answer of activity.list: {error}");
                            tl!("Something went wrong. Please try again.").to_owned()
                        })?
                        .items
                }
                Outcome::Started(_) => return Err(tl!("Something went wrong. Please try again.").to_owned()),
            };
            let full = items.len() >= LIST_PAGE as usize;
            activities.extend(items);
            if !full {
                return Ok(activities);
            }
        }
        log::warn!("the activity list is longer than {} pages; the window shows the newest {}", LIST_MAX_PAGES, activities.len());
        Ok(activities)
    }

    /// Reads the open activity again when it may be stale: always with `force` (a command ran, a job ended, another process
    /// wrote), else only when its line in the list no longer matches what was read, as when its file arrives during a
    /// sync, which refreshes the window on every frame.
    fn refresh_detail(&mut self, force: bool) {
        let Some(id) = self.detail_for else { return };
        let changed = self.detail.as_ref().is_some_and(|detail| self.activities.iter().find(|item| item.id.0 == id) != Some(&detail.summary));
        if force || changed {
            self.read_detail();
        }
    }

    /// Reads the open exercise again when it may be stale: always with `force` (a command ran, a job ended, another process
    /// wrote), else only when `ui.progress.open` is not the key it was read for (the user opened another, or the refresh
    /// closed one that is gone). Nothing is read while none is open.
    fn refresh_progress_detail(&mut self, force: bool) {
        if self.ui.progress.open != self.progress_detail_for || (force && self.ui.progress.open.is_some()) {
            self.read_progress_detail();
        }
    }

    /// Reads the exercise `ui.progress.open` names (`progress.get`); none when none is open.
    fn read_progress_detail(&mut self) {
        let open = self.ui.progress.open.clone();
        self.progress_detail_for = open.clone();
        self.progress_detail = None;
        self.progress_detail_error = None;
        let Some(key) = open else { return };
        #[cfg(test)]
        {
            self.progress_detail_reads += 1;
        }
        let mut params = Map::new();
        params.insert("exercise".to_owned(), Value::from(key));
        let read = match self.run("progress.get", Value::Object(params)) {
            Ok(Outcome::Done(value)) => serde_json::from_value::<ProgressView>(value).map_err(|error| {
                log::error!("cannot read the answer of progress.get: {error}");
                tl!("Something went wrong. Please try again.").to_owned()
            }),
            Ok(Outcome::Started(_)) => Err(tl!("Something went wrong. Please try again.").to_owned()),
            Err(text) => Err(text),
        };
        match read {
            Ok(detail) => self.progress_detail = Some(detail),
            Err(text) => self.progress_detail_error = Some(text),
        }
    }

    /// Reads the selected activity (`activity.get`). One that is not listed (any more) opens nothing.
    fn read_detail(&mut self) {
        let selected = self.ui.activities.selected;
        self.detail_for = selected;
        self.detail = None;
        self.detail_error = None;
        let Some(id) = selected.filter(|id| self.activities.iter().any(|item| item.id.0 == *id)) else { return };
        #[cfg(test)]
        {
            self.detail_reads += 1;
        }
        let mut params = Map::new();
        params.insert("id".to_owned(), Value::from(id));
        let read = match self.run("activity.get", Value::Object(params)) {
            Ok(Outcome::Done(value)) => serde_json::from_value::<ActivityView>(value).map_err(|error| {
                log::error!("cannot read the answer of activity.get: {error}");
                tl!("Something went wrong. Please try again.").to_owned()
            }),
            Ok(Outcome::Started(_)) => Err(tl!("Something went wrong. Please try again.").to_owned()),
            Err(text) => Err(text),
        };
        match read {
            Ok(detail) => self.detail = Some(detail),
            Err(text) => self.detail_error = Some(text),
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

    /// The Garmin card learns how the job it waits for ended: the key check of its form, or the disconnect the user
    /// confirmed. The end of the start-up re-parse lets the start-up sync go, and so does a key Garmin accepted.
    fn job_finished(&mut self, job: JobFinished) {
        if self.startup == Startup::Reparsing(job.id) {
            if let Err(error) = &job.result {
                log::warn!("the start-up re-parse of the stored files failed: {error}");
            }
            self.startup = Startup::SyncDue;
            return;
        }
        let form = &mut self.ui.connect;
        if form.pending == Some(job.id) {
            form.pending = None;
            match &job.result {
                Ok(_) => {
                    form.key.clear();
                    form.error = None;
                    form.replacing = false;
                    self.import_after_connect();
                }
                // The key stays in the field so the user can fix it.
                Err(error) => form.error = Some(error_text(error).to_owned()),
            }
        } else if form.disconnecting == Some(job.id) {
            form.disconnecting = None;
            match &job.result {
                Ok(_) => {
                    form.error = None;
                    form.replacing = false;
                }
                // The connection is still there: the card says why, and "Disconnect" can be tried again.
                Err(error) => form.error = Some(error_text(error).to_owned()),
            }
        }
    }

    /// Starts the import a connected app makes when it opens, once (Ruling 4): the re-parse of the stored files first
    /// (a job that ends at once when this version has nothing to parse better), then, when its end has been applied, the
    /// sync, which begins with the key check the app used to make on its own. The two never run together. What becomes of
    /// them is shown through the status; a start that fails is logged, not an error of the user's.
    fn start_once(&mut self) {
        match self.startup {
            Startup::Pending => {
                // Without a status there is nothing to decide on: the import is neither started nor forgotten, and the
                // next tick looks again.
                let Some(status) = &self.status else { return };
                if status.status == ConnectionStatus::NotConnected {
                    self.startup = Startup::Done;
                    return;
                }
                match self.start("activity.reparse", Value::Null) {
                    Ok(Outcome::Started(job)) => self.startup = Startup::Reparsing(job),
                    Ok(Outcome::Done(_)) => {
                        log::warn!("activity.reparse finished at once, though it is a background job");
                        self.start_sync();
                    }
                    Err(reason) => {
                        log::info!("the start-up re-parse did not start: {reason}");
                        self.start_sync();
                    }
                }
            }
            Startup::SyncDue => self.start_sync(),
            Startup::Reparsing(_) | Startup::Done => {}
        }
    }

    /// Ruling 19: Garmin was just connected, so its workouts are loaded at once (spec §2, items 1 and 16) instead of
    /// waiting for "Refresh". The start-up import's sync does it when that is still to come or under way; else the sync
    /// starts at this tick, never beside a re-parse.
    fn import_after_connect(&mut self) {
        if self.startup == Startup::Done {
            self.startup = Startup::SyncDue;
        }
    }

    fn start_sync(&mut self) {
        self.startup = Startup::Done;
        if let Err(reason) = self.start("garmin.sync", Value::Null) {
            log::info!("the start-up sync did not start: {reason}");
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
        // A 404 never reaches the user as such (a missing activity file is a state of the activity, not a failure), so
        // where it would, Garmin answered something it should not have.
        EngineError::Garmin(GarminAuthError::Unavailable | GarminAuthError::Unexpected | GarminAuthError::NotFound) => {
            tl!("Garmin is unavailable right now. Try again later.")
        }
        EngineError::FileStoreConsentRequired => tl!("The system password store is unavailable. Allow saving the key to a file to continue."),
        EngineError::NotConnected => tl!("Garmin Connect is not connected."),
        EngineError::KeyMissing => tl!("The Garmin key was not found in the password store. Connect Garmin again."),
        EngineError::Secrets(_) => tl!("Gymgar could not use the password store. Check that it is unlocked and try again."),
        EngineError::Store(_) => tl!("Gymgar could not access its data. Check that the disk is available and try again."),
        EngineError::BadParams { .. } => tl!("The value is not valid. Check it and try again."),
        EngineError::Disabled { .. } => tl!("This is not available right now."),
        EngineError::UnknownCommand(_)
        | EngineError::AgentNotAllowed(_)
        | EngineError::JobPanicked(_)
        | EngineError::JobDropped(_)
        | EngineError::CommandPanicked(_) => tl!("Something went wrong. Please try again."),
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
