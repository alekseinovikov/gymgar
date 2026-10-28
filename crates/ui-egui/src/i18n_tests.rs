use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gymgar_domain::Language;

use super::*;

#[test]
fn i18n_resolve() {
    assert_eq!(resolve(Language::System, Some("ru-RU")), Lang::Ru);
    assert_eq!(resolve(Language::System, Some("de-DE")), Lang::En);
    assert_eq!(resolve(Language::Ru, Some("en-US")), Lang::Ru);
    assert_eq!(resolve(Language::System, None), Lang::En);
}

#[test]
fn resolve_follows_the_choice_and_reads_the_language_subtag_only() {
    assert_eq!(resolve(Language::En, Some("ru-RU")), Lang::En);
    assert_eq!(resolve(Language::En, None), Lang::En);
    assert_eq!(resolve(Language::Ru, None), Lang::Ru);
    // The platforms spell locales differently: `ru_RU.UTF-8`, `ru`, `RU-ru`.
    for locale in ["ru", "ru_RU", "ru_RU.UTF-8", "ru-UA", "RU-ru", "ru@euro"] {
        assert_eq!(resolve(Language::System, Some(locale)), Lang::Ru, "{locale}");
    }
    // Only the language counts: another language that merely starts with `ru`, or `ru` as a region, is not Russian.
    for locale in ["rue-UA", "run", "en-RU", "", "-ru", "uk-UA", "be-BY"] {
        assert_eq!(resolve(Language::System, Some(locale)), Lang::En, "{locale}");
    }
}

#[test]
fn i18n_plural_ru() {
    set_current(Lang::Ru);
    let ru = |n| trn(n, "one", "few", "many");
    assert_eq!(ru(1), "one");
    assert_eq!(ru(3), "few");
    assert_eq!(ru(5), "many");
    assert_eq!(ru(11), "many");
    assert_eq!(ru(21), "one");
    assert_eq!(ru(22), "few");
    // The teens are always `many`, even though they end in 1 to 4.
    for n in [0, 11, 12, 13, 14, 111, 112, 113, 114, 25, 100] {
        assert_eq!(ru(n), "many", "{n}");
    }
    for n in [1, 21, 101, 1001] {
        assert_eq!(ru(n), "one", "{n}");
    }
    for n in [2, 4, 22, 24, 102, 1004] {
        assert_eq!(ru(n), "few", "{n}");
    }
    assert_eq!(ru(u64::MAX), "many");
}

#[test]
fn plurals_in_english_have_one_and_many() {
    set_current(Lang::En);
    let en = |n| trn(n, "one", "few", "many");
    assert_eq!(en(1), "one");
    for n in [0, 2, 3, 5, 11, 21, 22] {
        assert_eq!(en(n), "many", "{n}");
    }
}

#[test]
fn plural_forms_get_their_number() {
    set_current(Lang::Ru);
    assert_eq!(trn(1, "{} подход", "{} подхода", "{} подходов"), "1 подход");
    assert_eq!(trn(3, "{} подход", "{} подхода", "{} подходов"), "3 подхода");
    assert_eq!(trn(5, "{} подход", "{} подхода", "{} подходов"), "5 подходов");
    set_current(Lang::En);
    assert_eq!(trn(1, "{} set", "{} sets", "{} sets"), "1 set");
    assert_eq!(trn(3, "{} set", "{} sets", "{} sets"), "3 sets");
}

#[test]
fn t_translates_for_russian_and_keeps_english_as_is() {
    set_current(Lang::En);
    assert_eq!(t("Summary"), "Summary");
    assert_eq!(tl!("Settings"), "Settings");
    set_current(Lang::Ru);
    assert_eq!(t("Summary"), "Сводка");
    assert_eq!(tl!("Settings"), "Настройки");
}

#[test]
fn a_context_tells_apart_two_uses_of_one_text_and_english_never_shows_it() {
    set_current(Lang::En);
    assert_eq!(t("May"), "May");
    assert_eq!(t("May|label"), "May", "English shows the text without its context");
    set_current(Lang::Ru);
    assert_eq!(t("May"), "мая", "the month inside a date");
    assert_eq!(t("May|label"), "май", "the month alone");
    // A context with no line says the text, not the context.
    assert_eq!(t("Nothing here|in no catalog"), "Nothing here");
}

