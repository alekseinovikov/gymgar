//! The open session: the database, the injected services, the background jobs and the single entry point for
//! commands.
//!
//! The session keeps no copy of what lives in the database (language, connection record): every query reads it
//! again, so a change written by another process (the CLI, an agent) is seen by the next query. What it does keep in
//! memory belongs to this run only: the Garmin session the last sync opened, and the progress of a running sync. It is
//! `Send`, so a server can keep it behind a mutex and run commands on a blocking thread.

use std::collections::BTreeSet;
use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gymgar_domain::Language;
use gymgar_store::{Store, StoreError};
use serde_json::Value;

use crate::jobs::{JobCtx, JobMessage, JobOutput, Jobs};
use crate::{Caller, EngineError, GarminSession, JobEvent, JobFinished, JobId, Outcome, Services, Spawner, commands};

/// How many messages of background jobs one [`Session::poll_jobs`] applies at most. Each is a transaction, and the UI
/// polls once a frame: a sync that has sent hundreds of results must not hold one frame up with all of them.
const POLL_BUDGET: usize = 32;

pub struct Session {
    store: Store,
    services: Services,
    /// The store's `data_version` at the last [`Session::store_changed`]; `None` when it could not be read.
    data_version: Option<i64>,
    jobs: Jobs,
    /// The Garmin session of the last sync, kept while Garmin accepts its key, so the next sync of this run neither
    /// reads the key nor exchanges it again. Dropped when the key changes (connect, disconnect) or Garmin refuses it.
    garmin_session: Option<Arc<dyn GarminSession>>,
    /// Files done and files to fetch of the running sync, as its job last reported them.
    sync_progress: Option<(u32, u32)>,
    /// Running jobs a partial result of which failed to apply. The job never learns it, so its end must not count as a
    /// clean run: a sync that lost a page must not seal the history or record a success.
    failed_partials: BTreeSet<JobId>,
}

