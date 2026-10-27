use chrono::TimeDelta;
use egui::{OutputCommand, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use gymgar_domain::{ConnectionStatus, GarminConnection, KeyTerm, SecretBackend};
use gymgar_engine::testing::{TestConfig, TestHandles, connection_at, stored_key, test_session_on};
use gymgar_engine::{GarminAuthError, SecretStore};
use gymgar_store::Store;
use secrecy::SecretString;
use serde_json::json;

use super::*;
use crate::i18n::{self, Lang};
use crate::test_support::{
    app_connected, app_in, app_with_key, assert_not_exposed, button_enabled, click, harness_at, pick, tick, type_key, working_connection,
};
use crate::{GymgarApp, Section, UiState};

/// A real key is 132 characters long, longer than the 128 some inputs cut at.
fn key() -> String {
    let tail: String = ('a'..='z').cycle().take(129).collect();
    format!("zq-{tail}")
}

fn settings(lang: Lang, cfg: TestConfig) -> (Harness<'static, GymgarApp>, TestHandles) {
    let (app, handles) = app_in(lang, cfg);
    (harness_at(app, Section::Settings), handles)
}

fn settings_connected(lang: Lang, cfg: &TestConfig, connection: &GarminConnection) -> (Harness<'static, GymgarApp>, TestHandles) {
    let (app, handles) = app_connected(lang, cfg.clone(), connection);
    (harness_at(app, Section::Settings), handles)
}

fn status_of(harness: &Harness<'_, GymgarApp>) -> ConnectionStatus {
    harness.state().status().expect("the status was read").status
}

fn with_status(cfg: &TestConfig, status: ConnectionStatus) -> GarminConnection {
    GarminConnection { last_status: status, ..working_connection(cfg) }
}

/// The word on the button that connects, in `lang`.
fn connect_label(lang: Lang) -> &'static str {
    pick(lang, "Connect", "Подключить")
}

/// The texts of spec section 2.3, for each way a key check can fail.
fn failure_texts() -> [(GarminAuthError, &'static str, &'static str); 5] {
    let unavailable = ("Garmin is unavailable right now. Try again later.", "Garmin сейчас недоступен. Попробуйте позже.");
    [
        (
            GarminAuthError::KeyRejected,
            "Garmin did not accept the key. Check that it was copied in full, or create a new one.",
            "Garmin не принял ключ. Проверьте, что он скопирован целиком, или создайте новый.",
        ),
        (
            GarminAuthError::Network,
            "Could not reach Garmin. Check your internet connection.",
            "Не удалось связаться с Garmin. Проверьте подключение к интернету.",
        ),
        (
            GarminAuthError::RateLimited,
            "Garmin has temporarily limited requests. Try again in a few minutes.",
            "Garmin временно ограничил запросы. Попробуйте через несколько минут.",
        ),
        (GarminAuthError::Unavailable, unavailable.0, unavailable.1),
        (GarminAuthError::Unexpected, unavailable.0, unavailable.1),
    ]
}

#[test]
fn the_card_gives_the_four_steps_and_the_key_form() {
    for lang in Lang::ALL {
        let (harness, _handles) = settings(lang, TestConfig::default());
        harness.get_by_label(pick(lang, "Garmin Connect", "Garmin Connect"));
        harness.get_by_role_and_label(
            Role::Link,
            pick(
                lang,
                "Open the Personal API Key page in Garmin Connect and sign in.",
                "Открыть страницу Personal API Key в Garmin Connect и войти в аккаунт.",
            ),
        );
        harness.get_by_label(pick(lang, "Click Create Key and accept the terms.", "Нажать Create Key и принять условия."));
        harness.get_by_label(pick(lang, "Choose how long the key is valid; 1 year is the most convenient.", "Выбрать срок действия; удобнее всего 1 год."));
        harness.get_by_label(pick(
            lang,
            "Click Create and copy the key right away: Garmin shows it only once.",
            "Нажать Create и сразу скопировать ключ: Garmin показывает его один раз.",
        ));
        for number in ["1.", "2.", "3.", "4."] {
            harness.get_by_label(number);
        }
        // The key is hidden and has its label; the term defaults to a year, with the hint of the spec.
        assert_eq!(harness.get_by_label(pick(lang, "Key", "Ключ")).accesskit_node().role(), Role::PasswordInput, "{lang:?}");
        harness.get_by_label(pick(lang, "Key term", "Срок ключа"));
        harness.get_by_role_and_label(Role::ComboBox, pick(lang, "1 year", "1 год"));
        harness.get_by_label(pick(
            lang,
            "as chosen on the website; we will warn you a week before it ends",
            "как выбрали на сайте; предупредим за неделю до конца",
        ));
        harness.get_by_role_and_label(Role::Button, connect_label(lang));
        // Nothing is connected yet: no status, no buttons of a connection.
        assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Disconnect", "Отключить")).is_none(), "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "Connected", "Подключено")).is_none(), "{lang:?}");
    }
}

#[test]
fn the_first_step_opens_the_key_page_in_the_browser() {
    let (mut harness, _handles) = settings(Lang::En, TestConfig::default());
    harness.get_by_role_and_label(Role::Link, "Open the Personal API Key page in Garmin Connect and sign in.").click();
    harness.step();
    let opened: Vec<_> = harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            OutputCommand::OpenUrl(open) => Some((open.url.as_str(), open.new_tab)),
            _ => None,
        })
        .collect();
    assert_eq!(opened, [("https://connect.garmin.com/app/settings/personalApiToken", true)]);
    assert_eq!(KEY_PAGE_URL, "https://connect.garmin.com/app/settings/personalApiToken");
}

