//! The open session: the database, the injected services, the background jobs and the single entry point for
//! commands.
//!
//! The session keeps no copy of what lives in the database (language, connection record): every query reads it
//! again, so a change written by another process (the CLI, an agent) is seen by the next query. It is `Send`, so a
//! server can keep it behind a mutex and run commands on a blocking thread.

use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gymgar_domain::Language;
use gymgar_store::{Store, StoreError};
use serde_json::Value;

use crate::jobs::{JobOutput, Jobs};
use crate::{Caller, EngineError, JobFinished, JobId, Outcome, Services, Spawner, commands};

pub struct Session {
    store: Store,
    services: Services,
    /// The store's `data_version` at the last [`Session::store_changed`]; `None` when it could not be read.
    data_version: Option<i64>,
    jobs: Jobs,
}

impl Session {
    pub fn new(store: Store, services: Services) -> Session {
        let data_version = store.data_version().ok();
        Session { store, services, data_version, jobs: Jobs::new() }
    }

    /// Runs command `id`. Checks, in order: the command exists, an agent may run it, it is enabled now; then the
    /// command parses `params` and runs. A panic in the command is caught and becomes [`EngineError::CommandPanicked`]
    /// (the last line of defence of CLAUDE.md): the UI calls this on its own thread, where a panic would end the app.
    pub fn execute(&mut self, id: &str, params: Value, caller: Caller) -> Result<Outcome, EngineError> {
        let spec = commands::find(id).ok_or_else(|| EngineError::UnknownCommand(id.to_owned()))?;
        if caller == Caller::Agent && !spec.agent {
            return Err(EngineError::AgentNotAllowed(id.to_owned()));
        }
        guarded(spec.id, || {
            (spec.enabled)(self).map_err(|reason| EngineError::Disabled { cmd: id.to_owned(), reason })?;
            // Params may hold a secret: only the id is logged.
            log::debug!("running {id} for {caller:?}");
            (spec.run)(self, params)
        })
    }

    /// Whether another connection (another process) committed to the database since the last call. The UI polls
    /// this to pick up, say, a language an agent changed. Our own writes do not count; a failed read reports no change.
    pub fn store_changed(&mut self) -> bool {
        match self.store.data_version() {
            Ok(version) => {
                let changed = self.data_version != Some(version);
                self.data_version = Some(version);
                changed
            }
            Err(error) => {
                log::debug!("cannot read the database data_version: {error}");
                false
            }
        }
    }

