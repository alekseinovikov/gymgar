//! The bodies of the import's background jobs: `garmin.sync` (activity import spec §7, «Фоновые задачи: загрузка»,
//! «Темп и повторы») and `activity.reparse`.
//!
//! A job works from what it was handed at its start and never looks at the database. It makes one Garmin request at a
//! time, parses the answer itself (a file takes tens of milliseconds, which is no work for the UI thread), and sends
//! every piece to the session as a partial result: a page of the list, one file. The session applies each in its own
//! transaction, so a sync that stops half-way (an error, the window closed) keeps what it got, and the next one goes on
//! from there. Pauses and retries go through the session's sleep, so tests never wait.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gymgar_domain::{ActivityId, SyncErrorKind};
use gymgar_store::{FileOutcome, NewActivity, SyncSnapshot};
use gymgar_sync::list::{self, ListPage, ListedActivity};
use gymgar_sync::policy::{self, FILE_PAUSE, PAGE_SIZE, RETRY_PAUSES};
use gymgar_sync::strength::{self, PARSER_VERSION};

use crate::garmin_cmds::read_key;
use crate::jobs::{JobCtx, JobOutput};
use crate::{EngineError, GarminAuth, GarminAuthError, GarminSession, SecretStore};

/// Pages in a row without one readable activity after which paging stops. A list where hundreds of elements in a row
/// cannot be read is broken (or a Garmin that ignores `start`), and asking on would only repeat it.
const MAX_UNREADABLE_PAGES: u32 = 3;

/// What a sync starts from, read on the session's thread.
pub(crate) struct SyncInput {
    pub(crate) snapshot: SyncSnapshot,
    /// The secret store the recorded connection keeps its key in; `None` when that store is unavailable here.
    pub(crate) key_store: Option<Arc<dyn SecretStore>>,
    /// The session an earlier sync of this run opened and Garmin accepted; with it, the key is neither read nor checked.
    pub(crate) cached: Option<Arc<dyn GarminSession>>,
    pub(crate) garmin: Arc<dyn GarminAuth>,
}

/// `garmin.sync`: check the key (unless a session is kept), page through the list, then fetch the strength files.
pub(crate) async fn run(ctx: JobCtx, input: SyncInput) -> JobOutput {
    let SyncInput { snapshot, key_store, cached, garmin } = input;
    let session = match cached {
        Some(session) => session,
        None => match open(&ctx, key_store, garmin.as_ref()).await {
            Ok(session) => session,
            Err(kind) => return JobOutput::SyncFinished { result: Err(kind), session: None },
        },
    };
    let result = download(&ctx, session.as_ref(), &snapshot).await;
    if result == Err(SyncErrorKind::KeyRejected) {
        // Garmin refused a key it had accepted (deleted or expired since): the status says so, as a key check would,
        // and the session goes, so the next sync reads the key again and checks it first.
        ctx.send(JobOutput::SyncVerified { result: Err(EngineError::Garmin(GarminAuthError::KeyRejected)) });
        return JobOutput::SyncFinished { result, session: None };
    }
    // Any other stop leaves a key Garmin accepted: the next sync of this run goes on with this session.
    JobOutput::SyncFinished { result, session: Some(session) }
}

/// Reads the key, opens a session for it and asks Garmin about it, sending what the check found out. The session is
/// only handed back if Garmin accepted the key.
async fn open(ctx: &JobCtx, key_store: Option<Arc<dyn SecretStore>>, garmin: &dyn GarminAuth) -> Result<Arc<dyn GarminSession>, SyncErrorKind> {
    // Read inside the job, off the session's thread: the system store may wait for the user to allow access.
    let opened = read_key(key_store).and_then(|key| garmin.open(key).map_err(EngineError::from));
    let checked = match opened {
        Ok(session) => with_retries(ctx, may_pass_or_unexpected, || session.verify()).await.map(|()| session).map_err(EngineError::from),
        Err(error) => Err(error),
    };
    ctx.send(JobOutput::SyncVerified { result: checked.as_ref().map(|_| ()).map_err(Clone::clone) });
    checked.map_err(|error| match error {
        EngineError::Garmin(error) => sync_error(error),
        // No key, or a store that cannot be read: the user's way out is the same, connecting again.
        _ => SyncErrorKind::KeyMissing,
    })
}

/// Pages through the list, then fetches the files: steps 3 and 4 of the algorithm.
async fn download(ctx: &JobCtx, garmin: &dyn GarminSession, snapshot: &SyncSnapshot) -> Result<(), SyncErrorKind> {
    let listed_files = page_through(ctx, garmin, snapshot).await?;
    fetch_files(ctx, garmin, &file_queue(listed_files, &snapshot.pending_files)).await
}

