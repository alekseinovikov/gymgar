//! The activity import's pacing and stopping rules.
//!
//! Only the rules live here, as constants and pure functions; the engine's job decides when to sleep and which request to make
//! next. Garmin answers 429 to a client that is in a hurry, so the import makes one request at a time, with pauses.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use gymgar_domain::ActivityId;

/// How many activities one list request asks for.
pub const PAGE_SIZE: u32 = 100;

/// The pause between two file downloads.
pub const FILE_PAUSE: Duration = Duration::from_secs(2);

/// The pauses before the 1st, 2nd and 3rd retry of a request that failed with 429, 5xx or a network error. After the third retry
/// fails the import stops.
pub const RETRY_PAUSES: [Duration; 3] = [Duration::from_secs(30), Duration::from_secs(120), Duration::from_secs(300)];

/// Whether to ask for the next (older) page after `page`.
pub fn keep_paging(page: &[ActivityId], known: &HashSet<ActivityId>, history_complete: bool) -> bool {
    // The list runs from newest to oldest. Once the history is complete, the first activity we already have means everything
    // beyond it is ours too. While it is not (an earlier import was interrupted), gaps may lie behind known activities: go on to the end.
    // The engine keeps "complete" honest: a page with nothing known takes it back until paging reaches a known activity again.
    !page.is_empty() && !(history_complete && page.iter().any(|id| known.contains(id)))
}

/// Whether an activity that started at `start` is new to the user: only when the database already held activities before this import
/// (`newest_before` is the start of the newest of them) and this one started after it. The first import marks nothing, so a hundred
/// old workouts never light up at once.
pub fn is_new(start: DateTime<Utc>, newest_before: Option<DateTime<Utc>>) -> bool {
    newest_before.is_some_and(|newest| start > newest)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ids(values: &[i64]) -> Vec<ActivityId> {
        values.iter().copied().map(ActivityId).collect()
    }

    fn known(values: &[i64]) -> HashSet<ActivityId> {
        ids(values).into_iter().collect()
    }

    #[test]
    fn pacing_constants() {
        assert_eq!(PAGE_SIZE, 100);
        assert_eq!(FILE_PAUSE, Duration::from_secs(2));
        assert_eq!(RETRY_PAUSES, [Duration::from_secs(30), Duration::from_secs(120), Duration::from_secs(300)]);
    }

    #[test]
    fn paging_rule() {
        // An empty page is the end of the list, whatever else is true.
        assert!(!keep_paging(&[], &known(&[1]), false));
        assert!(!keep_paging(&[], &known(&[]), true));
        // The history is not complete yet: known activities on a page are no reason to stop, there may be gaps further back.
        assert!(keep_paging(&ids(&[3, 2, 1]), &known(&[2]), false));
        // The history is complete: the first known activity means everything older is known too.
        assert!(!keep_paging(&ids(&[3, 2, 1]), &known(&[2]), true));
        assert!(!keep_paging(&ids(&[3, 2, 1]), &known(&[1]), true));
        // Complete history, nothing on this page known yet: keep going until the overlap.
        assert!(keep_paging(&ids(&[3, 2, 1]), &known(&[9, 8]), true));
        assert!(keep_paging(&ids(&[3, 2, 1]), &known(&[]), true));
    }

    #[test]
    fn new_rule() {
        let newest = Utc.with_ymd_and_hms(2026, 10, 20, 7, 30, 0).unwrap();
        // The first import: nothing is new.
        assert!(!is_new(newest, None));
        assert!(is_new(newest + chrono::Duration::seconds(1), Some(newest)));
        assert!(!is_new(newest, Some(newest)));
        assert!(!is_new(newest - chrono::Duration::seconds(1), Some(newest)));
    }
}