    /// The interface language. A stored value this version cannot read falls back to `System` (with a warning), so
    /// a damaged setting never stops the app from starting; choosing a language again repairs it.
    pub fn language(&self) -> Result<Language, EngineError> {
        match self.store.language() {
            Ok(language) => Ok(language),
            Err(StoreError::Corrupt(reason)) => {
                log::warn!("the stored interface language is unusable, using the system language: {reason}");
                Ok(Language::System)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Applies the results of the background jobs that have finished since the last call, on the calling thread,
    /// and returns them in the order they finished. The UI calls this every frame; a job counts as running (and
    /// keeps commands such as `garmin.connect` disabled) until its result has been applied here. A panic while a
    /// result is applied becomes [`EngineError::CommandPanicked`] for that job, and the other results still come.
    pub fn poll_jobs(&mut self) -> Vec<JobFinished> {
        let mut finished = Vec::new();
        while let Some((id, command, output)) = self.jobs.take_finished() {
            let result = output.and_then(|output| guarded(command, || output.apply(self)));
            if let Err(error) = &result {
                log::info!("job {} of {command} failed: {error}", id.0);
            }
            finished.push(JobFinished { id, command, result });
        }
        finished
    }

    /// Starts `fut` as a background job of `command` on `spawner`.
    pub(crate) fn start_job<F>(&mut self, command: &'static str, spawner: &Arc<dyn Spawner>, fut: F) -> JobId
    where
        F: Future<Output = JobOutput> + Send + 'static,
    {
        let id = self.jobs.start(command, spawner.as_ref(), self.services.notify.clone(), fut);
        log::debug!("started job {} of {command}", id.0);
        id
    }

    /// The commands of the background jobs that have not been applied yet.
    pub(crate) fn running_jobs(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.jobs.running()
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn services(&self) -> &Services {
        &self.services
    }

    /// The current time from the injected clock.
    pub(crate) fn now(&self) -> DateTime<Utc> {
        (self.services.clock)()
    }
}

/// Runs `f`, the body of command `command` on the session's thread, and turns a panic in it into
/// [`EngineError::CommandPanicked`]. The panic's message is not kept: it is whatever text the panicking code chose and
/// may hold the params (a key); the panic hook has already printed it to stderr.
fn guarded<T>(command: &str, f: impl FnOnce() -> Result<T, EngineError>) -> Result<T, EngineError> {
    // `AssertUnwindSafe`: the session keeps nothing a half-run command could leave inconsistent in memory (every command
    // reads its state from the database again), so it stays usable. What a panic can leave half-done is the command's own
    // work outside one transaction (a key saved, its record not yet): the safety net does not undo that.
    panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        log::error!("command {command} panicked");
        Err(EngineError::CommandPanicked(command.to_owned()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretStore;
    use crate::testing::{TestConfig, connection_at, fixed_clock, save_connection, saved_connection, test_session, test_session_on, test_session_with};
    use gymgar_domain::SecretBackend;
    use secrecy::SecretString;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn session_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Session>();
    }

    #[test]
    fn unknown_command_is_error() {
        let mut session = test_session();
        for caller in [Caller::User, Caller::Agent] {
            assert_eq!(session.execute("garmin.teleport", Value::Null, caller), Err(EngineError::UnknownCommand("garmin.teleport".to_owned())));
            assert_eq!(session.execute("", json!({}), caller), Err(EngineError::UnknownCommand(String::new())));
        }
    }

    #[test]
    fn store_changed_tracks_other_connections() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("gymgar.sqlite3");
        let (mut session, _) = test_session_on(Store::open(&path).unwrap(), TestConfig::default());
        let other = Store::open(&path).unwrap();
        assert!(!session.store_changed());

        // Our own write is not a change from elsewhere.
        session.execute("settings.setLanguage", json!({"language": "en"}), Caller::User).unwrap();
        assert!(!session.store_changed());

        other.set_language(Language::Ru).unwrap();
        assert!(session.store_changed());
        assert!(!session.store_changed(), "a change is reported once");
        // Nothing is cached: the next query sees the other connection's write.
        assert_eq!(session.execute("settings.get", Value::Null, Caller::User), Ok(Outcome::Done(json!({"language": "ru"}))));
    }

    /// A clock that panics, standing in for any bug in code a command runs.
    fn broken_clock() -> Arc<dyn Fn() -> DateTime<Utc> + Send + Sync> {
        Arc::new(|| panic!("the clock broke"))
    }

    #[test]
    fn a_panicking_command_is_an_error_and_the_session_keeps_working() {
        let cfg = TestConfig::default();
        let (mut session, _) = test_session_with(cfg.clone());
        // `garmin.status` reads the clock for the expiry warning.
        session.services.clock = broken_clock();
        for caller in [Caller::User, Caller::Agent] {
            assert_eq!(session.execute("garmin.status", Value::Null, caller), Err(EngineError::CommandPanicked("garmin.status".to_owned())));
        }
        // Commands that do not need the clock still run, and write to the database.
        session.execute("settings.setLanguage", json!({"language": "ru"}), Caller::User).unwrap();
        assert_eq!(session.execute("settings.get", Value::Null, Caller::Agent), Ok(Outcome::Done(json!({"language": "ru"}))));
        // Once the clock works again, so does the command that panicked.
        session.services.clock = fixed_clock(cfg.now);
        assert!(matches!(session.execute("garmin.status", Value::Null, Caller::User), Ok(Outcome::Done(_))));
    }

    #[test]
    fn a_panic_while_applying_a_job_is_an_error_and_the_session_keeps_working() {
        let cfg = TestConfig::default();
        let (mut session, handles) = test_session_with(cfg.clone());
        let connection = connection_at(cfg.now, SecretBackend::System);
        save_connection(&session, &connection);
        handles.system.as_ref().unwrap().set(&SecretString::from("k-123")).unwrap();
        // The check itself does not read the clock; recording that Garmin accepted the key does.
        session.services.clock = broken_clock();
        let Ok(Outcome::Started(job)) = session.execute("garmin.verify", Value::Null, Caller::User) else { panic!("verify did not start") };

        let finished = session.poll_jobs();
        assert_eq!(finished, [JobFinished { id: job, command: "garmin.verify", result: Err(EngineError::CommandPanicked("garmin.verify".to_owned())) }]);
        assert!(session.poll_jobs().is_empty(), "a result is handed out once");
        // Nothing was recorded, and the check no longer counts as running: another one can start.
        assert_eq!(saved_connection(&session), Some(connection));
        session.services.clock = fixed_clock(cfg.now);
        assert!(matches!(session.execute("garmin.verify", Value::Null, Caller::User), Ok(Outcome::Started(_))));
        assert!(matches!(session.poll_jobs().as_slice(), [JobFinished { result: Ok(_), .. }]));
    }

    #[test]
    fn corrupt_language_falls_back_to_system() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("gymgar.sqlite3");
        let (mut session, _) = test_session_on(Store::open(&path).unwrap(), TestConfig::default());
        rusqlite::Connection::open(&path).unwrap().execute("INSERT INTO app_settings (key, value) VALUES ('language', 'klingon')", []).unwrap();

        assert_eq!(session.language(), Ok(Language::System));
        assert_eq!(session.execute("settings.get", Value::Null, Caller::Agent), Ok(Outcome::Done(json!({"language": "system"}))));
        // Choosing a language repairs the setting.
        session.execute("settings.setLanguage", json!({"language": "ru"}), Caller::User).unwrap();
        assert_eq!(session.language(), Ok(Language::Ru));
    }
}