#[test]
fn connect_flow_success() {
    for lang in Lang::ALL {
        let (mut harness, handles) = settings(lang, TestConfig::default());
        let key = key();
        type_key(&mut harness, &key);
        click(&mut harness, connect_label(lang));
        tick(&mut harness);
        harness.run();

        harness.get_by_label(pick(lang, "Connected", "Подключено"));
        assert_eq!(status_of(&harness), ConnectionStatus::Connected, "{lang:?}");
        // The form is gone and the draft with it; the key went to Garmin whole and into the system store.
        assert!(harness.query_by_role(Role::PasswordInput).is_none(), "{lang:?}");
        assert_eq!(harness.state().ui.connect.key, "", "{lang:?}");
        assert_eq!(handles.garmin.last_key().as_deref(), Some(key.as_str()), "{lang:?}: the whole key reached Garmin");
        assert_eq!(stored_key(handles.system.as_deref().expect("a system store")).as_deref(), Some(key.as_str()), "{lang:?}");
        // The key is in no node of the accessibility tree: not while typed, not after.
        assert_not_exposed(&harness, &key);
    }
}

#[test]
fn the_key_is_never_in_the_accessibility_tree_while_it_is_typed() {
    let (mut harness, _handles) = settings(Lang::En, TestConfig::default());
    let key = key();
    type_key(&mut harness, &key);
    assert_eq!(harness.state().ui.connect.key, key);
    assert_not_exposed(&harness, &key);
    assert!(!format!("{:?}", harness.state().ui).contains(&key), "the form's Debug output hides the key");
}

#[test]
fn a_key_of_300_characters_goes_to_garmin_whole() {
    let (mut harness, handles) = settings(Lang::En, TestConfig::default());
    let key: String = ('a'..='z').cycle().take(300).collect();
    type_key(&mut harness, &key);
    click(&mut harness, "Connect");
    tick(&mut harness);
    assert_eq!(handles.garmin.last_key().map(|key| key.len()), Some(300));
}

