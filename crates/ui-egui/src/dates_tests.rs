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

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("a valid date")
}

#[test]
fn a_short_date_is_the_weekday_the_day_and_the_month() {
    i18n::set_current(Lang::En);
    assert_eq!(short_date(day(2026, 10, 23)), "Fri, 23 Oct");
    assert_eq!(short_date(day(2026, 5, 4)), "Mon, 4 May");
    i18n::set_current(Lang::Ru);
    assert_eq!(short_date(day(2026, 10, 23)), "Пт, 23 окт");
    assert_eq!(short_date(day(2026, 5, 4)), "Пн, 4 мая");
    // Every weekday and every month has its own short name.
    let week: Vec<String> = (19..=25).map(|d| short_date(day(2026, 10, d))).collect();
    assert_eq!(week, ["Пн, 19 окт", "Вт, 20 окт", "Ср, 21 окт", "Чт, 22 окт", "Пт, 23 окт", "Сб, 24 окт", "Вс, 25 окт"]);
    let months: Vec<String> = (1..=12).map(|m| short_date(day(2026, m, 1)).split(' ').next_back().unwrap_or_default().to_owned()).collect();
    assert_eq!(months, ["янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек"]);
    i18n::set_current(Lang::En);
    let months: Vec<String> = (1..=12).map(|m| short_date(day(2026, m, 1)).split(' ').next_back().unwrap_or_default().to_owned()).collect();
    assert_eq!(months, ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]);
}

#[test]
fn a_week_starts_on_monday() {
    assert_eq!(monday_of(day(2026, 10, 19)), day(2026, 10, 19), "a Monday is its own week's start");
    assert_eq!(monday_of(day(2026, 10, 23)), day(2026, 10, 19));
    assert_eq!(monday_of(day(2026, 10, 25)), day(2026, 10, 19), "Sunday ends the week");
    assert_eq!(monday_of(day(2026, 1, 1)), day(2025, 12, 29), "across a year");
    assert_eq!(monday_of(NaiveDate::MIN), NaiveDate::MIN, "the first day there is: no week before it to go back to");
}

#[test]
fn a_week_is_named_by_its_days() {
    i18n::set_current(Lang::En);
    assert_eq!(week(day(2026, 10, 12), 2026), "12–18 October");
    assert_eq!(week(day(2026, 9, 28), 2026), "28 September – 4 October");
    assert_eq!(week(day(2025, 10, 13), 2026), "13–19 October 2025", "another year says which");
    assert_eq!(week(day(2025, 12, 29), 2026), "29 December 2025 – 4 January 2026");
    i18n::set_current(Lang::Ru);
    assert_eq!(week(day(2026, 10, 12), 2026), "12–18 октября");
    assert_eq!(week(day(2026, 9, 28), 2026), "28 сентября – 4 октября");
    assert_eq!(week(day(2025, 12, 29), 2026), "29 декабря 2025 – 4 января 2026");
    // The last week of the calendar has no Sunday to name: its Monday stands for it.
    assert!(!week(NaiveDate::MAX, 2026).is_empty());
}

#[test]
fn a_long_date_names_the_day_of_the_week() {
    // The header of an open activity: `Четверг, 23 октября 2026`.
    let day = |d| NaiveDate::from_ymd_opt(2026, 10, d).expect("a valid date");
    i18n::set_current(Lang::En);
    assert_eq!(long_date(day(22)), "Thursday, 22 October 2026");
    assert_eq!(long_date(day(25)), "Sunday, 25 October 2026");
    i18n::set_current(Lang::Ru);
    assert_eq!(long_date(day(22)), "Четверг, 22 октября 2026");
    assert_eq!(long_date(day(19)), "Понедельник, 19 октября 2026");
    let week: Vec<String> = (19..=25).map(|d| long_date(day(d))).collect();
    assert_eq!(week.iter().collect::<std::collections::HashSet<_>>().len(), 7, "{week:?}");
}

#[test]
fn a_day_without_its_year_says_it_only_when_it_is_not_this_year() {
    i18n::set_current(Lang::En);
    assert_eq!(day_month(day(2026, 8, 18), 2026), "18 August");
    assert_eq!(day_month(day(2025, 12, 29), 2026), "29 December 2025");
    i18n::set_current(Lang::Ru);
    assert_eq!(day_month(day(2026, 8, 18), 2026), "18 августа");
    assert_eq!(day_month(day(2025, 12, 29), 2026), "29 декабря 2025");
}

#[test]
fn a_month_with_its_year_follows_the_since() {
    i18n::set_current(Lang::En);
    assert_eq!(month_year(day(2024, 3, 2)), "March 2024");
    i18n::set_current(Lang::Ru);
    assert_eq!(month_year(day(2024, 3, 2)), "марта 2024");
}

#[test]
fn two_days_share_their_month_when_they_have_one() {
    i18n::set_current(Lang::En);
    assert_eq!(two_days(day(2026, 8, 2), day(2026, 8, 18), 2026), "2 and 18 August");
    assert_eq!(two_days(day(2026, 7, 28), day(2026, 8, 18), 2026), "28 July and 18 August");
    assert_eq!(two_days(day(2025, 12, 29), day(2026, 1, 5), 2026), "29 December 2025 and 5 January");
    assert_eq!(two_days(day(2025, 8, 2), day(2025, 8, 18), 2026), "2 and 18 August 2025", "the year once, at the end");
    // The same month of two different years is not one month.
    assert_eq!(two_days(day(2025, 8, 2), day(2026, 8, 18), 2026), "2 August 2025 and 18 August");
    i18n::set_current(Lang::Ru);
    assert_eq!(two_days(day(2026, 8, 2), day(2026, 8, 18), 2026), "2 и 18 августа");
    assert_eq!(two_days(day(2026, 7, 28), day(2026, 8, 18), 2026), "28 июля и 18 августа");
}

#[test]
fn a_month_label_is_the_short_name_and_russian_may_stands_alone() {
    let labels = |lang| {
        i18n::set_current(lang);
        (1..=12).map(month_label).collect::<Vec<_>>()
    };
    assert_eq!(labels(Lang::En), ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]);
    // `май`, not the `мая` of a date.
    assert_eq!(labels(Lang::Ru), ["янв", "фев", "мар", "апр", "май", "июн", "июл", "авг", "сен", "окт", "ноя", "дек"]);
    assert_eq!(month_label(0), "");
    assert_eq!(month_label(13), "");
}
