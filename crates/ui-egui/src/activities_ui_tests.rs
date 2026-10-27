use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use gymgar_domain::{ConnectionStatus, STRENGTH_TYPE_KEY, SyncErrorKind};
use gymgar_engine::SyncStatusView;
use gymgar_engine::testing::TestConfig;
use gymgar_store::{NewActivity, Store};
use serde_json::{Value, json};

use super::*;
use crate::i18n::{self, Lang};
use crate::state::{ListFilter, Section};
use crate::test_support::{
    app_connected, app_in, app_seeded, button_enabled, click, harness_at, harness_sized, listed, pick, store_activities, tick, working_connection,
};
use crate::theme::metrics;

/// Two activities newer than the fixtures, neither opened yet: a strength one and a run without a name.
const NEW_STRENGTH: i64 = 9_000_000_000_001;
const NEW_RUN: i64 = 9_000_000_000_002;

fn utc(month: u32, day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, 0, 0).single().expect("a valid time")
}

fn two_new() -> Vec<NewActivity> {
    vec![listed(NEW_STRENGTH, STRENGTH_TYPE_KEY, Some("Upper A"), utc(10, 23, 8), true), listed(NEW_RUN, "running", None, utc(10, 22, 18), true)]
}

/// The Activities screen on the fixtures and the two new ones, in a window tall enough for the whole list.
fn activities(lang: Lang) -> Harness<'static, GymgarApp> {
    let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |store| store_activities(store, &two_new()));
    let mut harness = harness_sized(app, egui::vec2(1200.0, 1400.0));
    harness.state_mut().ui.section = Section::Activities;
    harness.run();
    harness
}

/// The top edge of the node `role` named `label`.
fn top(harness: &Harness<'_, GymgarApp>, role: Role, label: &str) -> f32 {
    harness.get_by_role_and_label(role, label).rect().top()
}

fn has_row(harness: &Harness<'_, GymgarApp>, title: &str) -> bool {
    harness.query_by_role_and_label(Role::Button, title).is_some()
}

#[test]
fn list_groups_by_week_and_filters() {
    for lang in Lang::ALL {
        let mut harness = activities(lang);
        let heading = |en: &'static str, ru: &'static str| pick(lang, en, ru).to_uppercase();
        let this_week = heading("This week", "Эта неделя");
        let september = heading("31 August – 6 September", "31 августа – 6 сентября");
        let may = heading("18–24 May", "18–24 мая");
        let april = heading("27 April – 3 May", "27 апреля – 3 мая");
        let mid_april = heading("13–19 April", "13–19 апреля");
        let running = pick(lang, "Running", "Бег");

        // Newest first, each week under its heading (the week of the test clock, Friday 23 October, is "this week").
        let order = [
            top(&harness, Role::Label, &this_week),
            top(&harness, Role::Button, "Upper A"),
            top(&harness, Role::Button, running),
            top(&harness, Role::Button, "strength_training 1"),
            top(&harness, Role::Button, "strength_training 2"),
            top(&harness, Role::Label, &september),
            top(&harness, Role::Button, "cycling 3"),
            top(&harness, Role::Label, &may),
            top(&harness, Role::Button, "cycling 4"),
            top(&harness, Role::Label, &april),
            top(&harness, Role::Button, "running 5"),
            top(&harness, Role::Button, "running 6"),
            top(&harness, Role::Label, &mid_april),
            top(&harness, Role::Button, "pilates 7"),
            top(&harness, Role::Button, "treadmill_running 8"),
        ];
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{lang:?}: {order:?}");

        // "New" shows only what the user has not opened yet.
        click(&mut harness, &pick(lang, "New · {}", "Новые · {}").replace("{}", "2"));
        assert_eq!(harness.state().ui.activities.filter, ListFilter::New, "{lang:?}");
        assert!(has_row(&harness, "Upper A") && has_row(&harness, running), "{lang:?}");
        for old in ["strength_training 1", "cycling 3", "pilates 7"] {
            assert!(!has_row(&harness, old), "{lang:?}: {old} is not new");
        }
        harness.get_by_label(&this_week);
        assert!(harness.query_by_label(&september).is_none(), "{lang:?}: an empty week has no heading");

        // "Strength" leaves the runs, rides and the pilates out.
        click(&mut harness, pick(lang, "Strength", "Силовые"));
        assert_eq!(harness.state().ui.activities.filter, ListFilter::Strength, "{lang:?}");
        for strength in ["Upper A", "strength_training 1", "strength_training 2"] {
            assert!(has_row(&harness, strength), "{lang:?}: {strength}");
        }
        for other in [running, "running 5", "cycling 3", "pilates 7", "treadmill_running 8"] {
            assert!(!has_row(&harness, other), "{lang:?}: {other} is not strength");
        }

        click(&mut harness, pick(lang, "All", "Все"));
        assert!(has_row(&harness, "pilates 7"), "{lang:?}");
    }
}

