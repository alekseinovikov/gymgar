//! The platform side of the engine's services: adapters from the engine's traits to the system keychain, the secret file,
//! the Garmin client and the tokio runtime, and [`build`], which puts them together for the desktop app.
//!
//! The engine knows none of these crates, and the adapters know no policy: which store a key goes to, and whether the
//! user has to agree first, is the engine's rule ([`SecretStores::new_key_backend`]); this module only decides which
//! stores exist. A failing store is reported as an error and never papered over with the other one, so a locked
//! keychain cannot move a key to a file behind the user's back.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use gymgar_domain::SecretBackend;
use gymgar_engine::{BoxFuture, GarminAuth, GarminAuthError, SecretStore, SecretStoreError, SecretStores, Services, Spawner};
use gymgar_garmin::GarminError;
use gymgar_garmin::personal::Auth;
use gymgar_secrets::{FileStore, GARMIN_ACCOUNT, SERVICE, SecretsError, SystemStore};
use secrecy::SecretString;

/// The directory of the file store inside the data directory.
const SECRETS_DIR: &str = "secrets";

fn store_error(error: SecretsError) -> SecretStoreError {
    // `SecretsError` texts describe the failure and never contain a secret.
    SecretStoreError(error.to_string())
}

/// The key in the system secret store (Keychain, Credential Manager, Secret Service).
pub struct SystemSecretStore(SystemStore);

impl SystemSecretStore {
    /// The entry Gymgar keeps its Garmin key in. Nothing is touched until `get`, `set` or `delete`.
    pub fn new() -> SystemSecretStore {
        SystemSecretStore(SystemStore::new(SERVICE, GARMIN_ACCOUNT))
    }
}

impl SecretStore for SystemSecretStore {
    fn backend(&self) -> SecretBackend {
        SecretBackend::System
    }

    fn get(&self) -> Result<Option<SecretString>, SecretStoreError> {
        self.0.get().map_err(store_error)
    }

    fn set(&self, key: &SecretString) -> Result<(), SecretStoreError> {
        self.0.set(key).map_err(store_error)
    }

    fn delete(&self) -> Result<(), SecretStoreError> {
        self.0.delete().map_err(store_error)
    }
}

/// The key in a file (`0600` on Unix) in the data directory.
pub struct FileSecretStore {
    store: FileStore,
    path: PathBuf,
}

impl FileSecretStore {
    pub fn new(path: PathBuf) -> FileSecretStore {
        FileSecretStore { store: FileStore::new(path.clone()), path }
    }
}

impl SecretStore for FileSecretStore {
    fn backend(&self) -> SecretBackend {
        SecretBackend::File
    }

    fn get(&self) -> Result<Option<SecretString>, SecretStoreError> {
        self.store.get().map_err(store_error)
    }

    fn set(&self, key: &SecretString) -> Result<(), SecretStoreError> {
        self.store.set(key).map_err(store_error)
    }

    fn delete(&self) -> Result<(), SecretStoreError> {
        self.store.delete().map_err(store_error)
    }

    /// The file's path, so the consent warning can name it (spec section 7).
    fn location(&self) -> Option<String> {
        Some(self.path.display().to_string())
    }
}

/// Which secret stores this run has. A debug build has no system store at all and prefers the file: it never touches or
/// even probes the OS keychain, so developing does not make macOS ask for access after every rebuild, and never reaches
/// the key of an installed release. A release build asks `probe_system_store` once; the system store exists only if that
/// says it works here.
fn secret_stores(data_dir: &Path, debug_build: bool, probe_system_store: impl FnOnce() -> bool) -> SecretStores {
    let file: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(data_dir.join(SECRETS_DIR).join(GARMIN_ACCOUNT)));
    if debug_build {
        return SecretStores { system: None, file, prefer_file: true };
    }
    let system = probe_system_store().then(|| Arc::new(SystemSecretStore::new()) as Arc<dyn SecretStore>);
    SecretStores { system, file, prefer_file: false }
}

/// What the user is told about a key Garmin did not accept, from what the client found out.
fn auth_error(error: &GarminError) -> GarminAuthError {
    match error {
        // A key with a character no real key has is as wrong as one Garmin turned down, and nothing was sent.
        GarminError::InvalidKey | GarminError::KeyRejected => GarminAuthError::KeyRejected,
        GarminError::RateLimited => GarminAuthError::RateLimited,
        GarminError::Unavailable { .. } => GarminAuthError::Unavailable,
        GarminError::Network(_) => GarminAuthError::Network,
        GarminError::UnexpectedResponse(_) => GarminAuthError::Unexpected,
    }
}