#[test]
fn connect_flow_rejected_shows_error() {
    for lang in Lang::ALL {
        let cfg = TestConfig { garmin: Err(GarminAuthError::KeyRejected), ..TestConfig::default() };
        let (mut harness, handles) = settings(lang, cfg);
        let key = key();
        type_key(&mut harness, &key);
        click(&mut harness, connect_label(lang));
        tick(&mut harness);
        harness.run();

        let (_, en, ru) = failure_texts()[0];
        harness.get_by_label(pick(lang, en, ru));
        // Still not connected: nothing saved, no "Connected", and the form with the key to correct.
        assert_eq!(status_of(&harness), ConnectionStatus::NotConnected, "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "Connected", "Подключено")).is_none(), "{lang:?}");
        assert_eq!(stored_key(handles.system.as_deref().expect("a system store")), None, "{lang:?}");
        assert_eq!(stored_key(&handles.file), None, "{lang:?}");
        assert_eq!(harness.state().ui.connect.key, key, "{lang:?}");
        assert!(button_enabled(&harness, connect_label(lang)), "{lang:?}: the user can try again");
        assert_not_exposed(&harness, &key);
    }
}

#[test]
fn every_way_the_check_can_fail_has_its_text() {
    for lang in Lang::ALL {
        for (error, en, ru) in failure_texts() {
            let (mut harness, _handles) = settings(lang, TestConfig { garmin: Err(error), ..TestConfig::default() });
            type_key(&mut harness, &key());
            click(&mut harness, connect_label(lang));
            tick(&mut harness);
            harness.run();
            harness.get_by_label(pick(lang, en, ru));
        }
    }
}

#[test]
fn connect_button_disabled_while_pending() {
    for lang in Lang::ALL {
        let cfg = TestConfig { garmin: Err(GarminAuthError::KeyRejected), deferred: true, ..TestConfig::default() };
        let (mut harness, handles) = settings(lang, cfg);
        let spawner = handles.spawner.expect("a deferred spawner");
        assert!(!button_enabled(&harness, connect_label(lang)), "{lang:?}: nothing to connect before a key is typed");
        type_key(&mut harness, &key());
        assert!(button_enabled(&harness, connect_label(lang)), "{lang:?}");

        click(&mut harness, connect_label(lang));
        assert!(harness.state().ui.connect.pending.is_some(), "{lang:?}");
        assert!(!button_enabled(&harness, connect_label(lang)), "{lang:?}: disabled while Garmin is asked");
        assert!(harness.get_by_role(Role::PasswordInput).accesskit_node().is_disabled(), "{lang:?}: the key cannot be changed under the check");
        harness.get_by_role(Role::ProgressIndicator);
        assert_eq!(spawner.pending(), 1, "{lang:?}: one check, and a second click starts no other");
        harness.get_by_role_and_label(Role::Button, connect_label(lang)).click();
        harness.step();
        assert_eq!(spawner.pending(), 1, "{lang:?}: clicking the disabled button does nothing");

        // The check ends: the form is free again and says why it failed.
        spawner.run_pending();
        tick(&mut harness);
        harness.run();
        assert!(harness.state().ui.connect.pending.is_none(), "{lang:?}");
        assert!(button_enabled(&harness, connect_label(lang)), "{lang:?}");
        assert!(!harness.get_by_role(Role::PasswordInput).accesskit_node().is_disabled(), "{lang:?}");
        assert!(harness.query_by_role(Role::ProgressIndicator).is_none(), "{lang:?}: the indicator is gone");
    }
}

#[test]
fn an_empty_key_cannot_be_submitted() {
    for lang in Lang::ALL {
        let (mut harness, handles) = settings(lang, TestConfig::default());
        assert!(!button_enabled(&harness, connect_label(lang)), "{lang:?}");
        type_key(&mut harness, "   ");
        assert!(!button_enabled(&harness, connect_label(lang)), "{lang:?}: blanks are no key");
        harness.get_by_role_and_label(Role::Button, connect_label(lang)).click();
        harness.run();
        assert_eq!((handles.garmin.calls(), harness.state().ui.connect.pending), (0, None), "{lang:?}");
        type_key(&mut harness, "x");
        assert!(button_enabled(&harness, connect_label(lang)), "{lang:?}");
    }
}

#[test]
fn enter_in_the_key_field_connects_like_the_button() {
    let (mut harness, handles) = settings(Lang::En, TestConfig::default());
    type_key(&mut harness, &key());
    harness.key_press(egui::Key::Enter);
    harness.step();
    harness.step();
    tick(&mut harness);
    harness.run();
    assert_eq!(status_of(&harness), ConnectionStatus::Connected);
    assert_eq!(handles.garmin.calls(), 1);
}

#[test]
fn the_term_chosen_sets_how_long_the_key_is_said_to_last() {
    for lang in Lang::ALL {
        let (mut harness, _handles) = settings(lang, TestConfig::default());
        harness.get_by_role_and_label(Role::ComboBox, pick(lang, "1 year", "1 год")).click();
        harness.run();
        for (en, ru) in [("30 days", "30 дней"), ("90 days", "90 дней"), ("180 days", "180 дней"), ("1 year", "1 год")] {
            harness.get_by_role_and_label(Role::Button, pick(lang, en, ru));
        }
        harness.get_by_role_and_label(Role::Button, pick(lang, "30 days", "30 дней")).click();
        harness.run();
        assert_eq!(harness.state().ui.connect.term, KeyTerm::Days30, "{lang:?}");

        type_key(&mut harness, &key());
        click(&mut harness, connect_label(lang));
        tick(&mut harness);
        harness.run();
        // 2026-10-23 plus 30 days.
        harness.get_by_label(pick(lang, "Key valid until 22 November 2026", "Ключ действует до 22 ноября 2026"));
    }
}

#[test]
fn the_card_says_where_the_key_will_be_stored() {
    // Before connecting it is where a new key will go (`newKeyBackend`): the file store in a debug build...
    let (harness, _handles) = settings(Lang::En, TestConfig { prefer_file: true, ..TestConfig::default() });
    harness.get_by_label("The key is stored in a file on disk");
    // ...and the system store otherwise, named in the terms of the user's system.
    let (harness, _handles) = settings(Lang::En, TestConfig::default());
    harness.get_by_label(storage_line(SecretBackend::System, std::env::consts::OS));
    // No consent box where the system store works.
    assert!(harness.query_by_role(Role::CheckBox).is_none());
}

#[test]
fn the_store_is_named_in_the_terms_of_the_users_system() {
    for lang in Lang::ALL {
        i18n::set_current(lang);
        let line = |backend, os| storage_line(backend, os);
        assert_eq!(line(SecretBackend::System, "macos"), pick(lang, "The key is stored in the macOS keychain", "Ключ хранится в связке ключей macOS"));
        assert_eq!(
            line(SecretBackend::System, "windows"),
            pick(lang, "The key is stored in the Windows credential manager", "Ключ хранится в диспетчере учётных данных Windows")
        );
        let password_store = pick(lang, "The key is stored in the system password store", "Ключ хранится в хранилище паролей системы");
        assert_eq!(line(SecretBackend::System, "linux"), password_store);
        assert_eq!(line(SecretBackend::System, "freebsd"), password_store, "any other system uses its password store");
        let file = pick(lang, "The key is stored in a file on disk", "Ключ хранится в файле на диске");
        for os in ["macos", "windows", "linux"] {
            assert_eq!(line(SecretBackend::File, os), file, "{lang:?}: a file is a file everywhere");
        }
    }
}

#[test]
fn file_store_consent_required() {
    for lang in Lang::ALL {
        let cfg = TestConfig { system_store: false, prefer_file: false, ..TestConfig::default() };
        let (mut harness, handles) = settings(lang, cfg);
        // Spec section 7: the warning names the file, so the user knows what they agree to.
        let warning = pick(
            lang,
            "The system password store is unavailable. Save the key to the file /test/secrets/garmin-personal-api-key? Any program running as you will be able to read it.",
            "Системное хранилище паролей недоступно. Сохранить ключ в файл /test/secrets/garmin-personal-api-key? Его сможет прочитать любая программа, запущенная от вашего имени.",
        );
        let tick_box = pick(lang, "Save the key to a file", "Сохранить ключ в файл");
        harness.get_by_label(warning);
        harness.get_by_label(pick(lang, "The key is stored in a file on disk", "Ключ хранится в файле на диске"));

        // Without the tick the engine refuses, and says what to do.
        let key = key();
        type_key(&mut harness, &key);
        click(&mut harness, connect_label(lang));
        harness.get_by_label(pick(
            lang,
            "The system password store is unavailable. Allow saving the key to a file to continue.",
            "Системное хранилище паролей недоступно. Разрешите сохранить ключ в файл, чтобы продолжить.",
        ));
        harness.get_by_label(warning);
        assert_eq!((status_of(&harness), handles.garmin.calls(), stored_key(&handles.file)), (ConnectionStatus::NotConnected, 0, None), "{lang:?}");

        // With it, the key is saved to the file.
        harness.get_by_role_and_label(Role::CheckBox, tick_box).click();
        harness.run();
        assert!(harness.state().ui.connect.allow_file_store, "{lang:?}");
        click(&mut harness, connect_label(lang));
        tick(&mut harness);
        harness.run();
        harness.get_by_label(pick(lang, "Connected", "Подключено"));
        assert_eq!(stored_key(&handles.file).as_deref(), Some(key.as_str()), "{lang:?}");
        assert_eq!(harness.state().status().and_then(|status| status.secret_backend), Some(SecretBackend::File), "{lang:?}");
        // A connection made to a file says so.
        harness.get_by_label(pick(lang, "The key is stored in a file on disk", "Ключ хранится в файле на диске"));
        assert!(harness.query_by_label(warning).is_none(), "{lang:?}: nothing to consent to once connected");
    }
}

#[test]
fn the_consent_warning_names_the_file_the_engine_reports() {
    for lang in Lang::ALL {
        let location = "/home/user/.local/share/gymgar/secrets/garmin-personal-api-key";
        let cfg = TestConfig { system_store: false, file_store_location: Some(location.to_owned()), ..TestConfig::default() };
        let (harness, _handles) = settings(lang, cfg);
        let (before, after) = match lang {
            Lang::En => ("The system password store is unavailable. Save the key to the file ", "? Any program running as you will be able to read it."),
            Lang::Ru => {
                ("Системное хранилище паролей недоступно. Сохранить ключ в файл ", "? Его сможет прочитать любая программа, запущенная от вашего имени.")
            }
        };
        harness.get_by_label(&format!("{before}{location}{after}"));
    }
}

#[test]
fn the_consent_warning_still_asks_when_the_file_is_not_known() {
    // A store that cannot say where it keeps the key: the warning has no path to show, but the consent is still asked for.
    for lang in Lang::ALL {
        let cfg = TestConfig { system_store: false, file_store_location: None, ..TestConfig::default() };
        let (harness, _handles) = settings(lang, cfg);
        harness.get_by_label(pick(
            lang,
            "The system password store is unavailable. Save the key to a file instead? Any program running as you will be able to read it.",
            "Системное хранилище паролей недоступно. Сохранить ключ в файл? Его сможет прочитать любая программа, запущенная от вашего имени.",
        ));
        harness.get_by_role_and_label(Role::CheckBox, pick(lang, "Save the key to a file", "Сохранить ключ в файл"));
    }
}

#[test]
fn the_consent_box_is_only_there_when_the_system_store_is_missing() {
    // A debug build prefers the file on purpose: the user has nothing to consent to.
    for (cfg, shown) in [
        (TestConfig::default(), false),
        (TestConfig { prefer_file: true, ..TestConfig::default() }, false),
        (TestConfig { system_store: false, prefer_file: true, ..TestConfig::default() }, false),
        (TestConfig { system_store: false, ..TestConfig::default() }, true),
    ] {
        let (harness, _handles) = settings(Lang::En, cfg.clone());
        assert_eq!(harness.query_by_role(Role::CheckBox).is_some(), shown, "{cfg:?}");
    }
}

#[test]
fn connected_card_shows_status_dates_and_where_the_key_is_kept() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (harness, _handles) = settings_connected(lang, &cfg, &working_connection(&cfg));
        harness.get_by_label(pick(lang, "Connected", "Подключено"));
        // Connected 2026-10-23 12:00 UTC with the default term of a year; checked at the same moment.
        harness.get_by_label(pick(lang, "Key valid until 23 October 2027", "Ключ действует до 23 октября 2027"));
        harness.get_by_label(pick(lang, "Verified 23 October 2026, 12:00", "Проверен 23 октября 2026, 12:00"));
        harness.get_by_label(pick(lang, "The key is stored in a file on disk", "Ключ хранится в файле на диске"));
        harness.get_by_role_and_label(Role::Button, pick(lang, "Replace key", "Заменить ключ"));
        harness.get_by_role_and_label(Role::Button, pick(lang, "Disconnect", "Отключить"));
        // The instructions and the form are for connecting; this is a connection that works.
        assert!(harness.query_by_role(Role::PasswordInput).is_none(), "{lang:?}");
        assert!(harness.query_by_role(Role::Link).is_none(), "{lang:?}");
        // No expiry warning with a year to go.
        assert!(harness.query_by_label_contains(pick(lang, "expires", "истекает")).is_none(), "{lang:?}");
    }
}

