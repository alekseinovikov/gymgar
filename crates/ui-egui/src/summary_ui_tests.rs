use chrono::TimeDelta;
use egui::Role;
use egui_kittest::kittest::Queryable;
use gymgar_domain::{ConnectionStatus, GarminConnection};
use gymgar_engine::testing::TestConfig;

use crate::Section;
use crate::i18n::Lang;
use crate::test_support::{app_connected, app_in, click, harness, pick, working_connection};

const CONNECTED_TEXT: (&str, &str) =
    ("Garmin is connected. Loading workouts will arrive in the next update.", "Garmin подключён. Загрузка тренировок появится в следующем обновлении.");

fn expiring_in(cfg: &TestConfig, days: i64) -> GarminConnection {
    GarminConnection { key_expires_at: cfg.now + TimeDelta::days(days), ..working_connection(cfg) }
}

fn with_status(cfg: &TestConfig, status: ConnectionStatus) -> GarminConnection {
    GarminConnection { last_status: status, ..working_connection(cfg) }
}

#[test]
fn first_run_summary_has_connect_card() {
    for lang in Lang::ALL {
        let (app, _handles) = app_in(lang, TestConfig::default());
        let mut harness = harness(app);
        assert_eq!(harness.state().ui.section, Section::Summary);
        harness.get_by_label(pick(lang, "Workouts, progress and recovery will appear here", "Здесь появятся тренировки, прогресс и восстановление"));
        harness.get_by_label(pick(lang, "Connect to Garmin Connect", "Подключите Garmin Connect"));
        harness.get_by_label(pick(
            lang,
            "Gymgar takes your workouts, sleep and HRV from Garmin. You need a personal API key, which takes a couple of minutes.",
            "Gymgar берёт ваши тренировки, сон и ВСР из Garmin. Нужен личный ключ API — это займёт пару минут.",
        ));

        click(&mut harness, pick(lang, "Connect Garmin", "Подключить Garmin"));
        assert_eq!(harness.state().ui.section, Section::Settings, "{lang:?}: the button leads to Settings");
        harness.get_by_label(pick(lang, "Garmin Connect", "Garmin Connect"));
        assert!(harness.query_by_label(pick(lang, "Connect Garmin", "Подключить Garmin")).is_none(), "{lang:?}: the card is not on Settings");
    }
}

#[test]
fn the_first_run_has_one_card_and_no_connected_text() {
    for lang in Lang::ALL {
        let (app, _handles) = app_in(lang, TestConfig::default());
        let harness = harness(app);
        assert!(harness.query_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1)).is_none());
        assert_eq!(harness.get_all_by_role(Role::Button).count(), 3, "{lang:?}: the two rail buttons and the one on the card");
    }
}

#[test]
fn connected_summary_text() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        let harness = harness(app);
        harness.get_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1));
        // Connected: no invitation, and no warning while the key has a year to live.
        assert!(harness.query_by_label(pick(lang, "Connect to Garmin Connect", "Подключите Garmin Connect")).is_none(), "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "Connect Garmin", "Подключить Garmin")).is_none(), "{lang:?}");
        assert!(harness.query_by_label_contains(pick(lang, "expires", "истекает")).is_none(), "{lang:?}");
    }
}

#[test]
fn a_key_that_could_not_be_checked_still_shows_the_connected_text() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &with_status(&cfg, ConnectionStatus::Unverified));
        harness(app).get_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1));
    }
}

#[test]
fn expiry_warning_on_summary() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        // Three days left, from 2026-10-23 12:00 UTC.
        let (app, _handles) = app_connected(lang, cfg.clone(), &expiring_in(&cfg, 3));
        let mut harness = harness(app);
        harness.get_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1));
        harness.get_by_label(pick(
            lang,
            "The Garmin key expires on 26 October 2026. Create a new one",
            "Ключ Garmin истекает 26 октября 2026. Выпустите новый",
        ));
        harness.get_by_role_and_label(Role::Link, pick(lang, "Personal API Key page", "Страница Personal API Key"));

        // The button takes the user to the form that replaces the key.
        click(&mut harness, pick(lang, "Replace key", "Заменить ключ"));
        assert_eq!((harness.state().ui.section, harness.state().ui.connect.replacing), (Section::Settings, true), "{lang:?}");
        harness.get_by_role(Role::PasswordInput);
    }
}

#[test]
fn no_expiry_warning_until_the_last_week() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        for days in [8, 30, 365] {
            let (app, _handles) = app_connected(lang, cfg.clone(), &expiring_in(&cfg, days));
            let harness = harness(app);
            assert!(harness.query_by_label_contains(pick(lang, "expires", "истекает")).is_none(), "{lang:?}: {days} days left");
        }
        let (app, _handles) = app_connected(lang, cfg.clone(), &expiring_in(&cfg, 7));
        harness(app).get_by_label_contains(pick(lang, "expires on", "истекает"));
    }
}

#[test]
fn a_rejected_or_missing_key_asks_to_fix_it_on_summary() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &with_status(&cfg, ConnectionStatus::KeyRejected));
        let mut rejected = harness(app);
        rejected.get_by_label(pick(lang, "Key not valid", "Ключ не действует"));
        assert!(rejected.query_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1)).is_none(), "{lang:?}: a key that does not work is not a connection");
        click(&mut rejected, pick(lang, "Replace key", "Заменить ключ"));
        assert_eq!(rejected.state().ui.section, Section::Settings, "{lang:?}");

        let (app, _handles) = app_connected(lang, cfg.clone(), &with_status(&cfg, ConnectionStatus::KeyMissing));
        let mut missing = harness(app);
        missing.get_by_label(pick(lang, "Key not found", "Ключ не найден"));
        missing.get_by_label(pick(lang, "Connect Garmin again", "Подключите Garmin заново"));
        click(&mut missing, pick(lang, "Connect Garmin", "Подключить Garmin"));
        assert_eq!(missing.state().ui.section, Section::Settings, "{lang:?}");
    }
}