/// Checks keys with Garmin's Personal API: one exchange of the key for a token.
pub struct GarminPersonalAuth;

impl GarminAuth for GarminPersonalAuth {
    fn check_key(&self, key: SecretString) -> BoxFuture<Result<(), GarminAuthError>> {
        // The client is built inside the future, so building it (TLS setup) is part of the background job too.
        Box::pin(async move { check(Auth::new(key)).await })
    }
}

/// Asks Garmin about the key `auth` was made from.
async fn check(auth: Result<Auth, GarminError>) -> Result<(), GarminAuthError> {
    let outcome = match auth {
        Ok(auth) => auth.exchange().await.map(|_| ()),
        Err(error) => Err(error),
    };
    outcome.map_err(|error| {
        // The engine's error says only what kind of failure it was; the details go to the log. Nothing in a `GarminError`
        // is the key or a token.
        log::info!("the Garmin key check failed: {error}");
        auth_error(&error)
    })
}

/// Runs the engine's background jobs on the tokio runtime.
pub struct TokioSpawner(pub tokio::runtime::Handle);

impl Spawner for TokioSpawner {
    /// Every job gets a thread of the blocking pool and is driven there. A job may block (a secret store read waits while
    /// the keychain asks the user for access), and on a worker thread that would stall every other job and the runtime's
    /// I/O with it. Timers and sockets still run on the runtime's own threads.
    fn spawn(&self, fut: BoxFuture<()>) {
        let handle = self.0.clone();
        // The job's result travels through the engine's channel, so the join handle has nothing left to report. A panic
        // never gets here: the engine catches it around the job.
        self.0.spawn_blocking(move || handle.block_on(fut));
    }
}

