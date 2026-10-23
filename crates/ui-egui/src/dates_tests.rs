use chrono::{NaiveDate, NaiveDateTime};

use super::*;
use crate::i18n::{self, Lang};

fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day).and_then(|date| date.and_hms_opt(hour, minute, 0)).expect("a valid date")
}

#[test]
fn a_date_is_day_month_in_words_and_year() {
    i18n::set_current(Lang::En);
    assert_eq!(date(at(2027, 10, 23, 12, 4)), "23 October 2027");
    assert_eq!(date(at(2026, 1, 5, 0, 0)), "5 January 2026", "no zero in front of the day");
    i18n::set_current(Lang::Ru);
    assert_eq!(date(at(2027, 10, 23, 12, 4)), "23 октября 2027");
    assert_eq!(date(at(2026, 1, 5, 0, 0)), "5 января 2026");
}

#[test]
fn a_time_follows_the_date_on_a_24_hour_clock_with_two_digit_minutes() {
    i18n::set_current(Lang::En);
    assert_eq!(date_time(at(2026, 10, 23, 12, 4)), "23 October 2026, 12:04");
    assert_eq!(date_time(at(2026, 10, 23, 9, 5)), "23 October 2026, 09:05");
    assert_eq!(date_time(at(2026, 10, 23, 23, 59)), "23 October 2026, 23:59");
    i18n::set_current(Lang::Ru);
    assert_eq!(date_time(at(2026, 10, 23, 0, 0)), "23 октября 2026, 00:00");
}

#[test]
fn every_month_has_its_own_name_in_both_languages() {
    let names = |lang| {
        i18n::set_current(lang);
        (1..=12).map(|month| date(at(2026, month, 1, 0, 0))).collect::<Vec<_>>()
    };
    let english = names(Lang::En);
    let russian = names(Lang::Ru);
    assert_eq!(english.iter().collect::<std::collections::HashSet<_>>().len(), 12, "{english:?}");
    assert_eq!(russian.iter().collect::<std::collections::HashSet<_>>().len(), 12, "{russian:?}");
    assert_eq!(
        russian,
        [
            "1 января 2026",
            "1 февраля 2026",
            "1 марта 2026",
            "1 апреля 2026",
            "1 мая 2026",
            "1 июня 2026",
            "1 июля 2026",
            "1 августа 2026",
            "1 сентября 2026",
            "1 октября 2026",
            "1 ноября 2026",
            "1 декабря 2026"
        ]
    );
    assert_eq!(english.first().map(String::as_str), Some("1 January 2026"));
    assert_eq!(english.last().map(String::as_str), Some("1 December 2026"));
}
