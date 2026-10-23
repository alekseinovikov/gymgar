use std::path::Path;

use egui_kittest::kittest::Queryable;

use super::*;
use crate::i18n::Lang;
use crate::test_support::{pick, ui_harness};

const ERROR: &str = "cannot open the database at /home/user/.local/share/gymgar/gymgar.sqlite3: unable to open database file";
const FILE: &str = "/home/user/.local/share/gymgar/gymgar.sqlite3";

fn locale(lang: Lang) -> Option<String> {
    Some(pick(lang, "en-US", "ru-RU").to_owned())
}

#[test]
fn a_store_error_is_shown_with_its_text_and_the_file_in_the_language_of_the_system() {
    for lang in Lang::ALL {
        let app = StartupErrorApp::with_system_locale(ERROR, Some(Path::new(FILE)), locale(lang));
        let harness = ui_harness(|ui, app: &mut StartupErrorApp| app.show(ui), app);
        harness.get_by_label(pick(lang, "Gymgar cannot start", "Gymgar не может запуститься"));
        harness.get_by_label(pick(
            lang,
            "Gymgar could not open its data. Check that the disk is available and the file is not damaged, then start Gymgar again.",
            "Gymgar не смог открыть свои данные. Проверьте, что диск доступен, а файл не повреждён, и запустите Gymgar снова.",
        ));
        // The error is the store's own text, as it is: it is what to quote when asking for help.
        harness.get_by_label(pick(lang, "Details", "Подробности"));
        harness.get_by_label(ERROR);
        harness.get_by_label(pick(lang, "File", "Файл"));
        harness.get_by_label(FILE);
    }
}

#[test]
fn an_error_without_a_file_has_no_file_line() {
    for lang in Lang::ALL {
        let app = StartupErrorApp::with_system_locale("cannot start the async runtime: out of threads", None, locale(lang));
        let harness = ui_harness(|ui, app: &mut StartupErrorApp| app.show(ui), app);
        harness.get_by_label(pick(lang, "Gymgar cannot start", "Gymgar не может запуститься"));
        harness.get_by_label(pick(
            lang,
            "Start Gymgar again. If the problem remains, report the error below.",
            "Запустите Gymgar снова. Если проблема останется, сообщите об ошибке ниже.",
        ));
        harness.get_by_label("cannot start the async runtime: out of threads");
        assert!(harness.query_by_label(pick(lang, "File", "Файл")).is_none(), "{lang:?}");
    }
}

#[test]
fn an_unknown_system_language_is_english() {
    for system in [None, Some("de-DE".to_owned())] {
        let app = StartupErrorApp::with_system_locale(ERROR, Some(Path::new(FILE)), system);
        let harness = ui_harness(|ui, app: &mut StartupErrorApp| app.show(ui), app);
        harness.get_by_label("Gymgar cannot start");
    }
}

#[test]
fn a_very_long_error_does_not_break_the_screen() {
    let error = "x".repeat(20_000);
    let app = StartupErrorApp::with_system_locale(&error, Some(Path::new(FILE)), None);
    let mut harness = ui_harness(|ui, app: &mut StartupErrorApp| app.show(ui), app);
    harness.run();
    harness.get_by_label("Gymgar cannot start");
}

#[test]
fn the_screen_fits_a_window_narrower_than_the_widest_content() {
    // The error window opens at 720 points; the content is allowed to be 760 wide on the main screens.
    let width = 520.0;
    let app = StartupErrorApp::with_system_locale(ERROR, Some(Path::new(FILE)), None);
    let harness = crate::test_support::builder().with_size(egui::vec2(width, 400.0)).build_ui_state(|ui, app: &mut StartupErrorApp| app.show(ui), app);
    for label in [ERROR, FILE, "Details"] {
        let right = harness.get_by_label(label).rect().right();
        assert!(right <= width, "`{label}` ends at {right}, past the window's edge at {width}");
    }
    // The card around them too: its text would be cut off by the edge otherwise.
    let card_right = harness.get_by_label(ERROR).rect().right();
    assert!(card_right <= width - 24.0 + 1.0, "the text runs into the margin: {card_right}");
}
