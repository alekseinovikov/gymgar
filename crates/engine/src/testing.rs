//! Test doubles for the engine's services and a ready-made test session, for the tests of this crate and of the
//! crates built on it (UI, MCP): secret stores in memory, a Garmin that answers from bytes the test gives it, spawners
//! that run jobs inline, later or never, a sleep that does not wait, and helpers that fill the database with activities.
//!
//! Compiled only for this crate's own tests and under the `testing` feature, which apps never enable. Nothing here
//! touches the network or the real keychain.

// Test-support code, compiled only under `cfg(test)` or the `testing` feature: the fakes panic on purpose
// (`FakeGarminAuth::panicking`) and fail loudly on misuse instead of returning errors.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeDelta, TimeZone, Utc};
use gymgar_domain::{
    ActivityId, ActivitySummary, ConnectionStatus, ExerciseId, GarminConnection, KeyTerm, Mass, PlannedTarget, RecordedSet, Reps, STRENGTH_TYPE_KEY,
    SecretBackend, StrengthRecord, key_expires_at,
};
use gymgar_store::{FileOutcome, NewActivity, Store};
use gymgar_sync::list;
use gymgar_sync::strength::{self, PARSER_VERSION};
use secrecy::{ExposeSecret, SecretString};

use crate::{
    BoxFuture, GarminAuth, GarminAuthError, GarminSession, JobEvent, JobFinished, SecretStore, SecretStoreError, SecretStores, Services, Session, Spawner,
};

/// One call a [`MemorySecretStore`] answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretOp {
    Get,
    Set,
    Delete,
}

/// A secret store in memory. `set_failing(true)` makes every operation fail, to test what a command leaves behind. It
/// records which thread made each call ([`MemorySecretStore::take_ops`]), so tests can tell that a real store's prompt
/// would not have frozen the window.
#[derive(Debug)]
pub struct MemorySecretStore {
    backend: SecretBackend,
    key: Mutex<Option<SecretString>>,
    failing: Mutex<bool>,
    location: Option<String>,
    ops: Mutex<Vec<(SecretOp, ThreadId)>>,
}

