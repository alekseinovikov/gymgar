use chrono::TimeDelta;
use egui::Role;
use egui_kittest::kittest::Queryable;
use gymgar_domain::{ConnectionStatus, GarminConnection, STRENGTH_TYPE_KEY};
use gymgar_engine::testing::TestConfig;

use crate::Section;
use crate::i18n::Lang;
use crate::state::ListFilter;
use crate::test_support::{app_connected, app_in, app_seeded, click, harness, listed, pick, store_activities, working_connection};

/// The state line of the Garmin card on a working connection.
const CONNECTED_TEXT: (&str, &str) = ("Connected", "Подключено");

/// The text the Summary had before workouts were loaded: it must be gone (spec §2, item 15).
const NEXT_UPDATE: (&str, &str) =
    ("Garmin is connected. Loading workouts will arrive in the next update.", "Garmin подключён. Загрузка тренировок появится в следующем обновлении.");

const NEW_STRENGTH: i64 = 9_000_000_000_001;

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
        assert_eq!(harness.get_all_by_role(Role::Button).count(), 4, "{lang:?}: the three rail buttons and the one on the card");
    }
}

#[test]
fn connected_summary_text() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        let harness = harness(app);
        harness.get_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1));
        assert!(harness.query_by_label(pick(lang, NEXT_UPDATE.0, NEXT_UPDATE.1)).is_none(), "{lang:?}: the promise is kept, so it goes");
        // Connected: no invitation, and no warning while the key has a year to live.
        assert!(harness.query_by_label(pick(lang, "Connect to Garmin Connect", "Подключите Garmin Connect")).is_none(), "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "Connect Garmin", "Подключить Garmin")).is_none(), "{lang:?}");
        assert!(harness.query_by_label_contains(pick(lang, "expires", "истекает")).is_none(), "{lang:?}");
    }
}

#[test]
fn a_key_that_could_not_be_checked_shows_the_connected_text_marked() {
    // Design spec §2, item 8: the last known state, with the mark that the check got no answer.
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &with_status(&cfg, ConnectionStatus::Unverified));
        let harness = harness(app);
        harness.get_by_label(pick(lang, CONNECTED_TEXT.0, CONNECTED_TEXT.1));
        harness.get_by_label(pick(lang, "Could not verify", "Не удалось проверить"));
    }
}

#[test]
fn a_key_checked_with_success_has_no_mark() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let (app, _handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        assert!(harness(app).query_by_label(pick(lang, "Could not verify", "Не удалось проверить")).is_none(), "{lang:?}");
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

/// The Summary on the fixtures plus two activities the user has not opened yet: a strength one and a run.
fn with_two_new(lang: Lang) -> egui_kittest::Harness<'static, crate::GymgarApp> {
    let cfg = TestConfig::default();
    let new = [
        listed(NEW_STRENGTH, STRENGTH_TYPE_KEY, Some("Upper A"), cfg.now - TimeDelta::hours(4), true),
        listed(NEW_STRENGTH + 1, "running", None, cfg.now - TimeDelta::hours(18), true),
    ];
    let (app, _handles) = app_seeded(lang, cfg, true, |store| store_activities(store, &new));
    harness(app)
}

#[test]
fn summary_new_card_opens_filtered_list() {
    for lang in Lang::ALL {
        let mut harness = with_two_new(lang);
        harness.get_by_label(pick(lang, "2 new workouts", "2 новые тренировки"));
        click(&mut harness, pick(lang, "Show new", "Показать новые"));
        let ui = &harness.state().ui;
        assert_eq!((ui.section, ui.activities.filter), (Section::Activities, ListFilter::New), "{lang:?}");
        // The list shows the two, and only them.
        harness.get_by_role_and_label(Role::Button, "Upper A");
        harness.get_by_role_and_label(Role::Button, pick(lang, "Running", "Бег"));
        assert!(harness.query_by_role_and_label(Role::Button, "strength_training 1").is_none(), "{lang:?}");
    }
}

#[test]
fn the_new_card_counts_in_the_language_and_goes_when_nothing_is_new() {
    for lang in Lang::ALL {
        let cfg = TestConfig::default();
        let one = [listed(NEW_STRENGTH, STRENGTH_TYPE_KEY, Some("Upper A"), cfg.now - TimeDelta::hours(4), true)];
        let (app, _handles) = app_seeded(lang, cfg.clone(), true, |store| store_activities(store, &one));
        harness(app).get_by_label(pick(lang, "1 new workout", "1 новая тренировка"));
        let five: Vec<_> = (0..5).map(|n| listed(NEW_STRENGTH + n, STRENGTH_TYPE_KEY, None, cfg.now - TimeDelta::hours(n + 1), true)).collect();
        let (app, _handles) = app_seeded(lang, cfg.clone(), true, |store| store_activities(store, &five));
        harness(app).get_by_label(pick(lang, "5 new workouts", "5 новых тренировок"));

        let (app, _handles) = app_seeded(lang, cfg, true, |_| {});
        let harness = harness(app);
        assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Show new", "Показать новые")).is_none(), "{lang:?}");
    }
}

#[test]
fn the_last_strength_workout_has_a_card_that_opens_it() {
    for lang in Lang::ALL {
        let mut summary = with_two_new(lang);
        summary.get_by_label(pick(lang, "Last strength workout", "Последняя силовая"));
        summary.get_by_label("Upper A");
        summary.get_by_label(pick(lang, "Fri, 23 Oct · 1:00", "Пт, 23 окт · 1:00"));
        summary.state_mut().ui.activities.filter = ListFilter::New;
        click(&mut summary, pick(lang, "Open", "Открыть"));
        let ui = &summary.state().ui;
        assert_eq!((ui.section, ui.activities.selected), (Section::Activities, Some(NEW_STRENGTH)), "{lang:?}");
        assert_eq!(ui.activities.filter, ListFilter::New, "{lang:?}: a filter that shows it stays");

        // From a filter that would hide it (an old workout is not under "New"), the list goes back to all.
        let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |_| {});
        let mut harness = harness(app);
        harness.get_by_label("strength_training 1");
        harness.state_mut().ui.activities.filter = ListFilter::New;
        click(&mut harness, pick(lang, "Open", "Открыть"));
        assert_eq!(harness.state().ui.activities.filter, ListFilter::All, "{lang:?}");
    }
}

#[test]
fn a_summary_without_strength_workouts_has_no_card_for_them() {
    let (app, _handles) = app_connected(Lang::En, TestConfig::default(), &working_connection(&TestConfig::default()));
    let harness = harness(app);
    assert!(harness.query_by_label("Last strength workout").is_none());
    harness.get_by_label("Connected");
}
