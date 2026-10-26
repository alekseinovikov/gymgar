//! A page of Garmin's activity list in domain terms.
//!
//! [`parse_page`] turns each element into an [`ActivitySummary`] and keeps the element's own JSON beside it, so that statistics not
//! computed today can be computed later without asking Garmin again. The list is hostile input: one odd element is skipped and
//! the rest of the page is kept, because failing the page would stall the import at that offset for good. The page also says how
//! many elements Garmin sent, skipped ones included: only an empty array is the end of the list, and the next page starts after
//! every element of this one. Log lines name an activity by its id only; names are the user's text.

use std::ops::RangeInclusive;

use chrono::{Datelike, NaiveDateTime};
use gymgar_domain::{ActivityId, ActivitySummary};
use gymgar_garmin::personal::ActivityListItem;
use serde_json::Value;

use crate::SyncError;

/// How Garmin writes `startTimeGMT`: UTC, no zone marker.
const START_FORMAT: &str = "%Y-%m-%d %H:%M:%S";
/// The years a start may have. Chrono reads `+10000` and `-0001` too, but a stored time has a four-digit year, and the
/// store refuses a whole page for one start outside these.
const START_YEARS: RangeInclusive<i32> = 0..=9999;
/// The longest duration kept, in seconds (136 years). Anything longer is a broken value, and a bound keeps the milliseconds
/// well inside every integer type the engine stores them in.
const MAX_DURATION_S: f64 = u32::MAX as f64;

/// One element of the list: what the app shows, and the element as Garmin sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedActivity {
    pub summary: ActivitySummary,
    /// The element's own JSON value, every field included (`owner*` too). It is for storage only and never goes into a command or MCP answer.
    pub raw_json: String,
}

/// One page of the list: the activities that could be read, and how many elements Garmin sent.
#[derive(Debug, Clone, PartialEq)]
pub struct ListPage {
    /// In Garmin's order (newest first), without the elements that were skipped.
    pub items: Vec<ListedActivity>,
    /// Every element of the array, skipped ones included. Zero, and only zero, is the end of the list.
    pub raw_count: usize,
}

/// Reads the body of one list request: a JSON array, in Garmin's order (newest first).
///
/// A body that is not an array is an error. An element that is not an activity, or has no readable start time, is skipped with a log
/// line: it must not take the rest of the page with it.
pub fn parse_page(body: &[u8]) -> Result<ListPage, SyncError> {
    // Not `error.to_string()`: serde_json quotes the offending value, and that is the user's data.
    let elements: Vec<Value> = serde_json::from_slice(body).map_err(|_| SyncError::Garmin("the activity list is not a JSON array".to_owned()))?;
    let raw_count = elements.len();
    Ok(ListPage { items: elements.into_iter().filter_map(listed).collect(), raw_count })
}

/// One element, or `None` (logged) when it cannot be used.
fn listed(element: Value) -> Option<ListedActivity> {
    let id = element.get("activityId").and_then(Value::as_i64);
    let Ok(item) = serde_json::from_value::<ActivityListItem>(element.clone()) else {
        log_skipped(id, "it is not an activity");
        return None;
    };
    let Some(start) = item.start_time_gmt.as_deref().and_then(|text| NaiveDateTime::parse_from_str(text, START_FORMAT).ok()).map(|start| start.and_utc())
    else {
        log_skipped(id, "its start time cannot be read");
        return None;
    };
    if !START_YEARS.contains(&start.year()) {
        log_skipped(id, "its start time is out of range");
        return None;
    }
    // The element's own JSON, not the typed item written out again: that would turn `null` fields into absent ones and floats into
    // whatever they print as, and the stored copy is meant to be what Garmin sent.
    let Ok(raw_json) = serde_json::to_string(&element) else {
        log_skipped(id, "it cannot be written out");
        return None;
    };
    let manual = item.manual_activity == Some(true) || item.extra.get("isManualActivity").and_then(Value::as_bool) == Some(true);
    let summary = ActivitySummary {
        id: ActivityId(item.activity_id),
        type_key: item.activity_type.type_key,
        // A blank name is no name: the UI shows the type and date instead of an empty row.
        name: item.activity_name.filter(|name| !name.trim().is_empty()),
        start,
        duration_ms: in_range(item.duration, MAX_DURATION_S).map(|seconds| (seconds * 1000.0).round() as u64),
        distance_m: item.distance.filter(|meters| meters.is_finite() && *meters >= 0.0),
        avg_hr: in_range(item.average_hr, f64::from(u16::MAX)).map(|bpm| bpm.round() as u16),
        calories: in_range(item.calories, f64::from(u32::MAX)).map(|kcal| kcal.round() as u32),
        manual,
    };
    Some(ListedActivity { summary, raw_json })
}

/// `value` if it is a number from 0 up to `max` once rounded; negative, NaN, infinite and too large values are `None`.
fn in_range(value: Option<f64>, max: f64) -> Option<f64> {
    // `-0.4` rounds to `-0.0`, which is in range and casts to 0; NaN is in no range.
    value.filter(|number| (0.0..=max).contains(&number.round()))
}

fn log_skipped(id: Option<i64>, reason: &str) {
    match id {
        Some(id) => log::warn!("skipping activity {id} of the list: {reason}"),
        None => log::warn!("skipping an element of the activity list: {reason}"),
    }
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