#[test]
fn a_connection_never_checked_has_no_verified_line() {
    let cfg = TestConfig::default();
    let connection = GarminConnection { last_verified_at: None, ..working_connection(&cfg) };
    let (harness, _handles) = settings_connected(Lang::En, &cfg, &connection);
    harness.get_by_label("Connected");
    assert!(harness.query_by_label_contains("Verified").is_none());
}

#[test]
fn the_dates_are_shown_in_the_users_time_zone() {
    let cfg = TestConfig::default();
    let (app, _handles) = app_connected(Lang::En, cfg.clone(), &working_connection(&cfg));
    // 12:00 UTC is 01:00 the next day at UTC+13.
    let app = app.with_utc_offset(chrono::FixedOffset::east_opt(13 * 3600).expect("a valid offset"));
    let harness = harness_at(app, Section::Settings);
    harness.get_by_label("Verified 24 October 2026, 01:00");
    harness.get_by_label("Key valid until 24 October 2027");
}

#[test]
fn key_missing_and_rejected_states_render() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();

        // The key does not work any more: the same instructions and a form to replace it.
        let (harness, _handles) = settings_connected(lang, &cfg, &with_status(&cfg, ConnectionStatus::KeyRejected));
        harness.get_by_label(pick(lang, "Key not valid", "Ключ не действует"));
        harness.get_by_label(pick(
            lang,
            "Garmin no longer accepts the saved key: it may have expired or been deleted. Create a new key and enter it below.",
            "Garmin больше не принимает сохранённый ключ: он мог истечь или быть удалён. Выпустите новый ключ и введите его ниже.",
        ));
        harness.get_by_role(Role::Link);
        harness.get_by_role(Role::PasswordInput);
        harness.get_by_role_and_label(Role::Button, connect_label(lang));
        // The connection can also simply be removed.
        assert!(button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "Connected", "Подключено")).is_none(), "{lang:?}");
        assert!(harness.query_by_label_contains(pick(lang, "Key valid until", "Ключ действует до")).is_none(), "{lang:?}");

        // The key is gone from the store: the hint and a form to connect again.
        let (harness, _handles) = settings_connected(lang, &cfg, &with_status(&cfg, ConnectionStatus::KeyMissing));
        harness.get_by_label(pick(lang, "Key not found", "Ключ не найден"));
        harness.get_by_label(pick(lang, "Connect Garmin again", "Подключите Garmin заново"));
        harness.get_by_role(Role::Link);
        harness.get_by_role(Role::PasswordInput);
        harness.get_by_role_and_label(Role::Button, connect_label(lang));
        assert!(button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "Connected", "Подключено")).is_none(), "{lang:?}");

        // The key could not be checked (no network): the last known state, marked, with the buttons of a connection.
        let (harness, _handles) = settings_connected(lang, &cfg, &with_status(&cfg, ConnectionStatus::Unverified));
        harness.get_by_label(pick(lang, "Connected", "Подключено"));
        harness.get_by_label(pick(lang, "Could not verify", "Не удалось проверить"));
        harness.get_by_label(pick(lang, "Key valid until 23 October 2027", "Ключ действует до 23 октября 2027"));
        harness.get_by_role_and_label(Role::Button, pick(lang, "Replace key", "Заменить ключ"));
        harness.get_by_role_and_label(Role::Button, pick(lang, "Disconnect", "Отключить"));
        assert!(harness.query_by_role(Role::PasswordInput).is_none(), "{lang:?}");
    }
}

