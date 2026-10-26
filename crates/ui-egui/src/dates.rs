//! Dates and times as the user reads them: `23 October 2027` and `23 октября 2027`, never a machine format.
//!
//! The month names go through `tl!`, so they follow the interface language. They are only ever used inside a date, so
//! the Russian side of the catalog holds the genitive (`октября`), not the nominative. A date is written day, month,
//! year in both languages, and the time on a 24-hour clock, which no reader mistakes for another. The activity list
//! adds a short form (`Thu, 23 Oct`) and the name of a week (`13–19 October`); a week starts on Monday. The open
//! activity names its day in full (`Thursday, 23 October 2026`).

use chrono::{Datelike, Days, NaiveDate, NaiveDateTime, Timelike, Weekday};

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
    format!("{}, {}", date(at), time(at))
}

/// `12:04`.
pub(crate) fn time(at: NaiveDateTime) -> String {
    format!("{:02}:{:02}", at.hour(), at.minute())
}

/// The short name of month `month` (1 to 12) as it stands in a short date. Russian has `мая` for May, as English has
/// `May`, so the two share the line of the full name.
fn month_short(month: u32) -> &'static str {
    const NAMES: [fn() -> &'static str; 12] = [
        || tl!("Jan"),
        || tl!("Feb"),
        || tl!("Mar"),
        || tl!("Apr"),
        || tl!("May"),
        || tl!("Jun"),
        || tl!("Jul"),
        || tl!("Aug"),
        || tl!("Sep"),
        || tl!("Oct"),
        || tl!("Nov"),
        || tl!("Dec"),
    ];
    month.checked_sub(1).and_then(|index| NAMES.get(index as usize)).map_or("", |name| name())
}

/// The short name of a day of the week.
fn weekday_short(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => tl!("Mon"),
        Weekday::Tue => tl!("Tue"),
        Weekday::Wed => tl!("Wed"),
        Weekday::Thu => tl!("Thu"),
        Weekday::Fri => tl!("Fri"),
        Weekday::Sat => tl!("Sat"),
        Weekday::Sun => tl!("Sun"),
    }
}

/// `Thu, 23 Oct`: the day of an activity in the list, where the week's heading gives the year.
pub(crate) fn short_date(day: NaiveDate) -> String {
    format!("{}, {} {}", weekday_short(day.weekday()), day.day(), month_short(day.month()))
}

/// The name of a day of the week, as it begins a date (capitalised in both languages).
fn weekday_long(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => tl!("Monday"),
        Weekday::Tue => tl!("Tuesday"),
        Weekday::Wed => tl!("Wednesday"),
        Weekday::Thu => tl!("Thursday"),
        Weekday::Fri => tl!("Friday"),
        Weekday::Sat => tl!("Saturday"),
        Weekday::Sun => tl!("Sunday"),
    }
}

/// `Thursday, 23 October 2026`: the day of the activity that is open.
pub(crate) fn long_date(day: NaiveDate) -> String {
    format!("{}, {} {} {}", weekday_long(day.weekday()), day.day(), month_name(day.month()), day.year())
}

/// The Monday of the week `day` is in. The first day the calendar has stands for its own week.
pub(crate) fn monday_of(day: NaiveDate) -> NaiveDate {
    day.checked_sub_days(Days::new(u64::from(day.weekday().num_days_from_monday()))).unwrap_or(day)
}

/// The name of the week that starts on `monday`: `13–19 October`, or `29 September – 5 October` across two months. A
/// week of another year than `this_year` says which (`13–19 October 2025`), and one across two years says both.
pub(crate) fn week(monday: NaiveDate, this_year: i32) -> String {
    // The last week of the calendar has no Sunday: its Monday stands for the whole week.
    let sunday = monday.checked_add_days(Days::new(6)).unwrap_or(monday);
    let day_month = |day: NaiveDate| format!("{} {}", day.day(), month_name(day.month()));
    if monday.year() != sunday.year() {
        return format!("{} {} – {} {}", day_month(monday), monday.year(), day_month(sunday), sunday.year());
    }
    let days = if monday.month() == sunday.month() {
        format!("{}–{} {}", monday.day(), sunday.day(), month_name(sunday.month()))
    } else {
        format!("{} – {}", day_month(monday), day_month(sunday))
    };
    if sunday.year() == this_year { days } else { format!("{days} {}", sunday.year()) }
}

#[cfg(test)]
#[path = "dates_tests.rs"]
mod tests;