/// The services of the desktop app. `notify` wakes the window when a job has finished.
pub fn build(data_dir: &Path, ctx: egui::Context, rt: &tokio::runtime::Runtime) -> Services {
    let secrets = secret_stores(data_dir, cfg!(debug_assertions), SystemStore::is_available);
    log::info!(
        "secret stores: system store {}, new keys go to {}",
        if secrets.system.is_some() { "available" } else { "not used" },
        secrets.new_key_backend().as_str()
    );
    Services {
        secrets: Some(secrets),
        garmin: Some(Arc::new(GarminPersonalAuth)),
        spawner: Some(Arc::new(TokioSpawner(rt.handle().clone()))),
        notify: Arc::new(move || ctx.request_repaint()),
        clock: Arc::new(Utc::now),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError, mpsc};
    use std::thread::{self, ThreadId};
    use std::time::{Duration, Instant};

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use chrono::Utc;
    use gymgar_domain::{ConnectionStatus, SecretBackend};
    use gymgar_engine::{Caller, EngineError, GarminStatusView, JobFinished, JobId, Outcome, Session};
    use gymgar_store::Store;
    use secrecy::{ExposeSecret, SecretString};
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tokio::sync::Notify;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::logging::testing::capture;

    const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
    const KEY: &str = "not-a-real-key";

    fn secret(text: &str) -> SecretString {
        SecretString::from(text.to_owned())
    }

    fn runtime(workers: usize) -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().worker_threads(workers).enable_all().build().expect("a test runtime")
    }

    #[test]
    fn garmin_error_mapping() {
        let cases = [
            (GarminError::InvalidKey, GarminAuthError::KeyRejected),
            (GarminError::KeyRejected, GarminAuthError::KeyRejected),
            (GarminError::RateLimited, GarminAuthError::RateLimited),
            (GarminError::Unavailable { status: 503 }, GarminAuthError::Unavailable),
            (GarminError::Network("connection refused".to_owned()), GarminAuthError::Network),
            (GarminError::UnexpectedResponse("HTTP 418".to_owned()), GarminAuthError::Unexpected),
        ];
        for (error, expected) in cases {
            assert_eq!(auth_error(&error), expected, "{error:?}");
        }
    }

    #[test]
    fn file_adapter_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("secrets").join("garmin-personal-api-key");
        let store = FileSecretStore::new(path.clone());
        assert_eq!(store.backend(), SecretBackend::File);
        assert_eq!(store.location(), Some(path.display().to_string()));
        assert!(store.get().unwrap().is_none());

        // A real key is 132 characters; it is stored whole.
        let long = format!("{KEY}-{}", "k".repeat(132));
        store.set(&secret(&long)).unwrap();
        assert_eq!(store.get().unwrap().unwrap().expose_secret(), long);
        assert!(path.is_file(), "the key is in the file the adapter reports");
        store.set(&secret(KEY)).unwrap();
        assert_eq!(store.get().unwrap().unwrap().expose_secret(), KEY, "a new key replaces the old one");

        store.delete().unwrap();
        assert!(store.get().unwrap().is_none());
        assert!(!path.exists());
        store.delete().unwrap();
    }

    #[test]
    fn file_adapter_failures_name_the_problem_and_never_the_key() {
        let dir = TempDir::new().unwrap();
        // The directory the secret goes to is a file: nothing can be created there.
        let blocker = dir.path().join("secrets");
        std::fs::write(&blocker, "not a directory").unwrap();
        let store = FileSecretStore::new(blocker.join("garmin-personal-api-key"));
        let error = store.set(&secret(KEY)).unwrap_err();
        assert!(error.0.contains("secrets"), "the text says where it failed: {error}");
        assert!(!error.0.contains(KEY), "{error}");
    }

    #[test]
    fn system_adapter_reports_its_backend_and_no_file() {
        // Only a handle is built here: the system store is touched by `get`, `set` and `delete`, never by this.
        let store = SystemSecretStore::new();
        assert_eq!(store.backend(), SecretBackend::System);
        assert_eq!(store.location(), None);
    }

    #[test]
    fn a_debug_build_never_probes_the_system_store() {
        let probed = AtomicUsize::new(0);
        let dir = TempDir::new().unwrap();
        let stores = secret_stores(dir.path(), true, || {
            probed.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert_eq!(probed.load(Ordering::SeqCst), 0, "a development build must not touch the keychain, not even to ask");
        assert!(stores.system.is_none());
        assert!(stores.prefer_file);
        assert_eq!(stores.new_key_backend(), SecretBackend::File);
        assert!(!stores.needs_file_consent());
    }

    #[test]
    fn a_release_build_uses_the_system_store_when_it_answers_the_probe() {
        let dir = TempDir::new().unwrap();
        let probed = AtomicUsize::new(0);
        let stores = secret_stores(dir.path(), false, || {
            probed.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert_eq!(probed.load(Ordering::SeqCst), 1, "asked once");
        assert_eq!(stores.system.as_ref().map(|store| store.backend()), Some(SecretBackend::System));
        assert!(!stores.prefer_file);
        assert_eq!(stores.new_key_backend(), SecretBackend::System);
        assert!(!stores.needs_file_consent());
    }

    #[test]
    fn a_release_build_asks_before_a_file_when_the_system_store_does_not_answer() {
        let dir = TempDir::new().unwrap();
        let stores = secret_stores(dir.path(), false, || false);
        assert!(stores.system.is_none());
        assert!(!stores.prefer_file);
        assert_eq!(stores.new_key_backend(), SecretBackend::File);
        assert!(stores.needs_file_consent(), "the user has to agree before the key goes to a file");
    }

    #[test]
    fn the_file_store_lives_under_the_data_directory() {
        let dir = TempDir::new().unwrap();
        let stores = secret_stores(dir.path(), true, || false);
        let expected = dir.path().join("secrets").join("garmin-personal-api-key");
        assert_eq!(stores.file.location(), Some(expected.display().to_string()));
        assert_eq!(stores.file.backend(), SecretBackend::File);
    }

    /// `header.payload.signature` with only `exp` in the payload, as Garmin's token looks to the client.
    fn fake_token() -> String {
        let exp = (Utc::now() + chrono::TimeDelta::hours(24)).timestamp();
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("{header}.{payload}.c2lnbmF0dXJl")
    }

    /// Runs the adapter's check of `key` against a local server that answers the token request with `answer`; returns the
    /// result and how many requests the server got.
    fn check_against(answer: ResponseTemplate, key: &str) -> (Result<(), GarminAuthError>, usize) {
        let rt = runtime(1);
        rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST")).and(path(TOKEN_PATH)).respond_with(answer).mount(&server).await;
            let result = check(Auth::with_base_url(secret(key), &server.uri())).await;
            let requests = server.received_requests().await.map_or(0, |requests| requests.len());
            (result, requests)
        })
    }

    #[test]
    fn a_token_from_garmin_means_the_key_is_good() {
        let (result, requests) = check_against(ResponseTemplate::new(200).set_body_string(fake_token()), KEY);
        assert_eq!((result, requests), (Ok(()), 1));
    }

    #[test]
    fn garmin_answers_become_the_errors_the_user_is_told_about() {
        let cases = [
            (ResponseTemplate::new(401), GarminAuthError::KeyRejected),
            (ResponseTemplate::new(429), GarminAuthError::RateLimited),
            (ResponseTemplate::new(503), GarminAuthError::Unavailable),
            (ResponseTemplate::new(200).set_body_string("this is not a token"), GarminAuthError::Unexpected),
            (ResponseTemplate::new(418), GarminAuthError::Unexpected),
        ];
        for (answer, expected) in cases {
            let (result, requests) = check_against(answer, KEY);
            assert_eq!((result, requests), (Err(expected), 1));
        }
    }

    #[test]
    fn a_key_garmin_could_never_accept_is_rejected_without_a_request() {
        let (result, requests) = check_against(ResponseTemplate::new(200).set_body_string(fake_token()), "ключ с пробелами");
        assert_eq!((result, requests), (Err(GarminAuthError::KeyRejected), 0));
    }

    #[test]
    fn an_unreachable_garmin_is_a_network_error() {
        // A port nobody listens on: bind to get a free one, then close it.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let rt = runtime(1);
        let result = rt.block_on(check(Auth::with_base_url(secret(KEY), &format!("http://127.0.0.1:{port}"))));
        assert_eq!(result, Err(GarminAuthError::Network));
    }

    #[test]
    fn the_log_says_why_a_check_failed_and_never_shows_the_key() {
        let logged = capture();
        let token = fake_token();
        // Every way a check can end; the key of each is the one that must stay out of the log.
        let (accepted, _) = check_against(ResponseTemplate::new(200).set_body_string(token.clone()), KEY);
        assert_eq!(accepted, Ok(()));
        for answer in
            [ResponseTemplate::new(401), ResponseTemplate::new(429), ResponseTemplate::new(503), ResponseTemplate::new(200).set_body_string("not a token")]
        {
            let _ = check_against(answer, KEY);
        }
        let _ = check_against(ResponseTemplate::new(200), "ключ-с-пробелами и кириллицей");
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let refused = runtime(1).block_on(check(Auth::with_base_url(secret(KEY), &format!("http://127.0.0.1:{port}"))));
        assert_eq!(refused, Err(GarminAuthError::Network));

        let log = logged();
        assert!(log.contains("the Garmin key check failed: Garmin rejected the API key"), "{log}");
        assert!(log.contains("the Garmin key check failed: Garmin rate limit reached"), "{log}");
        assert!(log.contains("the Garmin key check failed: Garmin is unavailable (HTTP 503)"), "{log}");
        assert!(log.contains("the Garmin key check failed: network error"), "{log}");
        assert!(!log.contains(KEY), "the key is in the log");
        assert!(!log.contains("ключ-с"), "the key is in the log");
        assert!(!log.contains(&token) && !log.contains(token.split('.').nth(1).unwrap()), "the token is in the log");
    }

    #[test]
    fn a_job_that_blocks_its_thread_does_not_stop_the_others() {
        // One worker, as bad as it gets: a job that waits for the keychain (a blocking call) must not keep this worker from
        // running the next job. The first job waits for the second one's signal; if the second never gets to run, it
        // times out.
        let rt = runtime(1);
        let spawner = TokioSpawner(rt.handle().clone());
        let (release, released) = mpsc::channel::<()>();
        let (blocked_result, blocked_done) = mpsc::channel();
        spawner.spawn(Box::pin(async move {
            // A blocking wait in the future, like a secret store read.
            let got_signal = released.recv_timeout(Duration::from_secs(3)).is_ok();
            let _ = blocked_result.send(got_signal);
        }));
        let (second_result, second_done) = mpsc::channel();
        spawner.spawn(Box::pin(async move {
            let _ = release.send(());
            let _ = second_result.send(());
        }));
        assert_eq!(second_done.recv_timeout(Duration::from_secs(5)), Ok(()), "the second job ran while the first one was blocked");
        assert_eq!(blocked_done.recv_timeout(Duration::from_secs(5)), Ok(true), "the first job saw the signal of the second");
    }

    /// A Garmin that answers `result` only after the test opens its gate, and tells the test which thread its check started
    /// on and which key it was asked about.
    struct GatedGarmin {
        result: Result<(), GarminAuthError>,
        gate: Arc<Notify>,
        started: mpsc::Sender<ThreadId>,
        asked_with: Mutex<Option<String>>,
    }

    impl GarminAuth for GatedGarmin {
        fn check_key(&self, key: SecretString) -> BoxFuture<Result<(), GarminAuthError>> {
            let _ = self.started.send(thread::current().id());
            *self.asked_with.lock().unwrap_or_else(PoisonError::into_inner) = Some(key.expose_secret().to_owned());
            let (gate, result) = (self.gate.clone(), self.result);
            Box::pin(async move {
                gate.notified().await;
                result
            })
        }
    }

    /// What a `garmin.connect` left behind after it went through the real runtime, spawner and session.
    struct Connected {
        job: JobId,
        finished: Vec<JobFinished>,
        status: GarminStatusView,
        asked_with: Option<String>,
        key_file: PathBuf,
        key_in_file: Option<String>,
        /// Keeps the data directory until the test is done with the files in it.
        _dir: TempDir,
    }

    /// Connects `KEY` the way the window does, with everything real except Garmin: the 2-worker runtime of the app,
    /// `TokioSpawner`, a `Session` on a file database, and the file secret store. Garmin answers `result`, but only when
    /// the test lets it. Asserts on the way what the window depends on: the command returns at once, the check runs on a
    /// runtime thread, `notify` fires exactly once when it ends, and `poll_jobs` hands out its result exactly once.
    fn connect_through_the_runtime(result: Result<(), GarminAuthError>) -> Connected {
        let dir = TempDir::new().unwrap();
        let rt = crate::build_runtime().expect("the app's runtime");
        let key_file = dir.path().join("secrets").join("garmin-personal-api-key");
        let file = Arc::new(FileSecretStore::new(key_file.clone()));
        let gate = Arc::new(Notify::new());
        let (started_tx, started_rx) = mpsc::channel();
        let garmin = Arc::new(GatedGarmin { result, gate: gate.clone(), started: started_tx, asked_with: Mutex::new(None) });
        let (notified_tx, notified_rx) = mpsc::channel();
        let services = Services {
            secrets: Some(SecretStores { system: None, file: file.clone(), prefer_file: true }),
            garmin: Some(garmin.clone()),
            spawner: Some(Arc::new(TokioSpawner(rt.handle().clone()))),
            notify: Arc::new(move || {
                let _ = notified_tx.send(());
            }),
            clock: Arc::new(Utc::now),
        };
        let mut session = Session::new(Store::open(&dir.path().join("gymgar.sqlite3")).unwrap(), services);

        // If the command wrongly waited for Garmin, this would let it through after 5 seconds, and the checks below, which
        // need the check to be still under way, would fail instead of the test hanging.
        let watchdog = gate.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(5));
            watchdog.notify_one();
        });

        let started = session.execute("garmin.connect", json!({ "apiKey": KEY, "keyTerm": 365 }), Caller::User);
        let Ok(Outcome::Started(job)) = started else { panic!("garmin.connect did not start a job: {started:?}") };

        // The command has answered, and Garmin has not: the check is under way on a runtime thread, held at the gate.
        let check_thread = started_rx.recv_timeout(Duration::from_secs(5)).expect("the key check started on the runtime");
        assert_ne!(check_thread, thread::current().id(), "the check must not run on the session's thread");
        assert!(notified_rx.try_recv().is_err(), "notify fired before Garmin answered: the command waited for it");
        assert!(session.poll_jobs().is_empty(), "a result arrived before Garmin answered");
        let running = read_status(&mut session);
        assert!(running.check_running, "the session does not know a check is running");
        assert_eq!(running.status, ConnectionStatus::NotConnected);

        // Garmin answers: the runtime finishes the job and wakes the window, once.
        gate.notify_one();
        notified_rx.recv_timeout(Duration::from_secs(5)).expect("notify woke the window");
        let finished = session.poll_jobs();
        assert!(notified_rx.try_recv().is_err(), "notify fired more than once for one job");
        assert!(session.poll_jobs().is_empty(), "a result must be handed out once");

        let status = read_status(&mut session);
        assert!(!status.check_running, "the check still counts as running after its result was applied");
        let asked_with = garmin.asked_with.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let key_in_file = file.get().unwrap().map(|key| key.expose_secret().to_owned());
        Connected { job, finished, status, asked_with, key_file, key_in_file, _dir: dir }
    }

    fn read_status(session: &mut Session) -> GarminStatusView {
        match session.execute("garmin.status", Value::Null, Caller::User) {
            Ok(Outcome::Done(value)) => serde_json::from_value(value).unwrap(),
            other => panic!("garmin.status: {other:?}"),
        }
    }

    #[test]
    fn a_key_garmin_accepts_is_connected_through_the_runtime() {
        let run = connect_through_the_runtime(Ok(()));
        let [finished] = run.finished.as_slice() else { panic!("expected one finished job, got {:?}", run.finished) };
        assert_eq!((finished.id, finished.command), (run.job, "garmin.connect"));
        let Ok(answer) = &finished.result else { panic!("the connect failed: {:?}", finished.result) };
        assert_eq!(answer.get("status"), Some(&json!("connected")), "{answer}");
        assert!(!answer.to_string().contains(KEY), "the answer carries the key: {answer}");

        assert_eq!(run.status.status, ConnectionStatus::Connected);
        assert_eq!(run.status.secret_backend, Some(SecretBackend::File));
        assert!(run.status.key_expires_at.is_some());
        assert_eq!(run.asked_with.as_deref(), Some(KEY), "Garmin was asked about the key that was typed");
        assert_eq!(run.key_in_file.as_deref(), Some(KEY), "the key is in the file store");
        assert_eq!(std::fs::read_to_string(&run.key_file).unwrap(), KEY, "and in the file the adapter reports");
    }

    #[test]
    fn a_key_garmin_rejects_saves_nothing_through_the_runtime() {
        let run = connect_through_the_runtime(Err(GarminAuthError::KeyRejected));
        let [finished] = run.finished.as_slice() else { panic!("expected one finished job, got {:?}", run.finished) };
        assert_eq!((finished.id, finished.command), (run.job, "garmin.connect"));
        assert_eq!(finished.result, Err(EngineError::Garmin(GarminAuthError::KeyRejected)));

        assert_eq!(run.status.status, ConnectionStatus::NotConnected);
        assert_eq!(run.status.secret_backend, None);
        assert_eq!(run.asked_with.as_deref(), Some(KEY), "Garmin was asked, and said no");
        assert_eq!(run.key_in_file, None, "a rejected key is not stored");
        assert!(!run.key_file.exists(), "no key file was created");
    }

    #[test]
    fn a_job_can_use_the_runtime_timer() {
        let rt = runtime(1);
        let spawner = TokioSpawner(rt.handle().clone());
        let (done, finished) = mpsc::channel();
        let started = Instant::now();
        spawner.spawn(Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = done.send(started.elapsed());
        }));
        let elapsed = finished.recv_timeout(Duration::from_secs(5)).expect("the job finished");
        assert!(elapsed >= Duration::from_millis(50), "{elapsed:?}");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn build_wires_every_service_without_touching_the_keychain() {
        let dir = TempDir::new().unwrap();
        let rt = runtime(1);
        let ctx = egui::Context::default();
        let repaints = Arc::new(AtomicUsize::new(0));
        let counted = repaints.clone();
        ctx.set_request_repaint_callback(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        });

        let services = build(dir.path(), ctx, &rt);
        let secrets = services.secrets.as_ref().expect("secret stores");
        assert!(secrets.system.is_none() && secrets.prefer_file, "a debug build uses files only");
        assert!(services.garmin.is_some());
        assert!(services.spawner.is_some());
        let drift = (services.clock)().signed_duration_since(Utc::now()).num_seconds().abs();
        assert!(drift <= 1, "the clock is the real one, {drift}s off");

        let before = repaints.load(Ordering::SeqCst);
        (services.notify)();
        assert!(repaints.load(Ordering::SeqCst) > before, "notify wakes the window");

        // A job started through the services runs on the runtime.
        let (done, finished) = mpsc::channel();
        services.spawner.as_ref().expect("a spawner").spawn(Box::pin(async move {
            let _ = done.send(());
        }));
        assert_eq!(finished.recv_timeout(Duration::from_secs(5)), Ok(()));
    }
}