#[test]
fn a_connection_that_works_has_no_note_about_checking() {
    let cfg = TestConfig::default();
    let (harness, _handles) = settings_connected(Lang::En, &cfg, &working_connection(&cfg));
    assert!(harness.query_by_label("Could not verify").is_none());
}

#[test]
fn the_expiry_warning_on_settings_links_to_the_key_page_and_replace_key_is_one_button() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let connection = GarminConnection { key_expires_at: cfg.now + TimeDelta::days(3), ..working_connection(&cfg) };
        let (mut harness, _handles) = settings_connected(lang, &cfg, &connection);
        harness.get_by_label(pick(
            lang,
            "The Garmin key expires on 26 October 2026. Create a new one",
            "Ключ Garmin истекает 26 октября 2026. Выпустите новый",
        ));
        harness.get_by_role_and_label(Role::Link, pick(lang, "Personal API Key page", "Страница Personal API Key"));
        // One "Replace key" button, the one with "Disconnect", not a second one in the warning.
        click(&mut harness, pick(lang, "Replace key", "Заменить ключ"));
        assert!(harness.state().ui.connect.replacing, "{lang:?}");
        harness.get_by_role(Role::PasswordInput);
    }
}

#[test]
fn replacing_a_key_swaps_in_the_new_one_once_garmin_accepts_it() {
    for lang in Lang::ALL {
        let (app, handles) = app_with_key(lang, TestConfig::default(), "the-old-key");
        let mut harness = harness_at(app, Section::Settings);
        harness.get_by_label(pick(lang, "Connected", "Подключено"));
        click(&mut harness, pick(lang, "Replace key", "Заменить ключ"));
        harness.get_by_role(Role::PasswordInput);
        // The connection is still on show above the form, and the form can be left.
        harness.get_by_label(pick(lang, "Connected", "Подключено"));
        assert!(button_enabled(&harness, pick(lang, "Cancel", "Отмена")), "{lang:?}");

        type_key(&mut harness, "the-new-key");
        click(&mut harness, connect_label(lang));
        tick(&mut harness);
        harness.run();
        assert_eq!(handles.garmin.last_key().as_deref(), Some("the-new-key"), "{lang:?}");
        assert_eq!(stored_key(handles.system.as_deref().expect("a system store")).as_deref(), Some("the-new-key"), "{lang:?}");
        assert!(harness.query_by_role(Role::PasswordInput).is_none(), "{lang:?}: the form closes");
        assert!(!harness.state().ui.connect.replacing, "{lang:?}");
        assert_eq!(status_of(&harness), ConnectionStatus::Connected, "{lang:?}");
    }
}

#[test]
fn a_rejected_replacement_leaves_the_old_key_in_place() {
    for lang in Lang::ALL {
        let cfg = TestConfig { garmin: Err(GarminAuthError::KeyRejected), ..TestConfig::default() };
        let (mut harness, handles) = settings_connected(lang, &cfg, &connection_at(cfg.now, SecretBackend::System));
        let system = handles.system.as_deref().expect("a system store");
        system.set(&SecretString::from("the-old-key")).expect("save the old key");

        click(&mut harness, pick(lang, "Replace key", "Заменить ключ"));
        type_key(&mut harness, "the-new-key");
        click(&mut harness, connect_label(lang));
        tick(&mut harness);
        harness.run();

        let (_, en, ru) = failure_texts()[0];
        harness.get_by_label(pick(lang, en, ru));
        assert_eq!(status_of(&harness), ConnectionStatus::Connected, "{lang:?}: the old connection stays while the new key is refused");
        assert_eq!(stored_key(system).as_deref(), Some("the-old-key"), "{lang:?}: the refused key did not replace the old one");
        assert!(harness.state().ui.connect.replacing, "{lang:?}: the form stays open to correct the key");
        harness.get_by_role(Role::PasswordInput);
    }
}

#[test]
fn cancel_closes_the_replace_form_and_forgets_the_draft() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (mut harness, _handles) = settings_connected(lang, &cfg, &working_connection(&cfg));
        click(&mut harness, pick(lang, "Replace key", "Заменить ключ"));
        type_key(&mut harness, "a-draft");
        click(&mut harness, pick(lang, "Cancel", "Отмена"));
        assert!(harness.query_by_role(Role::PasswordInput).is_none(), "{lang:?}");
        assert_eq!((harness.state().ui.connect.replacing, harness.state().ui.connect.key.as_str()), (false, ""), "{lang:?}");
        harness.get_by_role_and_label(Role::Button, pick(lang, "Replace key", "Заменить ключ"));
    }
}

