use super::*;
use crate::i18n::{self, Lang};

#[test]
fn numbers_format() {
    i18n::set_current(Lang::Ru);
    assert_eq!(kg(77.5), "77,5 кг");
    assert_eq!(kg(80.0), "80 кг");
    i18n::set_current(Lang::En);
    assert_eq!(kg(77.5), "77.5 kg");
    assert_eq!(kg(80.0), "80 kg");
    assert_eq!(duration(5_940_528), "1:39");
    assert_eq!(duration(1_721_000), "28:41");
}

#[test]
fn weights_drop_trailing_zeros_but_keep_what_a_plate_adds() {
    i18n::set_current(Lang::En);
    assert_eq!(kg(1.25), "1.25 kg");
    assert_eq!(kg(22.5), "22.5 kg");
    assert_eq!(kg(0.0), "0 kg");
    // A weight entered in pounds and stored in grams: two decimals are plenty for a barbell.
    assert_eq!(kg(20.411_656), "20.41 kg");
    assert_eq!(kg(1000.0), "1000 kg");
    i18n::set_current(Lang::Ru);
    assert_eq!(kg(1.25), "1,25 кг");
    assert_eq!(kg(0.0), "0 кг");
}

#[test]
fn a_number_that_is_not_a_number_is_shown_as_a_dash() {
    // Never reached with what the engine sends (its weights are checked), but a NaN must not print as "NaN kg".
    i18n::set_current(Lang::En);
    assert_eq!(kg(f64::NAN), "— kg");
    assert_eq!(distance(f64::INFINITY), "— km");
    assert_eq!(kg(-0.0), "0 kg");
}

#[test]
fn durations_are_minutes_and_seconds_under_an_hour_and_hours_and_minutes_above() {
    assert_eq!(duration(0), "0:00");
    assert_eq!(duration(999), "0:00", "a part of a second is not rounded up");
    assert_eq!(duration(59_000), "0:59");
    assert_eq!(duration(60_000), "1:00");
    assert_eq!(duration(3_599_999), "59:59");
    assert_eq!(duration(3_600_000), "1:00");
    assert_eq!(duration(3_660_000), "1:01");
    assert_eq!(duration(36_000_000), "10:00");
    // The longest a u64 holds is far beyond a workout, and still no overflow.
    assert!(!duration(u64::MAX).is_empty());
}

#[test]
fn distances_have_one_decimal_in_kilometres() {
    i18n::set_current(Lang::Ru);
    assert_eq!(distance(5_200.0), "5,2 км");
    assert_eq!(distance(21_021.99), "21,0 км");
    i18n::set_current(Lang::En);
    assert_eq!(distance(5_087.53), "5.1 km");
    assert_eq!(distance(21_000.0), "21.0 km");
    assert_eq!(distance(450.0), "0.5 km");
}

#[test]
fn a_weight_without_its_unit_is_the_same_number() {
    // What the watch recorded, struck through after a correction: `77,5 кг` next to `80`.
    i18n::set_current(Lang::Ru);
    assert_eq!(kg_value(80.0), "80");
    assert_eq!(kg_value(77.5), "77,5");
    i18n::set_current(Lang::En);
    assert_eq!(kg_value(77.5), "77.5");
    assert_eq!(kg_value(1.25), "1.25");
    assert_eq!(kg_value(f64::NAN), "—");
}
