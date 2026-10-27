//! Background jobs: work a [`CommandKind::Job`](crate::CommandKind::Job) command starts on the injected
//! [`Spawner`], and the channel its results come back through.
//!
//! A job does the slow part: it asks Garmin, parses the files it downloads or reads again, and makes every call of a
//! secret store a command needs (reading the key, saving a key Garmin accepted, deleting it on disconnect), because the
//! system store may wait for the user to allow access, which must not freeze the window. Along the way it may send
//! partial results through its [`JobCtx`] (`garmin.sync` sends each page of the list and each file), and at the end it
//! returns its final [`JobOutput`]. A job never writes to the database: [`Session::poll_jobs`](crate::Session::poll_jobs)
//! applies every output on the session's thread (the UI thread in the app), in the order the job sent them, so every
//! database write happens there. A job counts as running from its start until `poll_jobs` has taken its end, even if
//! the future finished long before.
//!
//! What a secret store still has to do after an output was applied (put back a key, remove a replaced one) goes to the
//! spawner through [`spawn_tail`]: work without a job of its own, whose failures are only logged.
//!
//! Every job ends exactly once. A panic inside the job becomes [`EngineError::JobPanicked`] instead of taking the app
//! down, and a job the spawner drops before it finished (a runtime that shuts down, say) ends with
//! [`EngineError::JobDropped`], so it cannot count as running for the rest of the session.

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use futures::FutureExt;
use gymgar_domain::{ActivityId, KeyTerm, SecretBackend, SyncErrorKind};
use gymgar_store::{FileOutcome, NewActivity};
use secrecy::SecretString;
use serde_json::Value;

use crate::{BoxFuture, EngineError, GarminSession, JobId, Session, Spawner, garmin_cmds, sync_cmds};

/// A finished background job, as [`Session::poll_jobs`](crate::Session::poll_jobs) hands it out once its result has
/// been applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFinished {
    pub id: JobId,
    /// The id of the command that started the job.
    pub command: &'static str,
    /// The command's answer: a serialized result type, or why the job failed.
    pub result: Result<Value, EngineError>,
}

/// What [`Session::poll_jobs`](crate::Session::poll_jobs) hands out: a partial result of a job that is still running,
/// or the end of a job. A job's partial results all come before its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// A partial result was applied. An error here (applying it failed or panicked) does not stop the job.
    Partial {
        id: JobId,
        /// The id of the command that started the job.
        command: &'static str,
        result: Result<(), EngineError>,
    },
    Finished(JobFinished),
}

/// What a job's future hands back to the session: the outcome of the slow part only. Applying it (writing the connection
/// record, recording the status) is left to [`JobOutput::apply`] on the session's thread.
pub(crate) enum JobOutput {
    /// `garmin.connect` asked Garmin about the key and, if Garmin accepted it, saved it to `backend`'s store, chosen before
    /// the request. `saved` holds the key it replaced there (`None`: there was none), to put back should the connection
    /// record not be saved; or why the key was not saved: Garmin refused it, or the store failed.
    ConnectChecked { term: KeyTerm, backend: SecretBackend, saved: Result<Option<SecretString>, EngineError> },
    /// `garmin.disconnect` deleted the key from `backend`'s store, or could not.
    KeyDeleted { backend: SecretBackend, result: Result<(), EngineError> },
    /// `garmin.verify` read the recorded key and asked Garmin about it. `Err(KeyMissing)` or `Err(Secrets(..))`
    /// when there was no key to ask about.
    VerifyChecked { result: Result<(), EngineError> },
    /// `garmin.sync` asked Garmin about the key, or found no key to ask about, or Garmin refused the key later in the
    /// run: recorded as `garmin.verify` records its check. Partial.
    SyncVerified { result: Result<(), EngineError> },
    /// One page of Garmin's activity list, without what an earlier page of the same sync already brought. `beyond_history`:
    /// the history was complete and the page holds no activity stored before this sync, so the stored list has a gap until
    /// paging reaches it again. Partial.
    ActivityPage { items: Vec<NewActivity>, beyond_history: bool },
    /// Everything Garmin lists has been stored: the list has run out, or paging beyond the stored history has reached it
    /// again. Partial.
    HistoryComplete,
    /// What came of one activity's file, downloaded by `garmin.sync` or read again by `activity.reparse`. Partial.
    ActivityFile { id: ActivityId, outcome: FileOutcome },
    /// How many of the sync's files are done. Partial.
    Progress { done: u32, total: u32 },
    /// The end of `garmin.sync`: how it ended, and the Garmin session to keep for the next sync (`None`: the next one
    /// reads the key and asks Garmin about it again).
    SyncFinished { result: Result<(), SyncErrorKind>, session: Option<Arc<dyn GarminSession>> },
    /// The end of `activity.reparse`: how many stored files it read again.
    ReparseFinished { files: u32 },
    /// A test output, applied by [`probe::apply`].
    #[cfg(test)]
    Probe(u32),
}