/// Pages through Garmin's list, newest first, sending every page, until [`policy::keep_paging`] says enough or the list
/// runs out: an empty array, and nothing else, is its end. Each page starts after every element of the one before,
/// readable or not. Returns the activities on the pages whose file is to be fetched, with their starts: the strength
/// activities recorded by a device that this sync is the first to see, and those an earlier sync left waiting.
///
/// While the history is complete, a page without a known activity lies beyond what is stored (more new activities than a
/// page holds): it is sent marked so, which takes the completeness back, and only reaching a known activity gives it
/// back. A sync that stops in between leaves the history incomplete, and the next one pages to the end of the list.
async fn page_through(ctx: &JobCtx, garmin: &dyn GarminSession, snapshot: &SyncSnapshot) -> Result<Vec<(DateTime<Utc>, ActivityId)>, SyncErrorKind> {
    let had_activities = !snapshot.known.is_empty();
    let pending: HashSet<ActivityId> = snapshot.pending_files.iter().copied().collect();
    // Review Focus 1: an activity that appears while the list is paged shifts the offsets, so the last activity of one
    // page comes again at the top of the next. It is stored and fetched once.
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut start = 0_u32;
    let mut unreadable_pages = 0_u32;
    // Whether a page beyond the stored history was sent, so the history is no longer recorded as complete.
    let mut went_beyond = false;
    loop {
        let ListPage { items: page, raw_count } = with_retries(ctx, may_pass_or_unexpected, || read_page(garmin, start)).await.map_err(sync_error)?;
        if raw_count == 0 {
            ctx.send(JobOutput::HistoryComplete);
            return Ok(files);
        }
        if page.is_empty() {
            // Elements none of which could be read (each is logged): not the end of the list, so on to the next page.
            unreadable_pages = unreadable_pages.saturating_add(1);
            if unreadable_pages >= MAX_UNREADABLE_PAGES {
                log::warn!("{unreadable_pages} pages of the activity list in a row up to {start} have nothing readable; paging stops");
                return Ok(files);
            }
        } else {
            unreadable_pages = 0;
            let ids: Vec<ActivityId> = page.iter().map(|listed| listed.summary.id).collect();
            let mut items = Vec::with_capacity(page.len());
            for ListedActivity { summary, raw_json } in page {
                if !seen.insert(summary.id) {
                    continue;
                }
                let first_seen = !snapshot.known.contains(&summary.id);
                if (first_seen && summary.is_strength() && !summary.manual) || pending.contains(&summary.id) {
                    files.push((summary.start, summary.id));
                }
                let is_new = had_activities && policy::is_new(summary.start, snapshot.newest_start);
                items.push(NewActivity { summary, raw_json, is_new });
            }
            if items.is_empty() {
                // The whole page came earlier in this sync: Garmin is repeating itself, and asking on would only repeat it.
                log::warn!("the activity list at {start} repeats what this sync has seen; paging stops");
                return Ok(files);
            }
            let more = policy::keep_paging(&ids, &snapshot.known, snapshot.history_complete);
            // With a complete history paging goes on only past pages that hold nothing known.
            let beyond_history = snapshot.history_complete && more;
            went_beyond |= beyond_history;
            ctx.send(JobOutput::ActivityPage { items, beyond_history });
            if !more {
                if went_beyond {
                    // Back to what is stored: the gap is closed (unless a page could not be written, which the session checks).
                    ctx.send(JobOutput::HistoryComplete);
                }
                return Ok(files);
            }
        }
        let Some(next) = u32::try_from(raw_count).ok().and_then(|count| start.checked_add(count)) else { return Ok(files) };
        start = next;
    }
}

/// One page of the list, read. A body the mapping cannot read at all is an answer Garmin should not give, like a 5xx.
async fn read_page(garmin: &dyn GarminSession, start: u32) -> Result<ListPage, GarminAuthError> {
    let body = garmin.activities_page(start, PAGE_SIZE).await?;
    list::parse_page(&body).map_err(|error| {
        log::warn!("the activity list at {start} cannot be read: {error}");
        GarminAuthError::Unexpected
    })
}

/// The files to fetch, newest first, each once. Those seen on the pages are ordered by their start; those an earlier
/// sync left waiting that the pages did not reach are older than everything on them, and come last, in the order the
/// database gave them (newest first).
fn file_queue(mut listed: Vec<(DateTime<Utc>, ActivityId)>, pending: &[ActivityId]) -> Vec<ActivityId> {
    listed.sort_by(|a, b| b.cmp(a));
    let mut queued = HashSet::new();
    listed.into_iter().map(|(_, id)| id).chain(pending.iter().copied()).filter(|id| queued.insert(*id)).collect()
}