#[test]
fn disconnect_asks_confirmation() {
    for lang in Lang::ALL {
        let (app, handles) = app_with_key(lang, TestConfig::default(), "the-saved-key");
        let mut harness = harness_at(app, Section::Settings);
        let system = handles.system.as_deref().expect("a system store");
        assert_eq!(stored_key(system).as_deref(), Some("the-saved-key"));

        // The first click only asks.
        click(&mut harness, pick(lang, "Disconnect", "Отключить"));
        harness.get_by_label(pick(
            lang,
            "Disconnect Garmin? Gymgar will delete the key and the connection details.",
            "Отключить Garmin? Gymgar удалит ключ и сведения о подключении.",
        ));
        assert_eq!(status_of(&harness), ConnectionStatus::Connected, "{lang:?}");
        assert_eq!(stored_key(system).as_deref(), Some("the-saved-key"), "{lang:?}: the key is still there");

        // "Cancel" backs out and changes nothing.
        click(&mut harness, pick(lang, "Cancel", "Отмена"));
        assert_eq!((status_of(&harness), stored_key(system).as_deref()), (ConnectionStatus::Connected, Some("the-saved-key")), "{lang:?}");
        assert!(harness.query_by_label_contains(pick(lang, "Disconnect Garmin?", "Отключить Garmin?")).is_none(), "{lang:?}");

        // Confirming deletes the key and the connection, and the card is the form to connect again.
        click(&mut harness, pick(lang, "Disconnect", "Отключить"));
        click(&mut harness, pick(lang, "Yes, disconnect", "Да, отключить"));
        tick(&mut harness);
        harness.run();
        assert_eq!(status_of(&harness), ConnectionStatus::NotConnected, "{lang:?}");
        assert_eq!(stored_key(system), None, "{lang:?}");
        assert!(!harness.state().ui.connect.confirm_disconnect, "{lang:?}");
        harness.get_by_role(Role::PasswordInput);
        assert!(harness.query_by_label(pick(lang, "Connected", "Подключено")).is_none(), "{lang:?}");
    }
}

#[test]
fn a_connection_whose_key_does_not_work_can_be_disconnected_after_a_confirmation() {
    for lang in Lang::ALL {
        for status in [ConnectionStatus::KeyRejected, ConnectionStatus::KeyMissing] {
            let cfg = TestConfig::default();
            let (mut harness, handles) = settings_connected(lang, &cfg, &with_status(&cfg, status));
            if status == ConnectionStatus::KeyRejected {
                handles.file.set(&SecretString::from("the-refused-key")).expect("save the refused key");
            }

            // The first click only asks, and "Cancel" backs out.
            click(&mut harness, pick(lang, "Disconnect", "Отключить"));
            harness.get_by_label(pick(
                lang,
                "Disconnect Garmin? Gymgar will delete the key and the connection details.",
                "Отключить Garmin? Gymgar удалит ключ и сведения о подключении.",
            ));
            click(&mut harness, pick(lang, "Cancel", "Отмена"));
            assert!(harness.query_by_label_contains(pick(lang, "Disconnect Garmin?", "Отключить Garmin?")).is_none(), "{lang:?} {status:?}");
            assert_eq!(status_of(&harness), status, "{lang:?}: nothing changed");

            // Confirming removes the connection (and a key, if one is left), and the card offers to connect anew.
            click(&mut harness, pick(lang, "Disconnect", "Отключить"));
            click(&mut harness, pick(lang, "Yes, disconnect", "Да, отключить"));
            tick(&mut harness);
            harness.run();
            assert_eq!(status_of(&harness), ConnectionStatus::NotConnected, "{lang:?} {status:?}");
            assert_eq!(stored_key(&handles.file), None, "{lang:?} {status:?}");
            assert!(!harness.state().ui.connect.confirm_disconnect, "{lang:?} {status:?}");
            harness.get_by_role(Role::PasswordInput);
            assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Disconnect", "Отключить")).is_none(), "{lang:?} {status:?}");
            assert!(harness.query_by_label(pick(lang, "Key not valid", "Ключ не действует")).is_none(), "{lang:?} {status:?}");
            assert!(harness.query_by_label(pick(lang, "Key not found", "Ключ не найден")).is_none(), "{lang:?} {status:?}");
        }
    }
}

#[test]
fn the_form_of_a_key_that_does_not_work_has_no_cancel_of_its_own() {
    // Summary's "Replace key" opens the form in replacing mode. For a key that does not work the form is all there is,
    // so there is nothing to go back to, and the one "Cancel" on the card is the one of the question.
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (mut harness, _handles) = settings_connected(lang, &cfg, &with_status(&cfg, ConnectionStatus::KeyRejected));
        harness.state_mut().ui.connect.replacing = true;
        harness.run();
        assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Cancel", "Отмена")).is_none(), "{lang:?}");
        click(&mut harness, pick(lang, "Disconnect", "Отключить"));
        assert_eq!(harness.get_all_by_role_and_label(Role::Button, pick(lang, "Cancel", "Отмена")).count(), 1, "{lang:?}");
        click(&mut harness, pick(lang, "Cancel", "Отмена"));
        assert!(!harness.state().ui.connect.confirm_disconnect, "{lang:?}");
    }
}