#[test]
fn a_row_shows_the_date_the_duration_and_the_sets_or_the_distance() {
    for lang in Lang::ALL {
        let harness = activities(lang);
        let fixture = harness.state().activities().iter().find(|item| item.name.as_deref() == Some("strength_training 1")).cloned().expect("the fixture");
        assert!(fixture.counted_sets > 0);
        let sets = i18n::trn(
            u64::from(fixture.counted_sets),
            pick(lang, "{} set", "{} подход"),
            pick(lang, "{} sets", "{} подхода"),
            pick(lang, "{} sets", "{} подходов"),
        );
        harness.get_by_label(&format!("{} · 1:39 · {sets}", pick(lang, "Thu, 22 Oct", "Чт, 22 окт")));
        // A run has its distance before its duration; a strength workout whose file is still to come has no count yet.
        harness.get_by_label(&format!("{} · {} · 1:00", pick(lang, "Thu, 22 Oct", "Чт, 22 окт"), pick(lang, "5.2 km", "5,2 км")));
        harness.get_by_label(pick(lang, "Fri, 23 Oct · 1:00", "Пт, 23 окт · 1:00"));
    }
}

#[test]
fn clicking_a_row_selects_it_and_the_choice_is_kept() {
    let mut harness = activities(Lang::En);
    harness.get_by_label("Select an activity");
    click(&mut harness, "cycling 3");
    let selected = harness.state().activities().iter().find(|item| item.name.as_deref() == Some("cycling 3")).map(|item| item.id.0);
    assert_eq!(harness.state().ui.activities.selected, selected);
    let toggled =
        |harness: &Harness<'_, GymgarApp>, label: &str| format!("{:?}", harness.get_by_role_and_label(Role::Button, label).accesskit_node().toggled());
    assert_eq!((toggled(&harness, "cycling 3").as_str(), toggled(&harness, "cycling 4").as_str()), ("Some(True)", "Some(False)"));
}