impl Session {
    pub fn new(store: Store, services: Services) -> Session {
        let data_version = store.data_version().ok();
        Session { store, services, data_version, jobs: Jobs::new(), garmin_session: None, sync_progress: None, failed_partials: BTreeSet::new() }
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

    /// Applies what the background jobs have sent since the last call (partial results and ends), on the calling
    /// thread, one by one in the order they were sent, and returns an event for each. The UI calls this every frame; a
    /// job counts as running (and keeps commands such as `garmin.connect` disabled) until its end has been applied here.
    /// A panic while an output is applied becomes [`EngineError::CommandPanicked`] for that event, and the other events
    /// still come; a partial result that fails to apply does not stop its job.
    ///
    /// One call applies at most a few dozen messages. When it stops there, it calls `Services::notify`, so the frontend
    /// calls again for the rest (on the next frame, in the app) instead of one frame applying them all.
    pub fn poll_jobs(&mut self) -> Vec<JobEvent> {
        let mut events = Vec::new();
        while events.len() < POLL_BUDGET {
            let Some((id, command, message)) = self.jobs.take() else { return events };
            match message {
                JobMessage::Partial(output) => {
                    let result = guarded(command, || output.apply(self, id)).map(|_| ());
                    if let Err(error) = &result {
                        log::info!("a partial result of job {} of {command} failed: {error}", id.0);
                        self.failed_partials.insert(id);
                    }
                    events.push(JobEvent::Partial { id, command, result });
                }
                JobMessage::Finished(output) => {
                    let result = output.and_then(|output| guarded(command, || output.apply(self, id)));
                    // The job is over, however it ended: nothing of it is left to judge.
                    self.failed_partials.remove(&id);
                    if let Err(error) = &result {
                        log::info!("job {} of {command} failed: {error}", id.0);
                    }
                    events.push(JobEvent::Finished(JobFinished { id, command, result }));
                }
            }
        }
        // The budget is spent and more may be waiting (if exactly the budget was waiting, the frontend polls once for
        // nothing).
        (self.services.notify)();
        events
    }

    /// Starts the job `make` builds as a background job of `command` on `spawner`. The job gets a [`JobCtx`] for its
    /// partial results and for sleeping with the session's sleep.
    pub(crate) fn start_job<M, F>(&mut self, command: &'static str, spawner: &Arc<dyn Spawner>, make: M) -> JobId
    where
        M: FnOnce(JobCtx) -> F + Send + 'static,
        F: Future<Output = JobOutput> + Send + 'static,
    {
        let id = self.jobs.start(command, spawner.as_ref(), self.services.notify.clone(), self.services.sleep.clone(), make);
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

    /// Whether a partial result of the running job `job` failed to apply.
    pub(crate) fn partial_failed(&self, job: JobId) -> bool {
        self.failed_partials.contains(&job)
    }

    /// The Garmin session the next sync may reuse.
    pub(crate) fn garmin_session(&self) -> Option<Arc<dyn GarminSession>> {
        self.garmin_session.clone()
    }

    pub(crate) fn set_garmin_session(&mut self, session: Option<Arc<dyn GarminSession>>) {
        self.garmin_session = session;
    }

    /// Files done and files to fetch of the running sync; `None` before its job has said.
    pub(crate) fn sync_progress(&self) -> Option<(u32, u32)> {
        self.sync_progress
    }

    pub(crate) fn set_sync_progress(&mut self, progress: Option<(u32, u32)>) {
        self.sync_progress = progress;
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
    use crate::jobs::probe;
    use crate::testing::{
        DeferredSpawner, DroppingSpawner, InlineSpawner, TestConfig, connection_at, fixed_clock, save_connection, saved_connection, test_session,
        test_session_on, test_session_with,
    };
    use crate::{JobEvent, SecretStore};
    use gymgar_domain::SecretBackend;
    use secrecy::SecretString;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
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
        assert_eq!(
            finished,
            [JobEvent::Finished(JobFinished { id: job, command: "garmin.verify", result: Err(EngineError::CommandPanicked("garmin.verify".to_owned())) })]
        );
        assert!(session.poll_jobs().is_empty(), "a result is handed out once");
        // Nothing was recorded, and the check no longer counts as running: another one can start.
        assert_eq!(saved_connection(&session), Some(connection));
        session.services.clock = fixed_clock(cfg.now);
        assert!(matches!(session.execute("garmin.verify", Value::Null, Caller::User), Ok(Outcome::Started(_))));
        assert!(matches!(session.poll_jobs().as_slice(), [JobEvent::Finished(JobFinished { result: Ok(_), .. })]));
    }

    fn partial(id: JobId, result: Result<(), EngineError>) -> JobEvent {
        JobEvent::Partial { id, command: "x.cmd", result }
    }

    fn finished(id: JobId, result: Result<Value, EngineError>) -> JobEvent {
        JobEvent::Finished(JobFinished { id, command: "x.cmd", result })
    }

    /// A session whose jobs wait for `DeferredSpawner::run_pending`, and that spawner.
    fn deferred_session() -> (Session, Arc<DeferredSpawner>) {
        let (session, handles) = test_session_with(TestConfig { deferred: true, ..TestConfig::default() });
        let spawner = handles.spawner.expect("a deferred spawner");
        (session, spawner)
    }

    /// Runs the waiting jobs on another thread, as the app's runtime does.
    fn run_elsewhere(spawner: &Arc<DeferredSpawner>) -> std::thread::JoinHandle<()> {
        let spawner = spawner.clone();
        std::thread::spawn(move || spawner.run_pending())
    }

    /// Polls until `count` events have come from a job running elsewhere, for at most five seconds.
    fn poll_until(session: &mut Session, count: usize) -> Vec<JobEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while events.len() < count && Instant::now() < deadline {
            events.extend(session.poll_jobs());
            std::thread::sleep(Duration::from_millis(1));
        }
        events
    }

    #[test]
    fn partials_arrive_in_order_before_finish() {
        probe::take_applied();
        let (mut session, spawner) = deferred_session();
        let (open, gate) = mpsc::channel::<()>();
        let id = session.start_job("x.cmd", &(spawner.clone() as Arc<dyn Spawner>), move |ctx| async move {
            for n in 1..=3 {
                ctx.send(JobOutput::Probe(n));
            }
            // Holds its thread, as a job on the app's runtime may, until the test has seen the partial results.
            let _ = gate.recv_timeout(Duration::from_secs(5));
            JobOutput::Probe(4)
        });
        let runner = run_elsewhere(&spawner);

        assert_eq!(poll_until(&mut session, 3), vec![partial(id, Ok(())); 3]);
        assert_eq!(probe::take_applied(), [1, 2, 3], "applied in the order they were sent");
        assert_eq!(session.running_jobs().collect::<Vec<_>>(), ["x.cmd"], "the job runs until its end is applied");

        open.send(()).unwrap();
        runner.join().unwrap();
        assert_eq!(session.poll_jobs(), [finished(id, Ok(json!(4)))]);
        assert_eq!(probe::take_applied(), [4]);
        assert_eq!(session.running_jobs().count(), 0);
        assert!(session.poll_jobs().is_empty(), "every event is handed out once");
    }

    #[test]
    fn partial_apply_panic_is_error_and_job_continues() {
        probe::take_applied();
        let (mut session, spawner) = deferred_session();
        let (open, gate) = mpsc::channel::<()>();
        let id = session.start_job("x.cmd", &(spawner.clone() as Arc<dyn Spawner>), move |ctx| async move {
            ctx.send(JobOutput::Probe(probe::PANICS));
            let _ = gate.recv_timeout(Duration::from_secs(5));
            ctx.send(JobOutput::Probe(probe::FAILS));
            ctx.send(JobOutput::Probe(2));
            JobOutput::Probe(3)
        });
        let runner = run_elsewhere(&spawner);

        assert_eq!(poll_until(&mut session, 1), [partial(id, Err(EngineError::CommandPanicked("x.cmd".to_owned())))]);
        assert_eq!(session.running_jobs().collect::<Vec<_>>(), ["x.cmd"], "the job goes on");

        open.send(()).unwrap();
        runner.join().unwrap();
        assert_eq!(
            session.poll_jobs(),
            [partial(id, Err(EngineError::Store("probe 500 failed to apply".to_owned()))), partial(id, Ok(())), finished(id, Ok(json!(3)))]
        );
        assert_eq!(probe::take_applied(), [2, 3]);
        assert_eq!(session.running_jobs().count(), 0);
        // The session keeps working.
        session.execute("settings.setLanguage", json!({"language": "ru"}), Caller::User).unwrap();
        assert_eq!(session.language(), Ok(Language::Ru));
    }

    #[test]
    fn dropped_job_is_reported_once() {
        probe::take_applied();
        let (mut session, handles) = test_session_with(TestConfig::default());
        let spawner: Arc<dyn Spawner> = Arc::new(DroppingSpawner);
        let id = session.start_job("x.cmd", &spawner, |_| async { JobOutput::Probe(1) });
        assert_eq!(session.running_jobs().collect::<Vec<_>>(), ["x.cmd"], "running until the news is taken");
        assert_eq!(handles.notifications.load(Ordering::SeqCst), 1, "the frontend is woken up to take the news");

        assert_eq!(session.poll_jobs(), [finished(id, Err(EngineError::JobDropped("x.cmd".to_owned())))]);
        assert_eq!(session.running_jobs().count(), 0, "a dropped job no longer runs");
        assert!(session.poll_jobs().is_empty(), "reported once");
        assert!(probe::take_applied().is_empty(), "the job never ran");
    }

    #[test]
    fn poll_jobs_applies_a_bounded_number_of_results_per_call() {
        probe::take_applied();
        let (mut session, handles) = test_session_with(TestConfig::default());
        let spawner: Arc<dyn Spawner> = Arc::new(InlineSpawner);
        let id = session.start_job("x.cmd", &spawner, |ctx| async move {
            for n in 1..=40 {
                ctx.send(JobOutput::Probe(n));
            }
            JobOutput::Probe(41)
        });
        let woken = handles.notifications.load(Ordering::SeqCst);

        let first = session.poll_jobs();
        assert_eq!(first, vec![partial(id, Ok(())); POLL_BUDGET], "one call applies at most the budget");
        assert_eq!(probe::take_applied(), (1..=32).collect::<Vec<_>>());
        assert_eq!(handles.notifications.load(Ordering::SeqCst), woken + 1, "the frontend is woken up to poll for the rest");
        assert_eq!(session.running_jobs().collect::<Vec<_>>(), ["x.cmd"]);

        let rest = session.poll_jobs();
        assert_eq!(rest.len(), 9);
        assert_eq!(rest.last(), Some(&finished(id, Ok(json!(41)))));
        assert_eq!(probe::take_applied(), (33..=41).collect::<Vec<_>>());
        assert_eq!(handles.notifications.load(Ordering::SeqCst), woken + 1, "nothing is left, so nobody is woken up");
        assert_eq!(session.running_jobs().count(), 0);
    }

    #[test]
    fn a_failed_partial_is_remembered_for_its_own_job_until_it_ends() {
        probe::take_applied();
        let (mut session, spawner) = deferred_session();
        let (open, gate) = mpsc::channel::<()>();
        let failing = session.start_job("x.cmd", &(spawner.clone() as Arc<dyn Spawner>), move |ctx| async move {
            ctx.send(JobOutput::Probe(probe::FAILS));
            let _ = gate.recv_timeout(Duration::from_secs(5));
            JobOutput::Probe(1)
        });
        let clean = session.start_job("x.cmd", &(spawner.clone() as Arc<dyn Spawner>), |ctx| async move {
            ctx.send(JobOutput::Probe(2));
            JobOutput::Probe(3)
        });
        let runner = run_elsewhere(&spawner);

        assert_eq!(poll_until(&mut session, 1), [partial(failing, Err(EngineError::Store("probe 500 failed to apply".to_owned())))]);
        assert!(session.partial_failed(failing), "remembered while the job runs");
        assert!(!session.partial_failed(clean), "for that job only");

        open.send(()).unwrap();
        runner.join().unwrap();
        assert_eq!(
            session.poll_jobs(),
            [finished(failing, Ok(json!(1))), partial(clean, Ok(())), finished(clean, Ok(json!(3)))],
            "the failure is the job's own business to judge at its end"
        );
        assert!(!session.partial_failed(failing), "forgotten once the job has ended");
        assert!(!session.partial_failed(clean));
        assert_eq!(probe::take_applied(), [1, 2, 3]);
    }

    #[test]
    fn recording_sleep_records() {
        let (mut session, handles) = test_session_with(TestConfig::default());
        let spawner: Arc<dyn Spawner> = Arc::new(InlineSpawner);
        session.start_job("x.cmd", &spawner, |ctx| async move {
            ctx.sleep(Duration::from_secs(2)).await;
            ctx.sleep(Duration::from_secs(300)).await;
            JobOutput::Probe(1)
        });
        // The inline spawner ran the job to its end inside `start_job`: the sleeps returned at once.
        assert_eq!(handles.sleep.slept(), [Duration::from_secs(2), Duration::from_secs(300)]);
        assert!(matches!(session.poll_jobs().as_slice(), [JobEvent::Finished(JobFinished { result: Ok(_), .. })]));
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