impl JobOutput {
    /// Applies the output to the session and returns the command's answer (`null` for a partial result, which has none).
    /// `job` is the job that sent it.
    pub(crate) fn apply(self, session: &mut Session, job: JobId) -> Result<Value, EngineError> {
        match self {
            JobOutput::ConnectChecked { term, backend, saved } => garmin_cmds::apply_connect(session, term, backend, saved),
            JobOutput::KeyDeleted { backend, result } => garmin_cmds::apply_disconnect(session, backend, result),
            JobOutput::VerifyChecked { result } => garmin_cmds::apply_verify(session, result),
            JobOutput::SyncVerified { result } => sync_cmds::apply_verified(session, &result),
            JobOutput::ActivityPage { items, beyond_history } => sync_cmds::apply_page(session, &items, beyond_history),
            JobOutput::HistoryComplete => sync_cmds::apply_history_complete(session, job),
            JobOutput::ActivityFile { id, outcome } => sync_cmds::apply_file(session, id, outcome),
            JobOutput::Progress { done, total } => sync_cmds::apply_progress(session, done, total),
            JobOutput::SyncFinished { result, session: opened } => sync_cmds::apply_sync_finished(session, job, result, opened),
            JobOutput::ReparseFinished { files } => sync_cmds::apply_reparse_finished(files),
            #[cfg(test)]
            JobOutput::Probe(n) => probe::apply(n),
        }
    }
}

/// What a job sends the session.
pub(crate) enum JobMessage {
    /// A partial result; the job goes on.
    Partial(JobOutput),
    /// The job's end: its final output, or why there is none (it panicked, or it was dropped).
    Finished(Result<JobOutput, EngineError>),
}

/// A message on its way from a job to the session.
struct Delivery {
    id: JobId,
    message: JobMessage,
}

/// What a running job gets from the session: the way to send partial results, and the session's sleep.
pub(crate) struct JobCtx {
    id: JobId,
    command: &'static str,
    sender: Sender<Delivery>,
    notify: Arc<dyn Fn() + Send + Sync>,
    sleep: Arc<dyn Fn(Duration) -> BoxFuture<()> + Send + Sync>,
}

impl JobCtx {
    /// Sends a partial result: the session applies it on its own thread at its next `poll_jobs`, while the job goes on.
    /// The job does not learn whether applying it worked; the frontend does, through [`JobEvent::Partial`].
    pub(crate) fn send(&self, output: JobOutput) {
        if self.sender.send(Delivery { id: self.id, message: JobMessage::Partial(output) }).is_err() {
            // The session is gone (the app is closing): nobody is left to apply the result.
            log::debug!("job {} of {} sent a partial result after its session closed", self.id.0, self.command);
            return;
        }
        (self.notify)();
    }

    /// Waits `duration` with the session's sleep: the runtime's timer in the app, nothing at all in tests.
    pub(crate) fn sleep(&self, duration: Duration) -> BoxFuture<()> {
        (self.sleep)(duration)
    }
}