#[test]
fn a_text_without_a_translation_stays_english() {
    set_current(Lang::Ru);
    assert_eq!(t("This text is in no catalog"), "This text is in no catalog");
}

#[test]
fn the_language_is_per_thread() {
    // Tests (and screens) run in parallel on different threads; one thread's choice must not leak into another.
    set_current(Lang::Ru);
    let other = std::thread::spawn(|| (current(), t("Summary"))).join().expect("the thread finishes");
    assert_eq!(other, (Lang::En, "Summary"));
    assert_eq!((current(), t("Summary")), (Lang::Ru, "Сводка"));
}

#[test]
fn every_catalog_line_is_a_comment_or_one_english_tab_russian_pair() {
    let mut keys = HashSet::new();
    for (number, line) in RU_TSV.lines().enumerate() {
        match parse_line(line) {
            Line::Skip => {}
            Line::Entry(english, russian) => {
                assert!(keys.insert(english), "line {}: `{english}` is translated twice", number + 1);
                assert_eq!(
                    english.matches("{}").count(),
                    russian.matches("{}").count(),
                    "line {}: the placeholders of `{english}` and its translation differ",
                    number + 1
                );
            }
            Line::Malformed => panic!("line {}: expected `English<TAB>Russian`, got `{line}`", number + 1),
        }
    }
    assert!(!keys.is_empty(), "the catalog is empty");
}

#[test]
fn the_catalog_has_no_duplicate_keys_once_loaded() {
    let entries = RU_TSV.lines().filter(|line| matches!(parse_line(line), Line::Entry(..))).count();
    assert_eq!(catalog().len(), entries, "a duplicated key would shadow an earlier translation");
}

#[test]
fn line_parsing() {
    assert!(matches!(parse_line(""), Line::Skip));
    assert!(matches!(parse_line("   "), Line::Skip));
    assert!(matches!(parse_line("# a comment"), Line::Skip));
    assert!(matches!(parse_line("Summary\tСводка"), Line::Entry("Summary", "Сводка")));
    // A Windows checkout may turn the line ends into CR LF.
    assert!(matches!(parse_line("Summary\tСводка\r"), Line::Entry("Summary", "Сводка")));
    assert!(matches!(parse_line("no tab here"), Line::Malformed));
    assert!(matches!(parse_line("\tonly a translation"), Line::Malformed));
    assert!(matches!(parse_line("only an original\t"), Line::Malformed));
    assert!(matches!(parse_line("a\tb\tc"), Line::Malformed));
}

/// The name of the macro whose literals must have a translation.
const MACRO: &str = "tl!";

/// The `tl!` literals of `source`, as the strings they stand for.
///
/// The whole text is scanned, not line by line, because rustfmt moves a long literal onto a line of its own between
/// the brackets. Any whitespace may sit between the macro name, the bracket and the literal; ordinary and raw
/// strings are read. Comment lines are skipped, so documentation can show the macro without a translation. What is
/// not a string literal after `tl!` is skipped too: the macro takes nothing else, so the compiler refuses it.
fn tl_literals(source: &str) -> Vec<String> {
    let code = source.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = code[from..].find(MACRO) {
        let start = from + offset;
        from = start + MACRO.len();
        // `html!` and `my_tl!` are other macros.
        if code[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let Some(rest) = code[from..].trim_start().strip_prefix(['(', '[', '{']) else { continue };
        if let Some(literal) = string_literal(rest.trim_start()) {
            found.push(literal);
        }
    }
    found
}

/// The string literal `text` starts with, if it starts with one. Panics when the literal never ends.
fn string_literal(text: &str) -> Option<String> {
    if let Some(raw) = text.strip_prefix('r') {
        let hashes = raw.chars().take_while(|&c| c == '#').count();
        let body = raw[hashes..].strip_prefix('"')?;
        let end = body.find(&format!("\"{}", "#".repeat(hashes))).expect("a raw tl! literal must be closed");
        return Some(body[..end].to_owned());
    }
    let mut chars = text.strip_prefix('"')?.chars().peekable();
    let mut literal = String::new();
    loop {
        match chars.next().expect("a tl! literal must be closed") {
            '"' => return Some(literal),
            '\\' => match chars.next().expect("a tl! literal must be closed") {
                '"' => literal.push('"'),
                '\\' => literal.push('\\'),
                '\'' => literal.push('\''),
                'n' => literal.push('\n'),
                't' => literal.push('\t'),
                'r' => literal.push('\r'),
                '0' => literal.push('\0'),
                'u' if chars.next_if_eq(&'{').is_some() => {
                    let digits: String = std::iter::from_fn(|| chars.next_if(|c| *c != '}')).collect();
                    chars.next();
                    let escaped = u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32);
                    literal.push(escaped.expect("a valid unicode escape in a tl! literal"));
                }
                // A backslash at the end of a line continues the literal and drops the next line's indent.
                '\n' => while chars.next_if(|c| c.is_whitespace()).is_some() {},
                '\r' => while chars.next_if(|c| c.is_whitespace()).is_some() {},
                // Any other escape is kept as written, so it fails the translation check loudly.
                other => {
                    literal.push('\\');
                    literal.push(other);
                }
            },
            other => literal.push(other),
        }
    }
}

fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read a source directory") {
        let path = entry.expect("read a directory entry").path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}

#[test]
fn the_scanner_reads_literals() {
    let source = "let a = tl!(\"One\"); let b = tl!(\"Two \\\"quoted\\\" and back\\\\slash\");\n// tl!(\"A comment\")\nx(tl!(\"Three\"), tl!(\"Four\"))";
    assert_eq!(tl_literals(source), ["One", "Two \"quoted\" and back\\slash", "Three", "Four"]);
}

#[test]
fn the_scanner_reads_the_form_rustfmt_makes_of_a_long_call() {
    // rustfmt moves a long literal onto its own line, between the parentheses.
    let source = "label(\n    tl!(\n        \"A long text that rustfmt moved to its own line\"\n    ),\n);";
    assert_eq!(tl_literals(source), ["A long text that rustfmt moved to its own line"]);
    // Any spacing is fine, and so is any of the three kinds of brackets.
    let source = "tl! ( \"Spaced\" ) tl![\"Square\"] tl!{\"Curly\"} tl!(\t\"Tab\"\r\n)";
    assert_eq!(tl_literals(source), ["Spaced", "Square", "Curly", "Tab"]);
}

#[test]
fn the_scanner_reads_raw_strings() {
    let source = "tl!(r\"Raw text\"); tl!(r#\"It said \"hi\" and left\"#); tl!(\n    r##\"A \"# inside\"##\n)";
    assert_eq!(tl_literals(source), ["Raw text", "It said \"hi\" and left", "A \"# inside"]);
}

#[test]
fn the_scanner_reads_escapes() {
    // An escaped quote and backslash, a unicode escape, and a line continuation (which drops the next line's indent).
    let source = "tl!(\"Say \\\"hi\\\" now\") tl!(\"back\\\\slash\") tl!(\"Don\\u{2019}t\") tl!(\"One \\\n        two\")";
    assert_eq!(tl_literals(source), ["Say \"hi\" now", "back\\slash", "Don\u{2019}t", "One two"]);
}

#[test]
fn the_scanner_skips_what_is_not_a_tl_call() {
    // Comment lines, other macros whose name ends in `tl`, and a `tl!` that is not given a literal.
    let source = "// tl!(\"In a comment\")\n    /// tl!(\"In a doc comment\")\nhtml!(\"Not ours\"); my_tl!(\"Neither\"); tl!(some_name); tl!(r); tl!(\"Ours\")";
    assert_eq!(tl_literals(source), ["Ours"]);
}

#[test]
#[should_panic(expected = "must be closed")]
fn the_scanner_refuses_a_literal_that_never_ends() {
    tl_literals("tl!(\"Never closed");
}

#[test]
fn i18n_every_tl_literal_is_translated() {
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let catalog = catalog();
    let mut seen = 0;
    let mut missing = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).expect("read a source file");
        for literal in tl_literals(&source) {
            seen += 1;
            if !catalog.contains_key(literal.as_str()) {
                missing.push(format!("{}: `{literal}`", file.display()));
            }
        }
    }
    assert!(seen > 0, "no tl! literal found: the scanner is broken");
    assert!(missing.is_empty(), "tl! literals with no translation in i18n/ru.tsv:\n{}", missing.join("\n"));
}
