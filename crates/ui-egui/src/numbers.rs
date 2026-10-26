//! Numbers as the user reads them (activity import spec §2, item 17): `77,5 кг` in Russian and `77.5 kg` in English,
//! durations on a clock face (`1:39`, `28:41`) and distances in kilometres.
//!
//! Formatting only: the values come from the engine as they are stored, and nothing here rounds them for any purpose
//! but showing. The unit of weight is kilograms for now; the unit words go through `tl!`.

use crate::i18n::{self, Lang};
use crate::tl;

/// What stands for a number that is not one (NaN or an infinity): a dash, never `NaN`.
const NOT_A_NUMBER: &str = "—";

/// `value` with `decimals` digits after the separator of the interface language; with `trim`, the trailing zeros (and
/// a separator left alone) go.
fn decimal(value: f64, decimals: usize, trim: bool) -> String {
    if !value.is_finite() {
        return NOT_A_NUMBER.to_owned();
    }
    let mut text = format!("{value:.decimals$}");
    if trim && text.contains('.') {
        let kept = text.trim_end_matches('0').trim_end_matches('.').len();
        text.truncate(kept);
    }
    // Rounding a tiny negative (or -0.0) to zero leaves a sign nobody wants to read.
    if let Some(rest) = text.strip_prefix('-')
        && rest.chars().all(|c| c == '0' || c == '.')
    {
        text = rest.to_owned();
    }
    match i18n::current() {
        Lang::En => text,
        Lang::Ru => text.replace('.', ","),
    }
}

/// A weight: `77.5 kg`, `80 kg`, `1.25 kg`. Two decimals at most, which is finer than any plate.
pub(crate) fn kg(weight_kg: f64) -> String {
    tl!("{} kg").replace("{}", &kg_value(weight_kg))
}

/// The number of a weight without its unit (`77.5`), where the unit is plain from what stands next to it: the weight the
/// watch recorded, struck through after the corrected one.
pub(crate) fn kg_value(weight_kg: f64) -> String {
    decimal(weight_kg, 2, true)
}

/// A duration on a clock face: hours and minutes (`1:39`) from an hour up, minutes and seconds (`28:41`) below. Parts
/// of a second, and of a minute above an hour, are dropped, as a watch shows them.
pub(crate) fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
    if hours > 0 { format!("{hours}:{minutes:02}") } else { format!("{minutes}:{:02}", seconds % 60) }
}

/// A distance in kilometres with one decimal: `5.2 km`, `21.0 km`.
pub(crate) fn distance(m: f64) -> String {
    tl!("{} km").replace("{}", &decimal(m / 1000.0, 1, false))
}

#[cfg(test)]
#[path = "numbers_tests.rs"]
mod tests;