#[test]
fn disconnecting_a_missing_key_is_greyed_out_with_the_reason_when_its_store_is_out_of_reach() {
    for lang in Lang::ALL {
        // The key was kept in the system store, which this session does not have.
        let cfg = TestConfig { system_store: false, ..TestConfig::default() };
        let connection = GarminConnection { last_status: ConnectionStatus::KeyMissing, ..connection_at(cfg.now, SecretBackend::System) };
        let (harness, _handles) = settings_connected(lang, &cfg, &connection);
        assert!(!button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        harness.get_by_label(pick(
            lang,
            "Gymgar cannot reach the password store that holds the key, so it cannot disconnect now.",
            "Gymgar не может обратиться к хранилищу паролей, где лежит ключ, поэтому сейчас не может отключить Garmin.",
        ));
        // Connecting again still works.
        harness.get_by_role(Role::PasswordInput);
    }
}

#[test]
fn disconnecting_a_rejected_key_is_greyed_out_while_a_check_runs() {
    for lang in Lang::ALL {
        let cfg = TestConfig { deferred: true, ..TestConfig::default() };
        let (app, _handles) = app_connected(lang, cfg.clone(), &with_status(&cfg, ConnectionStatus::KeyRejected));
        let mut harness = harness_at(app, Section::Settings);
        assert!(button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        // A check of the key is under way.
        harness.state_mut().run("garmin.verify", serde_json::Value::Null).expect("start a check");
        tick(&mut harness);
        assert!(harness.state().status().is_some_and(|status| status.check_running), "{lang:?}");
        assert!(!button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        harness.get_by_label(pick(lang, "A key check is running. Try again in a moment.", "Идёт проверка ключа. Попробуйте через минуту."));
    }
}

#[test]
fn the_confirmation_does_not_come_back_armed_after_a_disconnect_elsewhere() {
    let (app, _handles) = app_with_key(Lang::En, TestConfig::default(), "the-saved-key");
    let mut harness = harness_at(app, Section::Settings);
    click(&mut harness, "Disconnect");
    assert!(harness.state().ui.connect.confirm_disconnect);
    // The connection goes away behind the window's back (another process, or the user in another window).
    harness.state_mut().run("garmin.disconnect", serde_json::Value::Null).expect("disconnect");
    tick(&mut harness);
    harness.run();
    assert!(!harness.state().ui.connect.confirm_disconnect, "nothing is left to confirm");
}

#[test]
fn disconnect_is_greyed_out_with_the_reason_when_the_store_is_out_of_reach() {
    for lang in Lang::ALL {
        // The key lives in the system store, which is not there now (a locked or removed keychain).
        let cfg = TestConfig { system_store: false, ..TestConfig::default() };
        let connection = connection_at(cfg.now, SecretBackend::System);
        let (harness, _handles) = settings_connected(lang, &cfg, &connection);
        assert!(!button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        harness.get_by_label(pick(
            lang,
            "Gymgar cannot reach the password store that holds the key, so it cannot disconnect now.",
            "Gymgar не может обратиться к хранилищу паролей, где лежит ключ, поэтому сейчас не может отключить Garmin.",
        ));
        // Replacing the key still works.
        assert!(button_enabled(&harness, pick(lang, "Replace key", "Заменить ключ")), "{lang:?}");
    }
}

#[test]
fn disconnect_is_greyed_out_while_a_check_runs() {
    for lang in Lang::ALL {
        let cfg = TestConfig { deferred: true, ..TestConfig::default() };
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        let mut harness = harness_at(app, Section::Settings);
        assert!(button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        // A check of the key is under way.
        harness.state_mut().run("garmin.verify", serde_json::Value::Null).expect("start a check");
        tick(&mut harness);
        assert!(harness.state().status().is_some_and(|status| status.check_running), "{lang:?}");
        assert!(!button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        harness.get_by_label(pick(lang, "A key check is running. Try again in a moment.", "Идёт проверка ключа. Попробуйте через минуту."));
    }
}

#[test]
fn a_failed_disconnect_shows_why_and_closes_the_confirmation() {
    let (app, handles) = app_with_key(Lang::En, TestConfig::default(), "the-saved-key");
    let mut harness = harness_at(app, Section::Settings);
    click(&mut harness, "Disconnect");
    // The store breaks between the question and the answer.
    handles.system.as_deref().expect("a system store").set_failing(true);
    click(&mut harness, "Yes, disconnect");
    tick(&mut harness);
    harness.get_by_label("Gymgar could not use the password store. Check that it is unlocked and try again.");
    assert!(!harness.state().ui.connect.confirm_disconnect);
    assert_eq!(status_of(&harness), ConnectionStatus::Connected, "nothing was removed");
}

#[test]
fn disconnect_shows_that_it_runs_and_is_available_again_after_an_error() {
    for lang in Lang::ALL {
        let cfg = TestConfig { deferred: true, ..TestConfig::default() };
        let (mut harness, handles) = settings_connected(lang, &cfg, &working_connection(&cfg));
        handles.file.set(&SecretString::from("the-saved-key")).expect("save the key");
        let spawner = handles.spawner.clone().expect("a deferred spawner");
        let disconnect = pick(lang, "Disconnect", "Отключить");
        assert!(harness.query_by_role(Role::ProgressIndicator).is_none(), "{lang:?}");

        click(&mut harness, disconnect);
        click(&mut harness, pick(lang, "Yes, disconnect", "Да, отключить"));
        // The key is being deleted: the button waits with the indicator beside it, and there is no reason to give.
        assert!(harness.state().ui.connect.disconnecting.is_some(), "{lang:?}");
        assert_eq!(spawner.pending(), 1, "{lang:?}");
        assert!(!button_enabled(&harness, disconnect), "{lang:?}");
        assert!(!button_enabled(&harness, pick(lang, "Replace key", "Заменить ключ")), "{lang:?}");
        harness.get_by_role(Role::ProgressIndicator);
        assert!(harness.query_by_label_contains(pick(lang, "cannot disconnect now", "сейчас не может отключить")).is_none(), "{lang:?}");
        assert!(harness.query_by_label_contains(pick(lang, "key check is running", "проверка ключа")).is_none(), "{lang:?}");

        // The store fails: the connection stays, the card says why, and the button can be tried again.
        handles.file.set_failing(true);
        spawner.run_pending();
        tick(&mut harness);
        harness.run();
        assert!(harness.state().ui.connect.disconnecting.is_none(), "{lang:?}");
        assert_eq!(status_of(&harness), ConnectionStatus::Connected, "{lang:?}");
        harness.get_by_label(pick(
            lang,
            "Gymgar could not use the password store. Check that it is unlocked and try again.",
            "Gymgar не смог обратиться к хранилищу паролей. Проверьте, что оно разблокировано, и попробуйте ещё раз.",
        ));
        assert!(harness.query_by_role(Role::ProgressIndicator).is_none(), "{lang:?}");
        assert!(button_enabled(&harness, disconnect), "{lang:?}");

        // The second try works.
        handles.file.set_failing(false);
        click(&mut harness, disconnect);
        click(&mut harness, pick(lang, "Yes, disconnect", "Да, отключить"));
        spawner.run_pending();
        tick(&mut harness);
        harness.run();
        assert_eq!(status_of(&harness), ConnectionStatus::NotConnected, "{lang:?}");
        assert_eq!(stored_key(&handles.file), None, "{lang:?}");
        assert!(harness.state().ui.connect.error.is_none(), "{lang:?}");
        harness.get_by_role(Role::PasswordInput);
    }
}

#[test]
fn while_a_key_that_does_not_work_is_disconnected_its_form_waits_too() {
    let cfg = TestConfig { deferred: true, ..TestConfig::default() };
    let (mut harness, handles) = settings_connected(Lang::En, &cfg, &with_status(&cfg, ConnectionStatus::KeyRejected));
    type_key(&mut harness, &key());
    assert!(button_enabled(&harness, "Connect"));
    click(&mut harness, "Disconnect");
    click(&mut harness, "Yes, disconnect");
    // One indicator, by the button that runs; the key cannot be connected while the old one is being deleted.
    assert!(!button_enabled(&harness, "Connect"));
    assert_eq!(harness.query_all_by_role(Role::ProgressIndicator).count(), 1);

    handles.spawner.expect("a deferred spawner").run_pending();
    tick(&mut harness);
    harness.run();
    assert_eq!(status_of(&harness), ConnectionStatus::NotConnected);
    assert!(button_enabled(&harness, "Connect"), "the key typed is still there to connect");
}

#[test]
fn an_error_goes_away_when_the_language_changes() {
    let cfg = TestConfig { garmin: Err(GarminAuthError::Network), ..TestConfig::default() };
    let (mut harness, _handles) = settings(Lang::En, cfg);
    type_key(&mut harness, &key());
    click(&mut harness, "Connect");
    tick(&mut harness);
    harness.run();
    harness.get_by_label("Could not reach Garmin. Check your internet connection.");

    // The error was stored in English: in another language it would be stale text, so it is dropped.
    harness.state_mut().run("settings.setLanguage", json!({ "language": "ru" })).expect("set the language");
    harness.run();
    assert!(harness.state().ui.connect.error.is_none());
    assert!(harness.query_by_label("Could not reach Garmin. Check your internet connection.").is_none());
    assert!(harness.query_by_label("Не удалось связаться с Garmin. Проверьте подключение к интернету.").is_none());
    // The key the user typed is not lost.
    assert_eq!(harness.state().ui.connect.key, key());
}

#[test]
fn an_error_stays_while_the_language_does_not_change() {
    let cfg = TestConfig { garmin: Err(GarminAuthError::Network), ..TestConfig::default() };
    let (mut harness, _handles) = settings(Lang::En, cfg);
    type_key(&mut harness, &key());
    click(&mut harness, "Connect");
    tick(&mut harness);
    harness.run();
    // Another command that refreshes the window changes nothing about the language.
    harness.state_mut().run("settings.setLanguage", json!({ "language": "system" })).expect("set the language");
    harness.run();
    harness.get_by_label("Could not reach Garmin. Check your internet connection.");
}

#[test]
fn a_status_that_cannot_be_read_says_so_instead_of_offering_a_form() {
    for lang in Lang::ALL {
        // The table of the connection is gone, so the status cannot be read.
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("gymgar.sqlite3");
        let store = Store::open(&path).expect("open the database");
        rusqlite::Connection::open(&path).expect("open the file").execute("DROP TABLE garmin_connection", []).expect("drop the table");
        let (session, _handles) = test_session_on(store, TestConfig::default());
        let app = GymgarApp::with_system_locale(session, UiState::default(), Some(pick(lang, "en-US", "ru-RU").to_owned()));
        assert!(app.status().is_none());

        let harness = harness_at(app, Section::Settings);
        harness.get_by_label(pick(lang, "The state of Garmin Connect is not available.", "Состояние Garmin Connect недоступно."));
        assert!(harness.query_by_role(Role::PasswordInput).is_none(), "{lang:?}");
        assert!(harness.query_by_role_and_label(Role::Button, connect_label(lang)).is_none(), "{lang:?}");
    }
}

#[test]
fn while_workouts_load_the_key_waits_and_says_why() {
    // The import runs at every start now: the key it uses is neither disconnected nor replaced under it, and the card
    // says so instead of blaming the password store.
    for lang in Lang::ALL {
        let cfg = TestConfig { deferred: true, ..TestConfig::default() };
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        let mut harness = harness_at(app, Section::Settings);
        harness.state_mut().run("garmin.sync", serde_json::Value::Null).expect("start a sync");
        tick(&mut harness);
        assert!(!button_enabled(&harness, pick(lang, "Disconnect", "Отключить")), "{lang:?}");
        harness.get_by_label(pick(
            lang,
            "Workouts are loading from Garmin. Try again when it ends.",
            "Идёт загрузка тренировок из Garmin. Попробуйте, когда она закончится.",
        ));
        assert!(harness.query_by_label_contains(pick(lang, "cannot reach the password store", "не может обратиться к хранилищу")).is_none(), "{lang:?}");

        click(&mut harness, pick(lang, "Replace key", "Заменить ключ"));
        type_key(&mut harness, "a-new-key");
        assert!(!button_enabled(&harness, pick(lang, "Connect", "Подключить")), "{lang:?}: the engine would refuse it now");
    }
}