/// Sends a job's end to the session exactly once: the result handed to [`JobEnd::send`], or, if the job is dropped
/// before that, [`EngineError::JobDropped`].
///
/// It is made before the job's future and moved into it, so it exists even if the spawner drops the future without
/// ever polling it.
struct JobEnd {
    id: JobId,
    command: &'static str,
    /// `None` once the end has been sent.
    sender: Option<Sender<Delivery>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl JobEnd {
    /// Sends the job's end, and wakes the frontend up to take it.
    fn send(mut self, output: Result<JobOutput, EngineError>) {
        self.deliver(output);
    }

    fn deliver(&mut self, output: Result<JobOutput, EngineError>) {
        let Some(sender) = self.sender.take() else { return };
        if sender.send(Delivery { id: self.id, message: JobMessage::Finished(output) }).is_err() {
            // The session is gone (the app is closing): nobody is left to apply the result.
            log::debug!("job {} of {} ended after its session closed", self.id.0, self.command);
            return;
        }
        (self.notify)();
    }
}

impl Drop for JobEnd {
    fn drop(&mut self) {
        if self.sender.is_some() {
            log::warn!("job {} of {} was dropped before it finished", self.id.0, self.command);
            self.deliver(Err(EngineError::JobDropped(self.command.to_owned())));
        }
    }
}

/// The session's jobs: those still running and the channel their messages arrive in.
pub(crate) struct Jobs {
    next_id: u64,
    /// Started jobs whose end has not been taken yet, with the id of the command that started each.
    running: BTreeMap<JobId, &'static str>,
    sender: Sender<Delivery>,
    receiver: Receiver<Delivery>,
}

impl Jobs {
    pub(crate) fn new() -> Jobs {
        let (sender, receiver) = mpsc::channel();
        Jobs { next_id: 1, running: BTreeMap::new(), sender, receiver }
    }

    /// Starts the job `make` builds, on `spawner`, as a job of `command`. Its partial results and its end (or its panic,
    /// or that it was dropped) are sent to the session, and each wakes the frontend up with `notify` to call
    /// `poll_jobs`. `make` runs inside the job, so a panic there is the job's panic too.
    pub(crate) fn start<M, F>(
        &mut self,
        command: &'static str,
        spawner: &dyn Spawner,
        notify: Arc<dyn Fn() + Send + Sync>,
        sleep: Arc<dyn Fn(Duration) -> BoxFuture<()> + Send + Sync>,
        make: M,
    ) -> JobId
    where
        M: FnOnce(JobCtx) -> F + Send + 'static,
        F: Future<Output = JobOutput> + Send + 'static,
    {
        let id = JobId(self.next_id);
        // 2^64 jobs never happen in one session; saturating keeps the counter total all the same.
        self.next_id = self.next_id.saturating_add(1);
        // Marked running before it is spawned: an inline spawner finishes the job inside `spawn`.
        self.running.insert(id, command);
        let end = JobEnd { id, command, sender: Some(self.sender.clone()), notify: notify.clone() };
        let ctx = JobCtx { id, command, sender: self.sender.clone(), notify, sleep };
        spawner.spawn(Box::pin(async move {
            // The panic message is not kept: the job may hold a secret, and the message is whatever text the
            // panicking code chose. The panic hook has already reported it where the app's panics go.
            let output = AssertUnwindSafe(async move { make(ctx).await }).catch_unwind().await;
            end.send(output.map_err(|_| EngineError::JobPanicked(command.to_owned())));
        }));
        id
    }

    /// The commands of the jobs that are running.
    pub(crate) fn running(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.running.values().copied()
    }

