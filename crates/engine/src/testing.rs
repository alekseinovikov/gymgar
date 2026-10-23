//! Test doubles for the engine's services and a ready-made test session, for the tests of this crate and of the
//! crates built on it (UI, MCP).
//!
//! Compiled only for this crate's own tests and under the `testing` feature, which apps never enable. Nothing here
//! touches the network or the real keychain.

// Test-support code, compiled only under `cfg(test)` or the `testing` feature: the fakes panic on purpose
// (`FakeGarminAuth::panicking`) and fail loudly on misuse instead of returning errors.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, TimeZone, Utc};
use gymgar_domain::{ConnectionStatus, GarminConnection, KeyTerm, SecretBackend, key_expires_at};
use gymgar_store::Store;
use secrecy::{ExposeSecret, SecretString};

use crate::{BoxFuture, GarminAuth, GarminAuthError, SecretStore, SecretStoreError, SecretStores, Services, Session, Spawner};

/// A secret store in memory. `set_failing(true)` makes every operation fail, to test what a command leaves behind.
#[derive(Debug)]
pub struct MemorySecretStore {
    backend: SecretBackend,
    key: Mutex<Option<SecretString>>,
    failing: Mutex<bool>,
    location: Option<String>,
}

impl MemorySecretStore {
    pub fn new(backend: SecretBackend) -> MemorySecretStore {
        MemorySecretStore { backend, key: Mutex::new(None), failing: Mutex::new(false), location: None }
    }

    /// The same store, reporting `location` as the place its key lives (like a file store reports its path).
    pub fn with_location(mut self, location: impl Into<String>) -> MemorySecretStore {
        self.location = Some(location.into());
        self
    }

    /// Makes every following `get`, `set` and `delete` fail (or work again).
    pub fn set_failing(&self, failing: bool) {
        *self.failing.lock().unwrap_or_else(PoisonError::into_inner) = failing;
    }

    fn check(&self) -> Result<(), SecretStoreError> {
        if *self.failing.lock().unwrap_or_else(PoisonError::into_inner) {
            return Err(SecretStoreError(format!("the {} test store is set to fail", self.backend.as_str())));
        }
        Ok(())
    }
}

impl SecretStore for MemorySecretStore {
    fn backend(&self) -> SecretBackend {
        self.backend
    }

    fn get(&self) -> Result<Option<SecretString>, SecretStoreError> {
        self.check()?;
        Ok(self.key.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|key| SecretString::from(key.expose_secret())))
    }

    fn set(&self, key: &SecretString) -> Result<(), SecretStoreError> {
        self.check()?;
        *self.key.lock().unwrap_or_else(PoisonError::into_inner) = Some(SecretString::from(key.expose_secret()));
        Ok(())
    }

    fn delete(&self) -> Result<(), SecretStoreError> {
        self.check()?;
        *self.key.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(())
    }

    fn location(&self) -> Option<String> {
        self.location.clone()
    }
}

#[derive(Debug, Clone, Copy)]
enum FakeBehavior {
    Return(Result<(), GarminAuthError>),
    Panic,
}

/// A Garmin that answers every key check the same way, and remembers the keys it was given.
#[derive(Debug)]
pub struct FakeGarminAuth {
    behavior: FakeBehavior,
    last_key: Mutex<Option<String>>,
    calls: AtomicUsize,
}

impl FakeGarminAuth {
    pub fn returning(result: Result<(), GarminAuthError>) -> FakeGarminAuth {
        FakeGarminAuth::with(FakeBehavior::Return(result))
    }

    /// A Garmin whose check panics inside the future, to test that a panicking job does not take the app down.
    pub fn panicking() -> FakeGarminAuth {
        FakeGarminAuth::with(FakeBehavior::Panic)
    }

    fn with(behavior: FakeBehavior) -> FakeGarminAuth {
        FakeGarminAuth { behavior, last_key: Mutex::new(None), calls: AtomicUsize::new(0) }
    }

    /// The key of the last `check_key` call, `None` when it was never called.
    pub fn last_key(&self) -> Option<String> {
        self.last_key.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// How many times `check_key` was called.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl GarminAuth for FakeGarminAuth {
    fn check_key(&self, key: SecretString) -> BoxFuture<Result<(), GarminAuthError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_key.lock().unwrap_or_else(PoisonError::into_inner) = Some(key.expose_secret().to_owned());
        let behavior = self.behavior;
        Box::pin(async move {
            match behavior {
                FakeBehavior::Return(result) => result,
                FakeBehavior::Panic => panic!("FakeGarminAuth::panicking was asked to check a key"),
            }
        })
    }
}

/// Runs every spawned future to completion right away, on the calling thread.
#[derive(Debug, Clone, Copy, Default)]
pub struct InlineSpawner;

impl Spawner for InlineSpawner {
    fn spawn(&self, fut: BoxFuture<()>) {
        futures::executor::block_on(fut);
    }
}

/// Keeps spawned futures until [`DeferredSpawner::run_pending`], to test what happens while a job is in flight.
#[derive(Default)]
pub struct DeferredSpawner {
    queue: Mutex<Vec<BoxFuture<()>>>,
}

impl DeferredSpawner {
    pub fn new() -> DeferredSpawner {
        DeferredSpawner::default()
    }

