//! Translations: English in the source, Russian from a catalog embedded in the binary.
//!
//! Every text the user sees is written in English inside `tl!`, which looks it up in `i18n/ru.tsv` when the interface
//! is Russian and returns it unchanged otherwise. A test fails when a `tl!` literal has no Russian line. Command ids,
//! parameters, MCP texts and logs stay English and never go through here.
//!
//! The current language is kept per thread, not per process: the UI runs on one thread, and tests (which run in
//! parallel, each on its own thread) must not change each other's language.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::OnceLock;

use gymgar_domain::Language;

/// The catalog: one `English<TAB>Russian` pair per line, `#` starts a comment line.
const RU_TSV: &str = include_str!("../i18n/ru.tsv");

/// A language the interface is drawn in. [`Language::System`] is resolved to one of these by [`resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];
}

/// The language to draw in for the user's choice. `System` follows the language of `system_locale` (`ru` is Russian,
/// anything else, or no locale at all, is English).
pub fn resolve(pref: Language, system_locale: Option<&str>) -> Lang {
    match pref {
        Language::En => Lang::En,
        Language::Ru => Lang::Ru,
        Language::System => match system_locale.and_then(language_subtag) {
            Some(subtag) if subtag.eq_ignore_ascii_case("ru") => Lang::Ru,
            _ => Lang::En,
        },
    }
}

/// The language part of a locale as the platforms spell it: `ru-RU`, `ru_RU.UTF-8`, `ru@euro`, `ru`.
fn language_subtag(locale: &str) -> Option<&str> {
    locale.split(['-', '_', '.', '@']).next()
}

thread_local! {
    static CURRENT: Cell<Lang> = const { Cell::new(Lang::En) };
}

/// Makes `lang` the language of `t`, `tl!` and [`trn`] on this thread.
pub fn set_current(lang: Lang) {
    CURRENT.with(|current| current.set(lang));
}

/// The language of this thread; English until [`set_current`].
pub fn current() -> Lang {
    CURRENT.with(Cell::get)
}

/// What a catalog line is.
#[derive(Debug, PartialEq, Eq)]
enum Line<'a> {
    /// Blank or a comment.
    Skip,
    Entry(&'a str, &'a str),
    /// Not a pair: no tab, an empty side or more than one tab. A test fails on it; loading ignores it.
    Malformed,
}

fn parse_line(line: &str) -> Line<'_> {
    // A checkout on Windows may end the lines with CR LF.
    let line = line.trim_end_matches('\r');
    if line.trim().is_empty() || line.starts_with('#') {
        return Line::Skip;
    }
    match line.split_once('\t') {
        Some((english, russian)) if !english.is_empty() && !russian.is_empty() && !russian.contains('\t') => Line::Entry(english, russian),
        _ => Line::Malformed,
    }
}

fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static CATALOG: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        RU_TSV
            .lines()
            .filter_map(|line| match parse_line(line) {
                Line::Entry(english, russian) => Some((english, russian)),
                Line::Skip | Line::Malformed => None,
            })
            .collect()
    })
}

/// The text in the current language. A text the catalog does not have is shown in English, which beats a gap.
pub fn t(text: &'static str) -> &'static str {
    match current() {
        Lang::En => text,
        Lang::Ru => catalog().get(text).copied().unwrap_or(text),
    }
}

/// Translates an English literal: `tl!("Settings")`. It has to be a literal, so the test can find it.
#[macro_export]
macro_rules! tl {
    ($text:literal) => {
        $crate::i18n::t($text)
    };
}

/// The form of a counted noun for `n`, with every `{}` in it replaced by `n`: `trn(n, tl!("{} set"), tl!("{} sets"),
/// tl!("{} sets"))`. English has two forms (`n == 1` is `one`, everything else `many`; `few` is not used); Russian has
/// three: `one` for 1, 21, 101 and so on except 11, `few` for 2 to 4, 22 to 24 and so on except 12 to 14, `many`
/// for the rest.
pub fn trn(n: u64, one: &str, few: &str, many: &str) -> String {
    let form = match current() {
        Lang::En if n == 1 => one,
        Lang::En => many,
        Lang::Ru => match (n % 10, n % 100) {
            (1, last_two) if last_two != 11 => one,
            (2..=4, last_two) if !(12..=14).contains(&last_two) => few,
            _ => many,
        },
    };
    form.replace("{}", &n.to_string())
}

#[cfg(test)]
#[path = "i18n_tests.rs"]
mod tests;