#[test]
fn long_and_missing_names() {
    // Review Focus 5: a name of 300 characters is cut with an ellipsis inside the list, and a missing or empty one is
    // the name of the activity's type.
    let long: String = "Very long workout name ".repeat(14).chars().take(300).collect();
    assert_eq!((long.chars().count(), long.trim() == long), (300, true));
    for lang in Lang::ALL {
        let start = utc(10, 21, 7);
        let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |store| {
            store_activities(
                store,
                &[
                    listed(1, STRENGTH_TYPE_KEY, Some(&long), start, false),
                    listed(2, "running", None, start - TimeDelta::hours(1), false),
                    listed(3, "lap_swimming", Some("  "), start - TimeDelta::hours(2), false),
                    listed(4, "e_bike_fitness", None, start - TimeDelta::hours(3), false),
                ],
            );
        });
        let harness = harness_at(app, Section::Activities);
        let list_edge = metrics::RAIL_WIDTH + metrics::LIST_WIDTH;
        let label = harness.get_by_role_and_label(Role::Label, &long);
        assert!(label.rect().right() <= list_edge + 0.5, "{lang:?}: the name reaches {:?}, past the list at {list_edge}", label.rect());
        let row = harness.get_by_role_and_label(Role::Button, &long);
        assert!(row.rect().right() <= list_edge + 0.5, "{lang:?}: {:?}", row.rect());
        let placeholder = harness.get_by_label(pick(lang, "Select an activity", "Выберите тренировку"));
        assert!(placeholder.rect().left() >= list_edge, "{lang:?}: the right side stays where it was");

        assert!(has_row(&harness, pick(lang, "Running", "Бег")), "{lang:?}");
        assert!(has_row(&harness, pick(lang, "Pool swimming", "Плавание в бассейне")), "{lang:?}: a blank name is no name");
        assert!(has_row(&harness, "E bike fitness"), "{lang:?}: an unknown type is its key, made readable");
    }
}

#[test]
fn every_type_of_the_list_has_a_name_in_both_languages() {
    let keys = [
        "strength_training",
        "running",
        "treadmill_running",
        "cycling",
        "indoor_cycling",
        "pilates",
        "walking",
        "swimming",
        "lap_swimming",
        "hiking",
        "yoga",
        "cardio",
        "hiit",
        "indoor_rowing",
    ];
    for lang in Lang::ALL {
        i18n::set_current(lang);
        let names: Vec<String> = keys.iter().map(|key| type_name(key)).collect();
        assert_eq!(names.iter().collect::<std::collections::HashSet<_>>().len(), keys.len(), "{lang:?}: {names:?}");
        for (key, name) in keys.iter().zip(&names) {
            assert!(!name.contains('_'), "{lang:?}: {key} is shown as {name}");
        }
    }
    i18n::set_current(Lang::Ru);
    assert_eq!(type_name("strength_training"), "Силовая тренировка");
    assert_eq!(type_name("running"), "Бег");
    assert_eq!(type_name(""), "", "nothing to make readable");
    assert_eq!(type_name("mountain_biking"), "Mountain biking");
}

/// The Activities screen with Garmin's last sync recorded as `success` and `error`.
fn after_sync(lang: Lang, success: Option<DateTime<Utc>>, error: Option<SyncErrorKind>) -> Harness<'static, GymgarApp> {
    let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |store: &Store| store.record_sync(success, error).expect("record the sync"));
    harness_at(app, Section::Activities)
}

#[test]
fn sync_error_texts() {
    // Activity import spec §7, «Ошибки загрузки», plus the storage error of Ruling 17.
    let texts = [
        (SyncErrorKind::KeyRejected, "Garmin does not accept the key. Replace it in Settings.", "Garmin не принимает ключ. Замените его в Настройках."),
        (SyncErrorKind::KeyMissing, "The Garmin key was not found. Connect Garmin again.", "Ключ Garmin не найден. Подключите Garmin заново."),
        (SyncErrorKind::RateLimited, "Garmin has temporarily limited requests. Will continue later.", "Garmin временно ограничил запросы. Продолжу позже."),
        (SyncErrorKind::Network, "No connection to Garmin. Showing what is already loaded.", "Нет связи с Garmin. Показываю то, что уже загружено."),
        (SyncErrorKind::Unavailable, "Garmin is unavailable right now. Will continue later.", "Garmin сейчас недоступен. Продолжу позже."),
        (SyncErrorKind::Storage, "Could not save part of what Garmin sent. Will try again later.", "Не удалось сохранить загруженное. Продолжу позже."),
    ];
    for lang in Lang::ALL {
        for (kind, en, ru) in texts {
            let mut harness = after_sync(lang, Some(utc(10, 22, 19)), Some(kind));
            harness.get_by_label(pick(lang, en, ru));
            // What was loaded is still there.
            assert!(has_row(&harness, "strength_training 1"), "{lang:?} {kind:?}");
            let link = pick(lang, "Open Settings", "Открыть настройки");
            let key = matches!(kind, SyncErrorKind::KeyRejected | SyncErrorKind::KeyMissing);
            assert_eq!(harness.query_by_role_and_label(Role::Link, link).is_some(), key, "{lang:?} {kind:?}: a link only where Settings is the fix");
            if key {
                harness.get_by_role_and_label(Role::Link, link).click();
                harness.run();
                assert_eq!((harness.state().ui.section, harness.state().ui.connect.replacing), (Section::Settings, true), "{lang:?} {kind:?}");
            }
        }
    }
}