impl MemorySecretStore {
    pub fn new(backend: SecretBackend) -> MemorySecretStore {
        MemorySecretStore { backend, key: Mutex::new(None), failing: Mutex::new(false), location: None, ops: Mutex::new(Vec::new()) }
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

    /// The calls `get`, `set` and `delete` answered since the last call, in order, each with the thread that made it.
    /// [`stored_key`] reads the key without being counted.
    pub fn take_ops(&self) -> Vec<(SecretOp, ThreadId)> {
        std::mem::take(&mut *self.ops.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The key as it is now, for assertions: neither recorded as a call nor refused while the store is set to fail.
    pub fn peek(&self) -> Option<String> {
        self.key.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|key| key.expose_secret().to_owned())
    }

    /// Records the call `op`, then fails it if the store is set to fail.
    fn check(&self, op: SecretOp) -> Result<(), SecretStoreError> {
        self.ops.lock().unwrap_or_else(PoisonError::into_inner).push((op, thread::current().id()));
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
        self.check(SecretOp::Get)?;
        Ok(self.key.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|key| SecretString::from(key.expose_secret())))
    }

    fn set(&self, key: &SecretString) -> Result<(), SecretStoreError> {
        self.check(SecretOp::Set)?;
        *self.key.lock().unwrap_or_else(PoisonError::into_inner) = Some(SecretString::from(key.expose_secret()));
        Ok(())
    }

    fn delete(&self) -> Result<(), SecretStoreError> {
        self.check(SecretOp::Delete)?;
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

/// A Garmin that answers every key check the same way, remembers the keys it was given, and opens the same
/// [`FakeGarminSession`] for every key.
#[derive(Debug)]
pub struct FakeGarminAuth {
    behavior: FakeBehavior,
    last_key: Mutex<Option<String>>,
    calls: AtomicUsize,
    session: Arc<FakeGarminSession>,
    opened_key: Mutex<Option<String>>,
    opens: AtomicUsize,
}

impl FakeGarminAuth {
    /// A Garmin whose checks answer `result`; so does the check of the session it opens.
    pub fn returning(result: Result<(), GarminAuthError>) -> FakeGarminAuth {
        FakeGarminAuth::with(FakeBehavior::Return(result), Arc::new(FakeGarminSession::verifying(result)))
    }

    /// A Garmin whose check panics inside the future, and whose `open` panics, to test that a panicking job does not
    /// take the app down.
    pub fn panicking() -> FakeGarminAuth {
        FakeGarminAuth::with(FakeBehavior::Panic, Arc::new(FakeGarminSession::new()))
    }

    /// The same Garmin, opening `session` for every key.
    pub fn with_session(mut self, session: Arc<FakeGarminSession>) -> FakeGarminAuth {
        self.session = session;
        self
    }

    fn with(behavior: FakeBehavior, session: Arc<FakeGarminSession>) -> FakeGarminAuth {
        FakeGarminAuth { behavior, last_key: Mutex::new(None), calls: AtomicUsize::new(0), session, opened_key: Mutex::new(None), opens: AtomicUsize::new(0) }
    }

    /// How many sessions `open` has opened.
    pub fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    /// The key of the last `open` call, `None` when it was never called.
    pub fn opened_key(&self) -> Option<String> {
        self.opened_key.lock().unwrap_or_else(PoisonError::into_inner).clone()
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

    fn open(&self, key: SecretString) -> Result<Arc<dyn GarminSession>, GarminAuthError> {
        if let FakeBehavior::Panic = self.behavior {
            panic!("FakeGarminAuth::panicking was asked to open a session");
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        *self.opened_key.lock().unwrap_or_else(PoisonError::into_inner) = Some(key.expose_secret().to_owned());
        Ok(self.session.clone())
    }
}

/// A request the fake Garmin session was asked to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FakeRequest {
    Verify,
    /// A page of the list, by the `start` it was asked for.
    Page(u32),
    File(ActivityId),
}

impl FakeRequest {
    pub fn kind(self) -> RequestKind {
        match self {
            FakeRequest::Verify => RequestKind::Verify,
            FakeRequest::Page(_) => RequestKind::Page,
            FakeRequest::File(_) => RequestKind::File,
        }
    }
}

/// The kinds of requests, for counting them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestKind {
    Verify,
    Page,
    File,
}

/// A Garmin session that answers from bytes the test gives it: the pages of the list in order, a file per activity
/// (none is a 404), and, before those, any errors the test scripted for a request. As in Garmin's list, `start` counts
/// elements: a page begins where the elements of the pages before it end, and a `start` where no page begins gets an
/// empty list. It records every request, in order.
#[derive(Debug, Default)]
pub struct FakeGarminSession {
    /// What `verify` answers once its scripted errors are used up.
    verify: Option<GarminAuthError>,
    pages: Mutex<Vec<Vec<u8>>>,
    files: Mutex<HashMap<ActivityId, Vec<u8>>>,
    failures: Mutex<HashMap<FakeRequest, VecDeque<GarminAuthError>>>,
    requests: Mutex<Vec<FakeRequest>>,
}

impl FakeGarminSession {
    /// A Garmin that accepts the key and lists nothing.
    pub fn new() -> FakeGarminSession {
        FakeGarminSession::default()
    }

    /// A Garmin whose key check always answers `result`.
    pub fn verifying(result: Result<(), GarminAuthError>) -> FakeGarminSession {
        FakeGarminSession { verify: result.err(), ..FakeGarminSession::default() }
    }

    /// The pages of the list, newest first; each is a body as Garmin sends it.
    pub fn set_pages(&self, pages: Vec<Vec<u8>>) {
        *self.pages.lock().unwrap_or_else(PoisonError::into_inner) = pages;
    }

    /// The file of activity `id`, as Garmin sends it.
    pub fn set_file(&self, id: ActivityId, zip: Vec<u8>) {
        self.files.lock().unwrap_or_else(PoisonError::into_inner).insert(id, zip);
    }

    /// The next times `request` is made, it fails with `errors`, one per request, before it answers as usual again.
    pub fn fail(&self, request: FakeRequest, errors: impl IntoIterator<Item = GarminAuthError>) {
        self.failures.lock().unwrap_or_else(PoisonError::into_inner).entry(request).or_default().extend(errors);
    }

    /// The requests made so far, in order.
    pub fn requests(&self) -> Vec<FakeRequest> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The requests made so far, forgetting them, so a test can look at the next sync on its own.
    pub fn take_requests(&self) -> Vec<FakeRequest> {
        std::mem::take(&mut *self.requests.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// How many requests of this kind were made.
    pub fn count(&self, kind: RequestKind) -> usize {
        self.requests().into_iter().filter(|request| request.kind() == kind).count()
    }

    /// Records `request` and answers it: a scripted error first, else `answer`.
    fn answer<T: Send + 'static>(&self, request: FakeRequest, answer: impl FnOnce() -> Result<T, GarminAuthError>) -> BoxFuture<Result<T, GarminAuthError>> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        let scripted = self.failures.lock().unwrap_or_else(PoisonError::into_inner).get_mut(&request).and_then(VecDeque::pop_front);
        let result = match scripted {
            Some(error) => Err(error),
            None => answer(),
        };
        Box::pin(std::future::ready(result))
    }
}

impl GarminSession for FakeGarminSession {
    fn verify(&self) -> BoxFuture<Result<(), GarminAuthError>> {
        self.answer(FakeRequest::Verify, || self.verify.map_or(Ok(()), Err))
    }

    fn activities_page(&self, start: u32, _limit: u32) -> BoxFuture<Result<Vec<u8>, GarminAuthError>> {
        self.answer(FakeRequest::Page(start), || {
            let pages = self.pages.lock().unwrap_or_else(PoisonError::into_inner);
            let mut begins = 0_usize;
            for page in pages.iter() {
                if u32::try_from(begins).ok() == Some(start) {
                    return Ok(page.clone());
                }
                // A body that is not an array has no elements: the next page begins where it does.
                begins += serde_json::from_slice::<Vec<serde_json::Value>>(page).map_or(0, |elements| elements.len());
            }
            Ok(b"[]".to_vec())
        })
    }

    fn activity_file(&self, id: ActivityId) -> BoxFuture<Result<Vec<u8>, GarminAuthError>> {
        self.answer(FakeRequest::File(id), || self.files.lock().unwrap_or_else(PoisonError::into_inner).get(&id).cloned().ok_or(GarminAuthError::NotFound))
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

    /// Drops the waiting futures without running them, as a runtime that shuts down does.
    pub fn drop_pending(&self) {
        // Taken out of the lock first: a dropped job reports itself, and that must not happen under the lock.
        let batch = std::mem::take(&mut *self.queue.lock().unwrap_or_else(PoisonError::into_inner));
        drop(batch);
    }
}

impl Spawner for DeferredSpawner {
    fn spawn(&self, fut: BoxFuture<()>) {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).push(fut);
    }
}

/// Drops every spawned future without running it, as a runtime that shuts down before a job got its turn.
#[derive(Debug, Clone, Copy, Default)]
pub struct DroppingSpawner;

impl Spawner for DroppingSpawner {
    fn spawn(&self, fut: BoxFuture<()>) {
        drop(fut);
    }
}

/// A sleep that returns at once and records how long it was asked to wait, so tests check pauses without waiting.
/// Clones share the record.
#[derive(Debug, Clone, Default)]
pub struct RecordingSleep {
    slept: Arc<Mutex<Vec<Duration>>>,
}

impl RecordingSleep {
    pub fn new() -> RecordingSleep {
        RecordingSleep::default()
    }

    /// The waits asked for so far, in order.
    pub fn slept(&self) -> Vec<Duration> {
        self.slept.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// This sleep as [`Services::sleep`].
    pub fn sleep_fn(&self) -> Arc<dyn Fn(Duration) -> BoxFuture<()> + Send + Sync> {
        let slept = self.slept.clone();
        Arc::new(move |duration| {
            slept.lock().unwrap_or_else(PoisonError::into_inner).push(duration);
            Box::pin(std::future::ready(()))
        })
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
    /// The session the fake Garmin opens for a sync; `None` for one that answers its key check as `garmin` says and
    /// lists nothing.
    pub session: Option<Arc<FakeGarminSession>>,
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
            session: None,
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
    /// The session the fake Garmin opens for a sync.
    pub session: Arc<FakeGarminSession>,
    /// The spawner when `TestConfig::deferred` is on; otherwise futures run inline.
    pub spawner: Option<Arc<DeferredSpawner>>,
    /// How many times the session called `Services::notify`.
    pub notifications: Arc<AtomicUsize>,
    /// The session's sleep: it records the waits of the jobs and returns at once.
    pub sleep: RecordingSleep,
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
    let session = cfg.session.clone().unwrap_or_else(|| Arc::new(FakeGarminSession::verifying(cfg.garmin)));
    let garmin = Arc::new(if cfg.garmin_panics { FakeGarminAuth::panicking() } else { FakeGarminAuth::returning(cfg.garmin) }.with_session(session.clone()));
    let deferred = cfg.deferred.then(|| Arc::new(DeferredSpawner::new()));
    let spawner: Arc<dyn Spawner> = match &deferred {
        Some(deferred) => deferred.clone(),
        None => Arc::new(InlineSpawner),
    };
    let notifications = Arc::new(AtomicUsize::new(0));
    let counter = notifications.clone();
    let sleep = RecordingSleep::new();
    let services = Services {
        secrets: Some(SecretStores { system: system.clone().map(|store| store as Arc<dyn SecretStore>), file: file.clone(), prefer_file: cfg.prefer_file }),
        garmin: Some(garmin.clone()),
        spawner: Some(spawner),
        notify: Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
        clock: fixed_clock(cfg.now),
        sleep: sleep.sleep_fn(),
    };
    (Session::new(store, services), TestHandles { system, file, garmin, session, spawner: deferred, notifications, sleep })
}

/// [`test_session_with`] with the default config.
pub fn test_session() -> Session {
    test_session_with(TestConfig::default()).0
}

/// Applies what the background jobs have sent ([`Session::poll_jobs`]) and returns the jobs that finished, for jobs
/// that send no partial results. Panics on a partial result.
pub fn poll_finished(session: &mut Session) -> Vec<JobFinished> {
    session
        .poll_jobs()
        .into_iter()
        .map(|event| match event {
            JobEvent::Finished(finished) => finished,
            JobEvent::Partial { id, command, .. } => panic!("job {} of {command} sent a partial result", id.0),
        })
        .collect()
}

/// Applies everything the background jobs have sent, however many calls of [`Session::poll_jobs`] that takes, and
/// returns the events in order. For jobs that have already run (the inline spawner, or `DeferredSpawner::run_pending`).
pub fn poll_all(session: &mut Session) -> Vec<JobEvent> {
    let mut events = Vec::new();
    loop {
        let polled = session.poll_jobs();
        if polled.is_empty() {
            return events;
        }
        events.extend(polled);
    }
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

/// The key in `store`, exposed for assertions. Not a call of the store: [`MemorySecretStore::take_ops`] does not see it.
pub fn stored_key(store: &MemorySecretStore) -> Option<String> {
    store.peek()
}

/// A page of Garmin's activity list (scrubbed): eight activities of five types, newest first, two of them strength.
pub const FIXTURE_ACTIVITY_PAGE: &[u8] = include_bytes!("../../../fixtures/garmin/activities-page.json");

/// The file (scrubbed) of the newest strength activity of [`FIXTURE_ACTIVITY_PAGE`]: a workout the watch led step by step,
/// 31 sets.
pub const FIXTURE_STRENGTH_ZIP: &[u8] = include_bytes!("../../../fixtures/garmin/strength-plan.zip");

/// Stores the activities of [`FIXTURE_ACTIVITY_PAGE`] as a first sync would (nothing new), and the file of its newest
/// strength activity, [`FIXTURE_STRENGTH_ZIP`]; the other strength activity still waits for its file. Both go through
/// `gymgar-sync` like downloaded bytes, without the network. Returns the ids in the list's order, newest first.
pub fn seed_fixture_activities(session: &Session) -> Vec<ActivityId> {
    let items: Vec<NewActivity> = list::parse_page(FIXTURE_ACTIVITY_PAGE)
        .expect("the list fixture reads")
        .items
        .into_iter()
        .map(|listed| NewActivity { summary: listed.summary, raw_json: listed.raw_json, is_new: false })
        .collect();
    session.store().upsert_activities(&items, session.now()).expect("store the listed activities");
    let strength = items.iter().find(|item| item.summary.is_strength()).expect("a strength activity in the list fixture").summary.id;
    let record = strength::parse_strength(FIXTURE_STRENGTH_ZIP).expect("the strength fixture reads");
    let outcome = FileOutcome::Parsed { zip: Some(FIXTURE_STRENGTH_ZIP.to_vec()), record, parser_version: PARSER_VERSION };
    session.store().apply_file(strength, outcome, session.now()).expect("store the strength file");
    items.into_iter().map(|item| item.summary.id).collect()
}

/// Stores a strength activity holding exactly `sets`, as if its file had been read (no file is stored), and returns its
/// id, one past the highest stored. It has no name, starts with its first set (an hour before the session's clock
/// without sets) and is not new.
pub fn seed_strength(session: &Session, sets: Vec<RecordedSet>) -> ActivityId {
    let known = session.store().sync_snapshot().expect("read the stored activities").known;
    let id = ActivityId(known.iter().map(|id| id.0).max().map_or(1, |max| max + 1));
    let start = sets.iter().map(|set| set.start).min().unwrap_or_else(|| session.now() - TimeDelta::hours(1));
    let summary = ActivitySummary {
        id,
        type_key: STRENGTH_TYPE_KEY.to_owned(),
        name: None,
        start,
        duration_ms: Some(3_600_000),
        distance_m: None,
        avg_hr: None,
        calories: None,
        manual: false,
    };
    let raw_json = serde_json::json!({ "activityId": id.0, "activityType": { "typeKey": STRENGTH_TYPE_KEY } }).to_string();
    session.store().upsert_activities(&[NewActivity { summary, raw_json, is_new: false }], session.now()).expect("store the activity");
    let outcome = FileOutcome::Parsed { zip: None, record: StrengthRecord { workout_name: None, sets, titles: Vec::new() }, parser_version: PARSER_VERSION };
    session.store().apply_file(id, outcome, session.now()).expect("store the sets");
    id
}

/// A set of the watch's record for tests: the exercise, the weight in kilograms (0.0 is a set that has a weight of zero, as a
/// bodyweight set does), the reps and the reps the workout plan asked for (`None`: the plan asked for none). It lasts half a
/// minute, and [`seed_workout`] says when it began.
pub fn strength_set(index: u16, exercise: ExerciseId, weight_kg: f64, reps: u32, plan_reps: Option<u32>) -> RecordedSet {
    RecordedSet {
        message_index: index,
        start: DateTime::<Utc>::UNIX_EPOCH,
        duration_ms: 30_000,
        reps: Some(Reps::new(reps).expect("reps in range")),
        weight: Some(Mass::from_kg(weight_kg).expect("a weight in range")),
        exercise: Some(exercise),
        step_index: None,
        plan: plan_reps.map(|plan| PlannedTarget { reps: Some(Reps::new(plan).expect("planned reps in range")), weight: None }),
        hr_avg: None,
        hr_max: None,
        rest_after_ms: None,
    }
}

/// Stores the strength activity `id` that began at `local_start` (`2026-10-20 07:55:00`, the wall clock where the user was,
/// which Garmin sends as `startTimeLocal`) in a zone `utc_offset_hours` ahead of UTC, as if its file had been read (no file is
/// stored). It holds `sets` and is named `workout_name`; each set begins one minute per message index after the activity,
/// whatever `start` it came with. It is not new. Returns `id`.
pub fn seed_workout(store: &Store, id: i64, local_start: &str, utc_offset_hours: i32, workout_name: Option<&str>, sets: Vec<RecordedSet>) -> ActivityId {
    let local = NaiveDateTime::parse_from_str(local_start, "%Y-%m-%d %H:%M:%S").expect("a local start like 2026-10-20 07:55:00");
    let start = Utc.from_utc_datetime(&(local - TimeDelta::hours(i64::from(utc_offset_hours))));
    let sets: Vec<RecordedSet> = sets.into_iter().map(|set| RecordedSet { start: start + TimeDelta::minutes(i64::from(set.message_index)), ..set }).collect();
    let id = ActivityId(id);
    let summary = ActivitySummary {
        id,
        type_key: STRENGTH_TYPE_KEY.to_owned(),
        name: None,
        start,
        duration_ms: Some(3_600_000),
        distance_m: None,
        avg_hr: None,
        calories: None,
        manual: false,
    };
    let raw_json = serde_json::json!({ "activityId": id.0, "activityType": { "typeKey": STRENGTH_TYPE_KEY }, "startTimeLocal": local_start }).to_string();
    let now = start + TimeDelta::hours(2);
    store.upsert_activities(&[NewActivity { summary, raw_json, is_new: false }], now).expect("store the activity");
    let record = StrengthRecord { workout_name: workout_name.map(str::to_owned), sets, titles: Vec::new() };
    store.apply_file(id, FileOutcome::Parsed { zip: None, record, parser_version: PARSER_VERSION }, now).expect("store the sets");
    id
}