    /// The next message a job has sent, with the job and the command that started it. A job whose end this is no
    /// longer counts as running.
    pub(crate) fn take(&mut self) -> Option<(JobId, &'static str, JobMessage)> {
        loop {
            let Delivery { id, message } = self.receiver.try_recv().ok()?;
            let command = match message {
                JobMessage::Partial(_) => self.running.get(&id).copied(),
                JobMessage::Finished(_) => self.running.remove(&id),
            };
            match command {
                Some(command) => return Some((id, command, message)),
                // A job sends its end once, after all its partial results, so this cannot happen; skipping it keeps the
                // queue moving.
                None => log::warn!("ignoring a message of job {}, which is not running", id.0),
            }
        }
    }
}

/// Runs `work` on `spawner` and does not wait for it. For what is left to do after a job's result was applied when it
/// touches a secret store (a key put back, the replaced key removed): the store may make the user wait for a system
/// prompt, so it must not run on the session's thread. It has no result and no job id: its failures are logged, and so
/// is a panic, which stays inside it. Work the spawner drops before it ran (a runtime that shuts down) is lost, like any
/// best-effort cleanup that fails.
///
/// Without a spawner the work runs at once, on the calling thread: done late rather than never. A session without a
/// spawner (the CLI) starts no jobs, so it never gets here.
pub(crate) fn spawn_tail(spawner: Option<&Arc<dyn Spawner>>, what: &'static str, work: impl FnOnce() + Send + 'static) {
    let guarded = move || {
        // `AssertUnwindSafe`: the work shares nothing with the session; a panic leaves at most its own store half-done,
        // as any failure of it would.
        if panic::catch_unwind(AssertUnwindSafe(work)).is_err() {
            log::error!("{what} panicked");
        }
    };
    match spawner {
        Some(spawner) => spawner.spawn(Box::pin(async move { guarded() })),
        None => {
            log::warn!("no spawner for {what}: running it on the calling thread");
            guarded();
        }
    }
}

#[cfg(test)]
pub(crate) mod probe {
    //! A job output for tests only, to exercise the job machinery without Garmin: applying a probe records its number on
    //! the applying thread, and two numbers stand for the ways applying can go wrong.

    use std::cell::RefCell;

    use serde_json::Value;

    use crate::EngineError;

    /// A probe with this number panics while it is applied: a bug in the code that applies an output.
    pub(crate) const PANICS: u32 = 666;
    /// A probe with this number fails to apply, as a write the database refuses.
    pub(crate) const FAILS: u32 = 500;