    /// How many futures wait to run.
    pub fn pending(&self) -> usize {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Runs the waiting futures (and any they spawn) to completion on the calling thread.
    pub fn run_pending(&self) {
        loop {
            // Taken out of the lock first: a running future may spawn another one.
            let batch = std::mem::take(&mut *self.queue.lock().unwrap_or_else(PoisonError::into_inner));
            if batch.is_empty() {
                return;
            }
            for fut in batch {
                futures::executor::block_on(fut);
            }
        }
    }
}

impl Spawner for DeferredSpawner {
    fn spawn(&self, fut: BoxFuture<()>) {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).push(fut);
    }
}

/// A clock that always says `now`.
pub fn fixed_clock(now: DateTime<Utc>) -> Arc<dyn Fn() -> DateTime<Utc> + Send + Sync> {
    Arc::new(move || now)
}

/// The place the test file store says its key lives in, unless `TestConfig::file_store_location` says otherwise.
pub const TEST_FILE_STORE_LOCATION: &str = "/test/secrets/garmin-personal-api-key";

/// How [`test_session_with`] builds the session's services.
#[derive(Debug, Clone)]
pub struct TestConfig {
    /// What the fake Garmin answers to every key check.
    pub garmin: Result<(), GarminAuthError>,
    /// The fake Garmin panics instead (overrides `garmin`).
    pub garmin_panics: bool,
    /// Whether a system secret store exists.
    pub system_store: bool,
    /// `SecretStores::prefer_file`, as in debug builds.
    pub prefer_file: bool,
    /// What the test file store reports as its location; `None` stands for a store that does not know where it keeps the key.
    pub file_store_location: Option<String>,
    /// Background futures wait for `DeferredSpawner::run_pending` instead of running inline.
    pub deferred: bool,
    /// The fixed time of the session's clock.
    pub now: DateTime<Utc>,
}

impl Default for TestConfig {
    fn default() -> TestConfig {
        TestConfig {
            garmin: Ok(()),
            garmin_panics: false,
            system_store: true,
            prefer_file: false,
            file_store_location: Some(TEST_FILE_STORE_LOCATION.to_owned()),
            deferred: false,
            now: Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap(),
        }
    }
}

/// The doubles behind a test session, for setting up state and checking what commands did.
pub struct TestHandles {
    /// The system secret store, `None` when `TestConfig::system_store` is off.
    pub system: Option<Arc<MemorySecretStore>>,
    pub file: Arc<MemorySecretStore>,
    pub garmin: Arc<FakeGarminAuth>,
    /// The spawner when `TestConfig::deferred` is on; otherwise futures run inline.
    pub spawner: Option<Arc<DeferredSpawner>>,
    /// How many times the session called `Services::notify`.
    pub notifications: Arc<AtomicUsize>,
}

/// A session on a fresh in-memory database with the doubles from `cfg`.
pub fn test_session_with(cfg: TestConfig) -> (Session, TestHandles) {
    test_session_on(Store::open_in_memory().expect("in-memory store"), cfg)
}

/// A session on `store` (for example a file database shared with a second connection) with the doubles from `cfg`.
pub fn test_session_on(store: Store, cfg: TestConfig) -> (Session, TestHandles) {
    let system = cfg.system_store.then(|| Arc::new(MemorySecretStore::new(SecretBackend::System)));
    let file = Arc::new(match &cfg.file_store_location {
        Some(location) => MemorySecretStore::new(SecretBackend::File).with_location(location),
        None => MemorySecretStore::new(SecretBackend::File),
    });
    let garmin = Arc::new(if cfg.garmin_panics { FakeGarminAuth::panicking() } else { FakeGarminAuth::returning(cfg.garmin) });
    let deferred = cfg.deferred.then(|| Arc::new(DeferredSpawner::new()));
    let spawner: Arc<dyn Spawner> = match &deferred {
        Some(deferred) => deferred.clone(),
        None => Arc::new(InlineSpawner),
    };
    let notifications = Arc::new(AtomicUsize::new(0));
    let counter = notifications.clone();
    let services = Services {
        secrets: Some(SecretStores { system: system.clone().map(|store| store as Arc<dyn SecretStore>), file: file.clone(), prefer_file: cfg.prefer_file }),
        garmin: Some(garmin.clone()),
        spawner: Some(spawner),
        notify: Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
        clock: fixed_clock(cfg.now),
    };
    (Session::new(store, services), TestHandles { system, file, garmin, spawner: deferred, notifications })
}

/// [`test_session_with`] with the default config.
pub fn test_session() -> Session {
    test_session_with(TestConfig::default()).0
}

/// A working connection made at `now` with the default key term, its key kept in `backend`.
pub fn connection_at(now: DateTime<Utc>, backend: SecretBackend) -> GarminConnection {
    GarminConnection {
        connected_at: now,
        key_term: KeyTerm::default(),
        key_expires_at: key_expires_at(now, KeyTerm::default()),
        last_verified_at: Some(now),
        last_status: ConnectionStatus::Connected,
        secret_backend: backend,
    }
}

/// Writes `connection` to the session's database, as a past `garmin.connect` would have.
pub fn save_connection(session: &Session, connection: &GarminConnection) {
    session.store().save_garmin_connection(connection).expect("save the Garmin connection");
}

/// The connection record in the session's database.
pub fn saved_connection(session: &Session) -> Option<GarminConnection> {
    session.store().garmin_connection().expect("read the Garmin connection")
}

/// The key in `store`, exposed for assertions.
pub fn stored_key(store: &MemorySecretStore) -> Option<String> {
    store.get().expect("read the test store").map(|key| key.expose_secret().to_owned())
}
