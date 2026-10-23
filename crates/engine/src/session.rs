//! The open session: the database, the injected services, the background jobs and the single entry point for
//! commands.
//!
//! The session keeps no copy of what lives in the database (language, connection record): every query reads it
//! again, so a change written by another process (the CLI, an agent) is seen by the next query. It is `Send`, so a
//! server can keep it behind a mutex and run commands on a blocking thread.

use std::future::Future;
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
    /// command parses `params` and runs.
    pub fn execute(&mut self, id: &str, params: Value, caller: Caller) -> Result<Outcome, EngineError> {
        let spec = commands::find(id).ok_or_else(|| EngineError::UnknownCommand(id.to_owned()))?;
        if caller == Caller::Agent && !spec.agent {
            return Err(EngineError::AgentNotAllowed(id.to_owned()));
        }
        (spec.enabled)(self).map_err(|reason| EngineError::Disabled { cmd: id.to_owned(), reason })?;
        // Params may hold a secret: only the id is logged.
        log::debug!("running {id} for {caller:?}");
        (spec.run)(self, params)
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
    /// keeps commands such as `garmin.connect` disabled) until its result has been applied here.
    pub fn poll_jobs(&mut self) -> Vec<JobFinished> {
        let mut finished = Vec::new();
        while let Some((id, command, output)) = self.jobs.take_finished() {
            let result = output.and_then(|output| output.apply(self));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestConfig, test_session, test_session_on};
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