    thread_local! {
        static APPLIED: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn apply(n: u32) -> Result<Value, EngineError> {
        match n {
            PANICS => panic!("probe {n} panicked while it was applied"),
            FAILS => Err(EngineError::Store(format!("probe {n} failed to apply"))),
            n => {
                APPLIED.with(|applied| applied.borrow_mut().push(n));
                Ok(Value::from(n))
            }
        }
    }

    /// The probes applied on this thread since the last call, in the order they were applied.
    pub(crate) fn take_applied() -> Vec<u32> {
        APPLIED.with(|applied| std::mem::take(&mut *applied.borrow_mut()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{DeferredSpawner, DroppingSpawner, InlineSpawner, RecordingSleep};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Waker};

    fn counter() -> (Arc<AtomicUsize>, Arc<dyn Fn() + Send + Sync>) {
        let count = Arc::new(AtomicUsize::new(0));
        let counted = count.clone();
        let notify = Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        (count, notify)
    }

    fn no_sleep() -> Arc<dyn Fn(Duration) -> BoxFuture<()> + Send + Sync> {
        RecordingSleep::new().sleep_fn()
    }

    fn verified() -> JobOutput {
        JobOutput::VerifyChecked { result: Ok(()) }
    }

    /// What a probe job sent, in a form tests can compare.
    #[derive(Debug, PartialEq, Eq)]
    enum Seen {
        Partial(u32),
        Finished(Result<u32, EngineError>),
    }

    fn seen(message: JobMessage) -> Seen {
        match message {
            JobMessage::Partial(JobOutput::Probe(n)) => Seen::Partial(n),
            JobMessage::Finished(Ok(JobOutput::Probe(n))) => Seen::Finished(Ok(n)),
            JobMessage::Finished(Err(error)) => Seen::Finished(Err(error)),
            JobMessage::Partial(_) | JobMessage::Finished(Ok(_)) => panic!("a probe job sent something other than a probe"),
        }
    }

    /// Everything the jobs have sent so far, in order.
    fn take_all(jobs: &mut Jobs) -> Vec<(JobId, &'static str, Seen)> {
        std::iter::from_fn(|| jobs.take()).map(|(id, command, message)| (id, command, seen(message))).collect()
    }

    /// Polls every future once and drops it, finished or not: a runtime that shuts down while a job waits.
    struct PollOnceSpawner;

    impl Spawner for PollOnceSpawner {
        fn spawn(&self, mut fut: BoxFuture<()>) {
            let _ = fut.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        }
    }

    #[test]
    fn results_wait_until_taken() {
        let mut jobs = Jobs::new();
        let spawner = DeferredSpawner::new();
        let (notified, notify) = counter();
        let first = jobs.start("x.first", &spawner, notify.clone(), no_sleep(), |_| async { verified() });
        let second = jobs.start("x.second", &spawner, notify, no_sleep(), |_| async { verified() });
        assert_ne!(first, second);
        assert_eq!(jobs.running().collect::<Vec<_>>(), ["x.first", "x.second"]);
        assert!(jobs.take().is_none(), "nothing has run yet");

        spawner.run_pending();
        assert_eq!(notified.load(Ordering::SeqCst), 2);
        assert_eq!(jobs.running().count(), 2, "finished jobs run until their results are taken");
        let mut taken = Vec::new();
        while let Some((id, command, message)) = jobs.take() {
            assert!(matches!(message, JobMessage::Finished(Ok(JobOutput::VerifyChecked { result: Ok(()) }))));
            taken.push((id, command));
        }
        assert_eq!(taken, [(first, "x.first"), (second, "x.second")]);
        assert_eq!(jobs.running().count(), 0);
    }

    #[test]
    fn partials_arrive_in_order_before_finish() {
        let mut jobs = Jobs::new();
        let spawner = DeferredSpawner::new();
        let (notified, notify) = counter();
        let id = jobs.start("x.cmd", &spawner, notify, no_sleep(), |ctx| async move {
            for n in 1..=3 {
                ctx.send(JobOutput::Probe(n));
            }
            JobOutput::Probe(4)
        });
        spawner.run_pending();
        assert_eq!(notified.load(Ordering::SeqCst), 4, "every partial result wakes the frontend up, and so does the end");

        for n in 1..=3 {
            let Some((taken, command, message)) = jobs.take() else { panic!("partial result {n} did not come") };
            assert_eq!((taken, command, seen(message)), (id, "x.cmd", Seen::Partial(n)));
            assert_eq!(jobs.running().collect::<Vec<_>>(), ["x.cmd"], "a job runs until its end is taken");
        }
        let Some((taken, command, message)) = jobs.take() else { panic!("the end did not come") };
        assert_eq!((taken, command, seen(message)), (id, "x.cmd", Seen::Finished(Ok(4))));
        assert_eq!(jobs.running().count(), 0);
        assert!(jobs.take().is_none(), "a job that finished sends nothing more, not even that it was dropped");
        assert_eq!(notified.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn panicking_job_becomes_an_error() {
        let mut jobs = Jobs::new();
        let spawner = DeferredSpawner::new();
        let (notified, notify) = counter();
        fn boom() -> JobOutput {
            panic!("the job failed")
        }
        let id = jobs.start("x.boom", &spawner, notify, no_sleep(), |_| async { boom() });
        spawner.run_pending();
        let Some((taken, command, message)) = jobs.take() else { panic!("the panicked job sent nothing") };
        assert_eq!((taken, command), (id, "x.boom"));
        assert!(matches!(message, JobMessage::Finished(Err(EngineError::JobPanicked(ref text))) if text == "x.boom"));
        assert!(jobs.take().is_none(), "a panicked job is not reported as dropped as well");
        assert_eq!(notified.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panic_while_the_job_is_made_is_a_job_panic() {
        // The closure that makes the future runs inside the job too, so its panic is caught there and not in `start`.
        let mut jobs = Jobs::new();
        let spawner = DeferredSpawner::new();
        let (notified, notify) = counter();
        fn boom(_: JobCtx) -> std::future::Ready<JobOutput> {
            panic!("the job could not be made")
        }
        let id = jobs.start("x.boom", &spawner, notify, no_sleep(), boom);
        spawner.run_pending();
        assert_eq!(take_all(&mut jobs), [(id, "x.boom", Seen::Finished(Err(EngineError::JobPanicked("x.boom".to_owned()))))]);
        assert_eq!(notified.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropped_job_is_reported_once() {
        // The spawner drops the job without ever polling it, as a runtime that shuts down before the job got a thread.
        let mut jobs = Jobs::new();
        let (notified, notify) = counter();
        let made = Arc::new(AtomicUsize::new(0));
        let counted = made.clone();
        let id = jobs.start("x.cmd", &DroppingSpawner, notify, no_sleep(), move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            async { JobOutput::Probe(1) }
        });
        assert_eq!(made.load(Ordering::SeqCst), 0, "the job never started");
        assert_eq!(notified.load(Ordering::SeqCst), 1, "the frontend is woken up to learn that the job is gone");
        assert_eq!(take_all(&mut jobs), [(id, "x.cmd", Seen::Finished(Err(EngineError::JobDropped("x.cmd".to_owned()))))]);
        assert_eq!(jobs.running().count(), 0, "a dropped job no longer runs");
        assert_eq!(notified.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn job_dropped_while_it_waits_is_reported_after_its_partials() {
        let mut jobs = Jobs::new();
        let (notified, notify) = counter();
        let id = jobs.start("x.cmd", &PollOnceSpawner, notify, no_sleep(), |ctx| async move {
            ctx.send(JobOutput::Probe(1));
            std::future::pending::<()>().await;
            JobOutput::Probe(2)
        });
        assert_eq!(take_all(&mut jobs), [(id, "x.cmd", Seen::Partial(1)), (id, "x.cmd", Seen::Finished(Err(EngineError::JobDropped("x.cmd".to_owned()))))]);
        assert_eq!(jobs.running().count(), 0);
        assert_eq!(notified.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_job_sleeps_through_the_injected_sleep() {
        let mut jobs = Jobs::new();
        let (_, notify) = counter();
        let sleep = RecordingSleep::new();
        let id = jobs.start("x.wait", &InlineSpawner, notify, sleep.sleep_fn(), |ctx| async move {
            ctx.sleep(Duration::from_secs(2)).await;
            ctx.sleep(Duration::from_secs(30)).await;
            JobOutput::Probe(1)
        });
        assert_eq!(sleep.slept(), [Duration::from_secs(2), Duration::from_secs(30)]);
        assert_eq!(take_all(&mut jobs), [(id, "x.wait", Seen::Finished(Ok(1)))]);
    }

    #[test]
    fn tail_work_runs_later_on_the_spawner() {
        let deferred = Arc::new(DeferredSpawner::new());
        let spawner: Arc<dyn Spawner> = deferred.clone();
        let (ran, count) = counter();
        spawn_tail(Some(&spawner), "x.tail", move || count());
        assert_eq!(ran.load(Ordering::SeqCst), 0, "the caller does not wait for it");
        assert_eq!(deferred.pending(), 1);
        deferred.run_pending();
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panic_in_tail_work_stays_in_it() {
        // The inline spawner runs it inside `spawn_tail`: a panic that got out would unwind into the caller.
        let spawner: Arc<dyn Spawner> = Arc::new(InlineSpawner);
        spawn_tail(Some(&spawner), "x.tail", || panic!("the tail work failed"));
        spawn_tail(None, "x.tail", || panic!("the tail work failed"));
    }

    #[test]
    fn tail_work_without_a_spawner_runs_at_once() {
        // A session without a spawner cannot have started a job, so this does not happen; if it did, the work must
        // still be done.
        let (ran, count) = counter();
        spawn_tail(None, "x.tail", move || count());
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn job_outliving_its_session_is_dropped() {
        let mut jobs = Jobs::new();
        let (late, dropped) = (DeferredSpawner::new(), DeferredSpawner::new());
        let (notified, notify) = counter();
        jobs.start("x.late", &late, notify.clone(), no_sleep(), |ctx| async move {
            ctx.send(JobOutput::Probe(1));
            verified()
        });
        jobs.start("x.dropped", &dropped, notify, no_sleep(), |_| async { verified() });
        drop(jobs);
        late.run_pending();
        dropped.drop_pending();
        assert_eq!(notified.load(Ordering::SeqCst), 0, "nobody is left to wake up");
    }
}
