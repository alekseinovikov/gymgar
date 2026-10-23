use egui::Role;
use egui_kittest::kittest::Queryable;
use gymgar_domain::Language;
use gymgar_engine::testing::{TestConfig, test_session_on};
use gymgar_store::Store;
use serde_json::Value;

use crate::i18n::{self, Lang};
use crate::test_support::{app_in, harness_at, pick};
use crate::{GymgarApp, Section, UiState};

/// The three choices of the language list, as `lang` spells them. A language is named in itself, whatever the language
/// of the window: someone who reads only Russian finds «Русский» in an English window.
fn choices(lang: Lang) -> [&'static str; 3] {
    [pick(lang, "System", "Системный"), "English", "Русский"]
}

#[test]
fn settings_has_the_garmin_card_and_the_language_card() {
    for lang in Lang::ALL {
        let (app, _handles) = app_in(lang, TestConfig::default());
        let harness = harness_at(app, Section::Settings);
        // The title of the screen, besides the rail button of the same name.
        let title = pick(lang, "Settings", "Настройки");
        assert_eq!(harness.get_all_by_label(title).count(), 2, "{lang:?}: the rail button and the title");
        harness.get_by_label("Garmin Connect");
        harness.get_by_label(pick(lang, "Interface language", "Язык интерфейса"));
        // The language follows the system until the user chooses.
        assert_eq!(
            harness.get_by_role_and_label(Role::ComboBox, pick(lang, "System", "Системный")).value().as_deref(),
            Some(pick(lang, "System", "Системный"))
        );
    }
}

#[test]
fn the_language_list_offers_system_english_and_russian() {
    for lang in Lang::ALL {
        let (app, _handles) = app_in(lang, TestConfig::default());
        let mut harness = harness_at(app, Section::Settings);
        harness.get_by_role_and_label(Role::ComboBox, choices(lang)[0]).click();
        harness.run();
        for choice in choices(lang) {
            harness.get_by_role_and_label(Role::Button, choice);
        }
    }
}

#[test]
fn choosing_a_language_applies_at_once_and_is_kept_by_the_engine() {
    let (app, _handles) = app_in(Lang::En, TestConfig::default());
    let mut harness = harness_at(app, Section::Settings);

    harness.get_by_role_and_label(Role::ComboBox, "System").click();
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Русский").click();
    harness.run();
    assert_eq!(harness.state().language(), Language::Ru);
    assert_eq!(i18n::current(), Lang::Ru);
    // The whole window is in Russian on the very next frame, with no restart: titles, cards, the list itself.
    harness.get_by_role_and_label(Role::Button, "Настройки");
    harness.get_by_label("Язык интерфейса");
    harness.get_by_role_and_label(Role::ComboBox, "Русский");
    assert!(harness.query_by_label("Interface language").is_none());
    // And it is the engine that keeps the choice, so the CLI and agents see it too.
    let settings = harness.state_mut().run("settings.get", Value::Null).expect("read the settings");
    assert!(format!("{settings:?}").contains("ru"), "{settings:?}");

    harness.get_by_role_and_label(Role::ComboBox, "Русский").click();
    harness.run();
    harness.get_by_role_and_label(Role::Button, "English").click();
    harness.run();
    assert_eq!(harness.state().language(), Language::En);
    harness.get_by_label("Interface language");
    harness.get_by_role_and_label(Role::ComboBox, "English");
}

#[test]
fn system_follows_the_machine_and_is_what_the_list_shows() {
    // On a Russian machine "System" is Russian, and the list still says the user left it on "System".
    let (app, _handles) = app_in(Lang::Ru, TestConfig::default());
    let mut harness = harness_at(app, Section::Settings);
    harness.get_by_role_and_label(Role::ComboBox, "Системный");

    harness.get_by_role_and_label(Role::ComboBox, "Системный").click();
    harness.run();
    harness.get_by_role_and_label(Role::Button, "English").click();
    harness.run();
    harness.get_by_label("Interface language");

    harness.get_by_role_and_label(Role::ComboBox, "English").click();
    harness.run();
    harness.get_by_role_and_label(Role::Button, "System").click();
    harness.run();
    assert_eq!(harness.state().language(), Language::System);
    harness.get_by_label("Язык интерфейса");
}

#[test]
fn a_language_that_cannot_be_saved_is_reported_in_the_banner() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("gymgar.sqlite3");
    let (session, _handles) = test_session_on(Store::open(&path).expect("open the database"), TestConfig::default());
    let app = GymgarApp::with_system_locale(session, UiState::default(), Some("en-US".to_owned()));
    let mut harness = harness_at(app, Section::Settings);
    // The table of the settings disappears under the window.
    rusqlite::Connection::open(&path).expect("open the file").execute("DROP TABLE app_settings", []).expect("drop the table");

    harness.get_by_role_and_label(Role::ComboBox, "System").click();
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Русский").click();
    harness.run();
    harness.get_by_label("Gymgar could not access its data. Check that the disk is available and try again.");
    assert_eq!(harness.state().language(), Language::System, "the choice was not saved, so it is not in effect");
    harness.get_by_role_and_label(Role::ComboBox, "System");
}