/// Fetches and reads the files one by one, with a pause before each but the first, sending each file and the progress.
/// A file Garmin answers with something the client does not understand (a body over the size cap, say) is not asked
/// for again now: asking again would most likely get the same, and stopping would hold up every older file behind it
/// on every sync. It stays waiting for the next sync.
async fn fetch_files(ctx: &JobCtx, garmin: &dyn GarminSession, queue: &[ActivityId]) -> Result<(), SyncErrorKind> {
    if queue.is_empty() {
        return Ok(());
    }
    // Only for the status line: a queue longer than `u32::MAX` cannot be held in memory anyway.
    let total = u32::try_from(queue.len()).unwrap_or(u32::MAX);
    ctx.send(JobOutput::Progress { done: 0, total });
    let mut done = 0_u32;
    for &id in queue {
        if done > 0 {
            ctx.sleep(FILE_PAUSE).await;
        }
        let outcome = match with_retries(ctx, may_pass, || garmin.activity_file(id)).await {
            Ok(zip) => Some(read_file(id, zip, true)),
            // Garmin has no file for it (an entry made by hand, say): it is not asked for again.
            Err(GarminAuthError::NotFound) => Some(FileOutcome::Missing),
            Err(GarminAuthError::Unexpected) => {
                log::warn!("Garmin answered the file of activity {} with something unexpected; it waits for the next sync", id.0);
                None
            }
            Err(error) => return Err(sync_error(error)),
        };
        if let Some(outcome) = outcome {
            ctx.send(JobOutput::ActivityFile { id, outcome });
        }
        done = done.saturating_add(1);
        ctx.send(JobOutput::Progress { done, total });
    }
    Ok(())
}

/// What a file holds, or that this parser cannot read it; the bytes are kept either way. `downloaded` says whether a
/// parsed file's bytes are still to be stored (a download) or are already (a re-parse).
fn read_file(id: ActivityId, zip: Vec<u8>, downloaded: bool) -> FileOutcome {
    match strength::parse_strength(&zip) {
        Ok(record) => FileOutcome::Parsed { zip: downloaded.then_some(zip), record, parser_version: PARSER_VERSION },
        Err(error) => {
            // The error names what is wrong with the file, never what is in it.
            log::warn!("the file of activity {} cannot be read: {error}", id.0);
            FileOutcome::Unreadable { zip, parser_version: PARSER_VERSION }
        }
    }
}

/// `activity.reparse`: reads every stored file again and sends what this parser makes of it. No network.
pub(crate) async fn reparse(ctx: JobCtx, files: Vec<(ActivityId, Vec<u8>)>) -> JobOutput {
    let total = u32::try_from(files.len()).unwrap_or(u32::MAX);
    for (id, zip) in files {
        ctx.send(JobOutput::ActivityFile { id, outcome: read_file(id, zip, false) });
    }
    log::info!("read {total} stored activity files again");
    JobOutput::ReparseFinished { files: total }
}

/// Runs `request` and, when it fails in a way `retried` says may pass, again after each pause of [`RETRY_PAUSES`]; the
/// last failure is returned. Anything else (a refused key, a missing file) is an answer, not a hiccup, and is returned
/// at once.
async fn with_retries<T, F, Fut>(ctx: &JobCtx, retried: fn(GarminAuthError) -> bool, mut request: F) -> Result<T, GarminAuthError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, GarminAuthError>>,
{
    let mut pauses = RETRY_PAUSES.iter();
    loop {
        match request().await {
            Err(error) if retried(error) => {
                let Some(&pause) = pauses.next() else { return Err(error) };
                log::info!("a Garmin request failed ({error}); trying again in {} s", pause.as_secs());
                ctx.sleep(pause).await;
            }
            outcome => return outcome,
        }
    }
}

/// A failure that may pass if asked again later: Garmin limiting requests, a server error, the network.
fn may_pass(error: GarminAuthError) -> bool {
    match error {
        GarminAuthError::RateLimited | GarminAuthError::Unavailable | GarminAuthError::Network => true,
        GarminAuthError::KeyRejected | GarminAuthError::NotFound | GarminAuthError::Unexpected => false,
    }
}

/// [`may_pass`], and an answer not understood too: for the key check and the list, which cannot be skipped the way one
/// file can, a garbled answer (a maintenance page, say) is most likely Garmin having a bad moment.
fn may_pass_or_unexpected(error: GarminAuthError) -> bool {
    may_pass(error) || error == GarminAuthError::Unexpected
}