#[test]
fn the_status_line_says_when_the_last_sync_ended() {
    for lang in Lang::ALL {
        let harness = after_sync(lang, Some(utc(10, 23, 11) + TimeDelta::minutes(12)), None);
        harness.get_by_label(pick(lang, "Loaded today at 11:12", "Загружено сегодня в 11:12"));
        let harness = after_sync(lang, Some(utc(10, 22, 19) + TimeDelta::minutes(5)), None);
        harness.get_by_label(pick(lang, "Loaded 22 October 2026, 19:05", "Загружено 22 октября 2026, 19:05"));
    }
}

#[test]
fn the_status_line_counts_the_files_while_a_sync_runs() {
    for lang in Lang::ALL {
        i18n::set_current(lang);
        let running = |done, total| SyncStatusView { running: true, files_done: done, files_total: total, ..SyncStatusView::default() };
        assert_eq!(progress_text(&running(12, 340)), Some(pick(lang, "Loading: 12 of 340", "Загружаю: 12 из 340").to_owned()));
        assert_eq!(progress_text(&running(0, 0)), Some(pick(lang, "Loading…", "Загружаю…").to_owned()), "still reading the list");
        // Nothing runs: nothing to count.
        assert_eq!(progress_text(&SyncStatusView { files_done: 3, files_total: 5, ..SyncStatusView::default() }), None);
    }
}

#[test]
fn refresh_disabled_while_syncing() {
    for lang in Lang::ALL {
        let cfg = TestConfig { deferred: true, ..TestConfig::default() };
        let (app, handles) = app_seeded(lang, cfg, true, |_| {});
        let spawner = handles.spawner.expect("a deferred spawner");
        let mut harness = harness_at(app, Section::Activities);
        let refresh = pick(lang, "Refresh", "Обновить");
        assert!(button_enabled(&harness, refresh), "{lang:?}");

        click(&mut harness, refresh);
        assert!(harness.state().status().is_some_and(|status| status.sync.running), "{lang:?}: the click started garmin.sync");
        assert!(!button_enabled(&harness, refresh), "{lang:?}: one sync at a time");
        harness.get_by_label(pick(lang, "Loading…", "Загружаю…"));
        assert_eq!(spawner.pending(), 1);

        spawner.run_pending();
        tick(&mut harness);
        tick(&mut harness);
        assert!(button_enabled(&harness, refresh), "{lang:?}: free again once the sync ended");
        harness.get_by_label(pick(lang, "Loaded today at 12:00", "Загружено сегодня в 12:00"));
    }
}

#[test]
fn not_connected_shows_the_way_to_settings_and_what_was_loaded() {
    for lang in Lang::ALL {
        let (app, _handles) = app_seeded(lang, TestConfig::default(), false, |_| {});
        let mut harness = harness_at(app, Section::Activities);
        harness.get_by_label(pick(lang, "Garmin Connect is not connected.", "Garmin Connect не подключён."));
        assert!(has_row(&harness, "strength_training 1"), "{lang:?}: what an earlier connection loaded stays");
        assert!(!button_enabled(&harness, pick(lang, "Refresh", "Обновить")), "{lang:?}: nothing to load from");
        harness.get_by_role_and_label(Role::Link, pick(lang, "Open Settings", "Открыть настройки")).click();
        harness.run();
        assert_eq!(harness.state().ui.section, Section::Settings);
    }
}

