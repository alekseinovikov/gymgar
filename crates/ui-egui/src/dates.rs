//! Dates and times as the user reads them: `23 October 2027` and `23 октября 2027`, never a machine format.
//!
//! The month names go through `tl!`, so they follow the interface language. They are only ever used inside a date, so
//! the Russian side of the catalog holds the genitive (`октября`), not the nominative. A date is written day, month,
//! year in both languages, and the time on a 24-hour clock, which no reader mistakes for another.

use chrono::{Datelike, NaiveDateTime, Timelike};

use crate::tl;

/// The name of month `month` (1 to 12) as it stands in a date. The Russian side is the genitive.
fn month_name(month: u32) -> &'static str {
    // Each name is a literal inside its own `tl!`, so that the test that every literal is translated sees it.
    const NAMES: [fn() -> &'static str; 12] = [
        || tl!("January"),
        || tl!("February"),
        || tl!("March"),
        || tl!("April"),
        || tl!("May"),
        || tl!("June"),
        || tl!("July"),
        || tl!("August"),
        || tl!("September"),
        || tl!("October"),
        || tl!("November"),
        || tl!("December"),
    ];
    month.checked_sub(1).and_then(|index| NAMES.get(index as usize)).map_or("", |name| name())
}

/// `23 October 2027`: the day, the month in words and the year.
pub(crate) fn date(at: NaiveDateTime) -> String {
    format!("{} {} {}", at.day(), month_name(at.month()), at.year())
}

/// `23 October 2027, 12:04`.
pub(crate) fn date_time(at: NaiveDateTime) -> String {
    format!("{}, {:02}:{:02}", date(at), at.hour(), at.minute())
}

#[cfg(test)]
#[path = "dates_tests.rs"]
mod tests;