/// Why a sync stopped, in the terms the user is told about (spec §7, «Ошибки загрузки»).
fn sync_error(error: GarminAuthError) -> SyncErrorKind {
    match error {
        GarminAuthError::KeyRejected => SyncErrorKind::KeyRejected,
        GarminAuthError::RateLimited => SyncErrorKind::RateLimited,
        GarminAuthError::Network => SyncErrorKind::Network,
        // A 404 where there must be something (the list, the token) is as much an answer we do not understand.
        GarminAuthError::Unavailable | GarminAuthError::Unexpected | GarminAuthError::NotFound => SyncErrorKind::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobMessage, Jobs};
    use crate::testing::{FakeGarminAuth, FakeGarminSession, InlineSpawner, RecordingSleep};
    use chrono::TimeZone;
    use serde_json::json;

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, d, 10, 0, 0).unwrap()
    }

    /// The ids of every page a sync sends, in order, when Garmin lists `pages` and nothing is stored yet.
    fn pages_sent(pages: &[serde_json::Value]) -> Vec<Vec<i64>> {
        let garmin = Arc::new(FakeGarminSession::new());
        garmin.set_pages(pages.iter().map(|page| page.to_string().into_bytes()).collect());
        let snapshot = SyncSnapshot { known: HashSet::new(), history_complete: false, newest_start: None, pending_files: Vec::new() };
        let input = SyncInput { snapshot, key_store: None, cached: Some(garmin.clone()), garmin: Arc::new(FakeGarminAuth::returning(Ok(()))) };
        let mut jobs = Jobs::new();
        jobs.start("garmin.sync", &InlineSpawner, Arc::new(|| {}), RecordingSleep::new().sleep_fn(), move |ctx| run(ctx, input));
        std::iter::from_fn(|| jobs.take())
            .filter_map(|(_, _, message)| match message {
                JobMessage::Partial(JobOutput::ActivityPage { items, .. }) => Some(items.iter().map(|item| item.summary.id.0).collect()),
                _ => None,
            })
            .collect()
    }

    fn running(id: i64, d: u32) -> serde_json::Value {
        json!({ "activityId": id, "activityType": { "typeKey": "running" }, "startTimeGMT": format!("2026-10-{d:02} 10:00:00") })
    }

    #[test]
    fn an_activity_a_page_repeats_is_sent_once() {
        // Review Focus 1: a new activity shifted the offsets, and 2 slid from the first page onto the second.
        let sent = pages_sent(&[json!([running(3, 22), running(2, 20)]), json!([running(2, 20), running(1, 18)])]);
        assert_eq!(sent, [vec![3, 2], vec![1]]);
    }

    #[test]
    fn file_queue_is_newest_first_without_repeats() {
        let listed = vec![(day(20), ActivityId(2)), (day(22), ActivityId(3)), (day(18), ActivityId(1)), (day(22), ActivityId(3))];
        // 9 and 8 were waiting and are beyond the pages; 1 was waiting and is on them.
        let pending = [ActivityId(9), ActivityId(1), ActivityId(8)];
        assert_eq!(file_queue(listed, &pending), [3, 2, 1, 9, 8].map(ActivityId));
        assert!(file_queue(Vec::new(), &[]).is_empty());
    }

    #[test]
    fn what_is_retried() {
        for error in [GarminAuthError::RateLimited, GarminAuthError::Unavailable, GarminAuthError::Network] {
            assert!(may_pass(error) && may_pass_or_unexpected(error), "{error:?}");
        }
        // A file is not asked for again on an answer not understood; the key check and the list are.
        assert!(!may_pass(GarminAuthError::Unexpected));
        assert!(may_pass_or_unexpected(GarminAuthError::Unexpected));
        for error in [GarminAuthError::KeyRejected, GarminAuthError::NotFound] {
            assert!(!may_pass(error) && !may_pass_or_unexpected(error), "{error:?}");
        }
    }

    #[test]
    fn what_the_user_is_told() {
        let cases = [
            (GarminAuthError::KeyRejected, SyncErrorKind::KeyRejected),
            (GarminAuthError::RateLimited, SyncErrorKind::RateLimited),
            (GarminAuthError::Network, SyncErrorKind::Network),
            (GarminAuthError::Unavailable, SyncErrorKind::Unavailable),
            (GarminAuthError::Unexpected, SyncErrorKind::Unavailable),
            (GarminAuthError::NotFound, SyncErrorKind::Unavailable),
        ];
        for (error, kind) in cases {
            assert_eq!(sync_error(error), kind, "{error:?}");
        }
    }
}