#[test]
fn an_empty_list_says_what_comes() {
    for lang in Lang::ALL {
        // Connected, nothing loaded yet, and the sync is under way.
        let cfg = TestConfig { deferred: true, ..TestConfig::default() };
        let (app, handles) = app_connected(lang, cfg.clone(), &working_connection(&cfg));
        let mut harness = harness_at(app, Section::Activities);
        harness.get_by_label(pick(lang, "No workouts yet. Refresh loads them from Garmin.", "Тренировок пока нет. «Обновить» загрузит их из Garmin."));
        harness.state_mut().run("garmin.sync", Value::Null).expect("start a sync");
        // The spinner of the running sync keeps asking for frames: a few are enough.
        harness.step();
        harness.step();
        harness.get_by_label(pick(lang, "Loading workouts from Garmin…", "Загружаю тренировки из Garmin…"));
        drop(handles);

        // A filter that leaves nothing out of a list that is not empty.
        let (app, _handles) = app_seeded(lang, TestConfig::default(), true, |_| {});
        let mut harness = harness_at(app, Section::Activities);
        click(&mut harness, &pick(lang, "New · {}", "Новые · {}").replace("{}", "0"));
        harness.get_by_label(pick(lang, "No workouts here", "Здесь тренировок нет"));
    }
}

#[test]
fn excluded_and_other_activities_are_dimmed_and_marked() {
    let (mut app, _handles) = app_seeded(Lang::En, TestConfig::default(), true, |_| {});
    let id = app.activities().iter().find(|item| item.name.as_deref() == Some("strength_training 1")).map(|item| item.id.0).expect("the fixture");
    app.run("activity.setExcluded", json!({ "id": id, "excluded": true })).expect("exclude it");
    let harness = harness_at(app, Section::Activities);
    harness.get_by_label_contains("1:39 · not counted");
    let item = harness.state().activities().iter().find(|item| item.id.0 == id).cloned().expect("still listed");
    assert!(item.excluded);
    let t = crate::theme::Tokens::SPORT;
    assert_eq!(text_color(&t, &item), t.text_dim);
    let other = harness.state().activities().iter().find(|item| item.type_key == "cycling").cloned().expect("a ride");
    assert_eq!(text_color(&t, &other), t.text_dim);
    let strength = harness.state().activities().iter().find(|item| item.name.as_deref() == Some("strength_training 2")).cloned().expect("strength");
    assert_eq!(text_color(&t, &strength), t.text);
}

#[test]
fn the_list_draws_only_the_rows_in_view() {
    // Thousands of activities: the list is read in pages of 1000 and drawn only where it is seen.
    let many: Vec<NewActivity> = (1..=2_500).map(|n| listed(n, "running", Some(&format!("Run {n}")), utc(10, 20, 0) - TimeDelta::hours(n), false)).collect();
    let (app, _handles) = app_seeded(Lang::En, TestConfig::default(), true, |store| store_activities(store, &many));
    assert_eq!(app.activities().len(), 2_508, "every page was read");
    let harness = harness_at(app, Section::Activities);
    let rows = harness.get_all_by_role(Role::Button).filter(|node| node.accesskit_node().label().is_some_and(|label| label.starts_with("Run "))).count();
    assert!(rows > 3 && rows < 40, "{rows} rows drawn");
}

#[test]
fn the_rail_leads_to_activities() {
    let (app, _handles) = app_in(Lang::En, TestConfig::default());
    let mut harness = harness_at(app, Section::Summary);
    click(&mut harness, "Activities");
    assert_eq!(harness.state().ui.section, Section::Activities);
    harness.get_by_role_and_label(Role::Heading, "Activities");
    let status = harness.state().status().map(|status| status.status);
    assert_eq!(status, Some(ConnectionStatus::NotConnected));
}
