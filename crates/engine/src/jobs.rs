//! Background jobs: work a [`CommandKind::Job`](crate::CommandKind::Job) command starts on the injected
//! [`Spawner`], and the channel its result comes back through.
//!
//! A job only does the slow part (a Garmin request) and returns a [`JobOutput`]; it never touches the database or
//! the secret stores' contents. [`Session::poll_jobs`](crate::Session::poll_jobs) applies the output on the
//! session's thread (the UI thread in the app), so every write happens there. A job counts as running from its
//! start until `poll_jobs` has taken its result, even if the future finished long before. A panic inside the job
//! becomes [`EngineError::JobPanicked`] instead of taking the app down.

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use futures::FutureExt;
use gymgar_domain::{KeyTerm, SecretBackend};
use secrecy::SecretString;
use serde_json::Value;

use crate::{EngineError, GarminAuthError, JobId, Session, Spawner, garmin_cmds};

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

/// What a job's future hands back to the session: the outcome of the slow part only. Applying it (saving the key,
/// recording the status) is left to [`JobOutput::apply`] on the session's thread.
pub(crate) enum JobOutput {
    /// `garmin.connect` asked Garmin about `key`. The key is saved to `backend`, chosen before the request, only if
    /// Garmin accepted it.
    ConnectChecked { key: SecretString, term: KeyTerm, backend: SecretBackend, result: Result<(), GarminAuthError> },
    /// `garmin.verify` read the recorded key and asked Garmin about it. `Err(KeyMissing)` or `Err(Secrets(..))`
    /// when there was no key to ask about.
    VerifyChecked { result: Result<(), EngineError> },
}

impl JobOutput {
    /// Applies the output to the session and returns the command's answer.
    pub(crate) fn apply(self, session: &Session) -> Result<Value, EngineError> {
        match self {
            JobOutput::ConnectChecked { key, term, backend, result } => garmin_cmds::apply_connect(session, &key, term, backend, result),
            JobOutput::VerifyChecked { result } => garmin_cmds::apply_verify(session, result),
        }
    }
}

/// A job's output (or its panic), on its way from the job to the session.
struct Delivery {
    id: JobId,
    output: Result<JobOutput, EngineError>,
}

/// The session's jobs: those still running and the channel their results arrive in.
pub(crate) struct Jobs {
    next_id: u64,
    /// Started jobs whose result has not been taken yet, with the id of the command that started each.
    running: BTreeMap<JobId, &'static str>,
    sender: Sender<Delivery>,
    receiver: Receiver<Delivery>,
}

impl Jobs {
    pub(crate) fn new() -> Jobs {
        let (sender, receiver) = mpsc::channel();
        Jobs { next_id: 1, running: BTreeMap::new(), sender, receiver }
    }

    /// Starts `fut` on `spawner` as a job of `command`. When it finishes (or panics), its output is sent to the
    /// session and `notify` wakes the frontend up to call `poll_jobs`.
    pub(crate) fn start<F>(&mut self, command: &'static str, spawner: &dyn Spawner, notify: Arc<dyn Fn() + Send + Sync>, fut: F) -> JobId
    where
        F: Future<Output = JobOutput> + Send + 'static,
    {
        let id = JobId(self.next_id);
        // 2^64 jobs never happen in one session; saturating keeps the counter total all the same.
        self.next_id = self.next_id.saturating_add(1);
        // Marked running before it is spawned: an inline spawner finishes the job inside `spawn`.
        self.running.insert(id, command);
        let sender = self.sender.clone();
        spawner.spawn(Box::pin(async move {
            // The panic message is not kept: the job may hold a secret, and the message is whatever text the
            // panicking code chose. The panic hook has already reported it where the app's panics go.
            let output = AssertUnwindSafe(fut).catch_unwind().await.map_err(|_| EngineError::JobPanicked(command.to_owned()));
            if sender.send(Delivery { id, output }).is_err() {
                // The session is gone (the app is closing): nobody is left to apply the result.
                log::debug!("job {} of {command} finished after its session closed", id.0);
                return;
            }
            notify();
        }));
        id
    }

    /// The commands of the jobs that are running.
    pub(crate) fn running(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.running.values().copied()
    }

    /// The next job that has finished, no longer counted as running, with the command that started it.
    pub(crate) fn take_finished(&mut self) -> Option<(JobId, &'static str, Result<JobOutput, EngineError>)> {
        loop {
            let Delivery { id, output } = self.receiver.try_recv().ok()?;
            match self.running.remove(&id) {
                Some(command) => return Some((id, command, output)),
                // Only `start` sends, and only once per job, so this cannot happen; skipping it keeps the queue moving.
                None => log::warn!("ignoring the result of job {}, which is not running", id.0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::DeferredSpawner;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn counter() -> (Arc<AtomicUsize>, Arc<dyn Fn() + Send + Sync>) {
        let count = Arc::new(AtomicUsize::new(0));
        let counted = count.clone();
        let notify = Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        (count, notify)
    }

    fn verified() -> JobOutput {
        JobOutput::VerifyChecked { result: Ok(()) }
    }

    #[test]
    fn results_wait_until_taken() {
        let mut jobs = Jobs::new();
        let spawner = DeferredSpawner::new();
        let (notified, notify) = counter();
        let first = jobs.start("x.first", &spawner, notify.clone(), async { verified() });
        let second = jobs.start("x.second", &spawner, notify, async { verified() });
        assert_ne!(first, second);
        assert_eq!(jobs.running().collect::<Vec<_>>(), ["x.first", "x.second"]);
        assert!(jobs.take_finished().is_none(), "nothing has run yet");

        spawner.run_pending();
        assert_eq!(notified.load(Ordering::SeqCst), 2);
        assert_eq!(jobs.running().count(), 2, "finished jobs run until their results are taken");
        let mut taken = Vec::new();
        while let Some((id, command, output)) = jobs.take_finished() {
            assert!(matches!(output, Ok(JobOutput::VerifyChecked { result: Ok(()) })));
            taken.push((id, command));
        }
        assert_eq!(taken, [(first, "x.first"), (second, "x.second")]);
        assert_eq!(jobs.running().count(), 0);
    }

    #[test]
    fn panicking_job_becomes_an_error() {
        let mut jobs = Jobs::new();
        let spawner = DeferredSpawner::new();
        let (notified, notify) = counter();
        fn boom() -> JobOutput {
            panic!("the job failed")
        }
        let id = jobs.start("x.boom", &spawner, notify, async { boom() });
        spawner.run_pending();
        let Some((taken, command, output)) = jobs.take_finished() else { panic!("the panicked job sent nothing") };
        assert_eq!((taken, command), (id, "x.boom"));
        assert!(matches!(output, Err(EngineError::JobPanicked(ref text)) if text == "x.boom"));
        assert_eq!(notified.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn job_outliving_its_session_is_dropped() {
        let mut jobs = Jobs::new();
        let spawner = DeferredSpawner::new();
        let (notified, notify) = counter();
        jobs.start("x.late", &spawner, notify, async { verified() });
        drop(jobs);
        spawner.run_pending();
        assert_eq!(notified.load(Ordering::SeqCst), 0, "nobody is left to wake up");
    }
}
