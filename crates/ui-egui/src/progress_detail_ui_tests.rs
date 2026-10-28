use chrono::{NaiveDate, TimeZone, Utc};
use egui::{Role, Shape};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gymgar_domain::{ActivityId, ExerciseId, RecordedSet};
use gymgar_engine::testing::strength_set;
use gymgar_engine::{AdviceKind, AdviceView, ExerciseView, GapView, PointView, ProgressItemView, ProgressView, SessionView, SetBriefView, WorkingView};
use serde_json::json;

use super::*;
use crate::i18n::{self, Lang};
use crate::state::Section;
use crate::test_support::{app_with_workouts, click, harness_at, harness_sized, pick, tick, ui_harness};
use crate::theme::{Tokens, metrics};

const SQUAT: ExerciseId = ExerciseId { category: 28, name: None };

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("a date")
}

fn working(weight_kg: f64, reps: &[u16], date: NaiveDate) -> WorkingView {
    WorkingView { weight_kg, reps: reps.to_vec(), date, activity_id: ActivityId(1) }
}

fn advice(kind: AdviceKind) -> AdviceView {
    AdviceView { kind, target: None, next_weight_kg: None, sessions_at_weight: None, met_once: false, basis: Vec::new(), break_days: None, before_break: None }
}

/// The page of an exercise with the advice `advice` and `last` as its latest session, and nothing else.
fn view_with(advice: AdviceView, last: WorkingView) -> ProgressView {
    let exercise = ExerciseView { id: "squat".to_owned(), english: "Squat".to_owned(), watch_title: None };
    ProgressView {
        exercise: exercise.clone(),
        summary: Some(ProgressItemView {
            exercise,
            advice,
            last,
            sessions: 14,
            first_date: day(2024, 3, 2),
            dormant: false,
            chart_reps: Some(10),
            sparkline: Vec::new(),
        }),
        step_kg: Some(2.5),
        points: Vec::new(),
        heavier: Vec::new(),
        gaps: Vec::new(),
        sessions: Vec::new(),
    }
}

fn session(date: NaiveDate, workout_name: Option<&str>) -> SessionView {
    SessionView {
        date,
        start: Utc.from_utc_datetime(&date.and_hms_opt(8, 0, 0).expect("a time")),
        activity_id: ActivityId(1),
        workout_name: workout_name.map(str::to_owned),
        working: working(80.0, &[10], date),
        sets: vec![SetBriefView { weight_kg: Some(80.0), reps: Some(10) }],
    }
}

/// The two texts of a card, as the page shows them.
fn texts(lang: Lang, view: &ProgressView) -> Option<(String, String)> {
    i18n::set_current(lang);
    advice_texts(view)
}

/// A row of the table of `advice_texts`: what the advice is, and the title and explanation in English and in Russian.
struct Row {
    name: &'static str,
    view: ProgressView,
    en: (&'static str, &'static str),
    ru: (&'static str, &'static str),
}

fn add(target: u16, next: f64, previous: WorkingView, last: WorkingView) -> ProgressView {
    let advice = AdviceView { target: Some(target), next_weight_kg: Some(next), basis: vec![previous, last.clone()], ..advice(AdviceKind::Add) };
    view_with(advice, last)
}

fn hold(target: u16, met_once: bool, last: WorkingView) -> ProgressView {
    view_with(AdviceView { target: Some(target), sessions_at_weight: Some(1), met_once, ..advice(AdviceKind::Hold) }, last)
}

fn after_break(days: i64, before: Option<WorkingView>, last: WorkingView) -> ProgressView {
    let advice =
        AdviceView { target: Some(10), sessions_at_weight: Some(1), met_once: false, break_days: Some(days), before_break: before, ..advice(AdviceKind::Hold) };
    view_with(advice, last)
}

fn on_break(days: Option<i64>, last: WorkingView) -> ProgressView {
    view_with(AdviceView { break_days: days, ..advice(AdviceKind::Break) }, last)
}

#[test]
fn advice_texts_say_why_in_both_languages() {
    let eighty = |date| working(80.0, &[10], date);
    let rows = vec![
        Row {
            name: "add, both sessions in one month",
            view: add(10, 85.0, eighty(day(2026, 8, 2)), eighty(day(2026, 8, 18))),
            en: ("Can add: 85 kg", "80 kg × 10 two sessions in a row — 2 and 18 August. With 85 kg expect about 8 reps, then build back up to 10."),
            ru: ("Можно добавлять: 85 кг", "80 кг × 10 два занятия подряд — 2 и 18 августа. С 85 кг ждите около 8 повторов, дальше снова до 10."),
        },
        Row {
            name: "add, the sessions in two months",
            view: add(10, 82.5, eighty(day(2026, 7, 28)), eighty(day(2026, 8, 18))),
            en: ("Can add: 82.5 kg", "80 kg × 10 two sessions in a row — 28 July and 18 August. With 82.5 kg expect about 8 reps, then build back up to 10."),
            ru: ("Можно добавлять: 82,5 кг", "80 кг × 10 два занятия подряд — 28 июля и 18 августа. С 82,5 кг ждите около 8 повторов, дальше снова до 10."),
        },
        Row {
            name: "add, the first session of another year says its year",
            view: add(10, 85.0, eighty(day(2025, 12, 29)), eighty(day(2026, 1, 5))),
            en: (
                "Can add: 85 kg",
                "80 kg × 10 two sessions in a row — 29 December 2025 and 5 January. With 85 kg expect about 8 reps, then build back up to 10.",
            ),
            ru: ("Можно добавлять: 85 кг", "80 кг × 10 два занятия подряд — 29 декабря 2025 и 5 января. С 85 кг ждите около 8 повторов, дальше снова до 10."),
        },
        Row {
            name: "add, a target other than 10 says a couple of reps fewer",
            view: add(12, 85.0, working(80.0, &[12], day(2026, 8, 2)), working(80.0, &[12, 13], day(2026, 8, 18))),
            en: (
                "Can add: 85 kg",
                "80 kg × 12, 13 two sessions in a row — 2 and 18 August. With 85 kg expect a couple of reps fewer, then build back up to 12.",
            ),
            ru: ("Можно добавлять: 85 кг", "80 кг × 12, 13 два занятия подряд — 2 и 18 августа. С 85 кг ждите на пару повторов меньше, дальше снова до 12."),
        },
        Row {
            name: "hold, one working set",
            view: hold(10, false, working(90.0, &[6], day(2026, 10, 20))),
            en: ("Hold 90 kg", "Last time 90 kg × 6 of 10 (20 October). Add weight once you hit 10 in two sessions in a row."),
            ru: ("Держим 90 кг", "Последний раз 90 кг × 6 из 10 (20 октября). Добавлять — когда сделаете 10 два занятия подряд."),
        },
        Row {
            name: "hold, several working sets",
            view: hold(8, false, working(100.0, &[8, 8, 6], day(2026, 10, 20))),
            en: (
                "Hold 100 kg",
                "Last time 100 kg × 8, 8, 6 with a target of 8 (20 October). Add weight once every set at this weight hits 8 in two sessions in a row.",
            ),
            ru: (
                "Держим 100 кг",
                "Последний раз 100 кг × 8, 8, 6 при цели 8 (20 октября). Добавлять — когда все подходы на этом весе сделают 8 два занятия подряд.",
            ),
        },
        Row {
            name: "hold, the target met once",
            view: hold(10, true, working(80.0, &[10], day(2026, 8, 18))),
            en: ("Hold 80 kg for one more session", "80 kg × 10 — 18 August. Hit 10 once more and you can add weight."),
            ru: ("Держим 80 кг ещё одно занятие", "80 кг × 10 — 18 августа. Ещё раз 10 — и можно добавлять."),
        },
        Row {
            name: "hold after a break, with the session before it",
            view: after_break(63, Some(working(80.0, &[10], day(2026, 6, 1))), working(70.0, &[8], day(2026, 8, 3))),
            en: (
                "Hold 70 kg",
                "Last time 70 kg × 8 of 10 (3 August). Add weight once you hit 10 in two sessions in a row. After a 9-week break we count anew. Before the break: 80 kg × 10.",
            ),
            ru: (
                "Держим 70 кг",
                "Последний раз 70 кг × 8 из 10 (3 августа). Добавлять — когда сделаете 10 два занятия подряд. После перерыва в 9 недель считаем заново. До перерыва: 80 кг × 10.",
            ),
        },
        Row {
            name: "hold after a break, without the session before it",
            view: after_break(63, None, working(70.0, &[8], day(2026, 8, 3))),
            en: (
                "Hold 70 kg",
                "Last time 70 kg × 8 of 10 (3 August). Add weight once you hit 10 in two sessions in a row. After a 9-week break we count anew.",
            ),
            ru: (
                "Держим 70 кг",
                "Последний раз 70 кг × 8 из 10 (3 августа). Добавлять — когда сделаете 10 два занятия подряд. После перерыва в 9 недель считаем заново.",
            ),
        },
        Row {
            name: "break",
            view: on_break(Some(70), working(75.0, &[10], day(2026, 6, 16))),
            en: ("Break 10 weeks", "Last session 16 June: 75 kg × 10. The first session after the break will be the new starting point."),
            ru: ("Перерыв 10 недель", "Последнее занятие 16 июня: 75 кг × 10. Первое занятие после перерыва станет новой точкой отсчёта."),
        },
        Row {
            name: "break of an unknown length",
            view: on_break(None, working(75.0, &[10], day(2026, 6, 16))),
            en: ("Break", "Last session 16 June: 75 kg × 10. The first session after the break will be the new starting point."),
            ru: ("Перерыв", "Последнее занятие 16 июня: 75 кг × 10. Первое занятие после перерыва станет новой точкой отсчёта."),
        },
        Row {
            name: "bodyweight",
            view: view_with(advice(AdviceKind::Bodyweight), working(0.0, &[12], day(2026, 10, 20))),
            en: ("Bodyweight exercise", "The chart shows reps."),
            ru: ("Упражнение без веса", "График показывает повторы."),
        },
        Row {
            name: "no target",
            view: view_with(advice(AdviceKind::NoTarget), working(60.0, &[10], day(2026, 10, 20))),
            en: ("No target in the program", "The advice will appear once the exercise is in a program that plans reps."),
            ru: ("Нет цели по программе", "Подсказка появится, когда упражнение пойдёт по программе с повторами в плане."),
        },
    ];
    for row in rows {
        for (lang, (title, explanation)) in [(Lang::En, row.en), (Lang::Ru, row.ru)] {
            assert_eq!(texts(lang, &row.view), Some((title.to_owned(), explanation.to_owned())), "{}: {lang:?}", row.name);
        }
    }
}

#[test]
fn weeks_decline_in_russian() {
    let break_title = |days| texts(Lang::Ru, &on_break(Some(days), working(75.0, &[10], day(2026, 6, 16)))).map(|(title, _)| title);
    for (weeks, title) in [(4, "Перерыв 4 недели"), (5, "Перерыв 5 недель"), (21, "Перерыв 21 неделя"), (22, "Перерыв 22 недели"), (26, "Перерыв 26 недель")]
    {
        assert_eq!(break_title(weeks * 7 + 3), Some(title.to_owned()), "{weeks}");
    }
    let after = |days| texts(Lang::Ru, &after_break(days, None, working(70.0, &[8], day(2026, 8, 3)))).map(|(_, explanation)| explanation);
    for (weeks, text) in [(4, "в 4 недели"), (5, "в 5 недель"), (21, "в 21 неделю"), (22, "в 22 недели")] {
        let explanation = after(weeks * 7 + 1).expect("an explanation");
        assert!(explanation.contains(&format!("После перерыва {text} считаем заново.")), "{weeks}: {explanation}");
    }
    assert_eq!(texts(Lang::En, &on_break(Some(29), working(75.0, &[10], day(2026, 6, 16)))).map(|(title, _)| title), Some("Break 4 weeks".to_owned()));
}

#[test]
fn an_answer_that_lacks_a_part_still_reads() {
    let last = working(80.0, &[10], day(2026, 8, 18));
    // Add, with neither the weight to go to nor the two sessions it rests on.
    let bare = view_with(advice(AdviceKind::Add), last.clone());
    assert_eq!(
        texts(Lang::En, &bare),
        Some(("Can add weight".to_owned(), "80 kg × 10 two sessions in a row.".to_owned())),
        "the weight and the sessions are what the card rests on"
    );
    assert_eq!(texts(Lang::Ru, &bare), Some(("Можно добавлять вес".to_owned(), "80 кг × 10 два занятия подряд.".to_owned())));
    // Hold without the target of the program: what was done, and nothing to compare it with.
    let no_target = view_with(advice(AdviceKind::Hold), last);
    assert_eq!(texts(Lang::En, &no_target), Some(("Hold 80 kg".to_owned(), "Last time 80 kg × 10 (18 August).".to_owned())));
    assert_eq!(texts(Lang::Ru, &no_target), Some(("Держим 80 кг".to_owned(), "Последний раз 80 кг × 10 (18 августа).".to_owned())));
}

#[test]
fn an_exercise_without_history_has_no_advice() {
    let mut view = view_with(advice(AdviceKind::Hold), working(80.0, &[10], day(2026, 8, 18)));
    view.summary = None;
    assert_eq!(texts(Lang::En, &view), None);
    assert_eq!(subtitle(&view), None);
}

#[test]
fn the_subtitle_counts_the_sessions_and_names_the_target_and_the_program() {
    let mut view = hold(10, false, working(90.0, &[6], day(2026, 10, 20)));
    // Newest first: the program is the newest one that has a name.
    view.sessions = vec![session(day(2026, 10, 20), None), session(day(2026, 10, 1), Some("FB2")), session(day(2026, 9, 1), Some("FB1"))];
    let table = [
        (Lang::En, "14 sessions since March 2024 · target — 10 reps (program FB2)"),
        (Lang::Ru, "14 занятий с марта 2024 · цель — 10 повторов (по программе FB2)"),
    ];
    for (lang, text) in table {
        i18n::set_current(lang);
        assert_eq!(subtitle(&view).as_deref(), Some(text), "{lang:?}");
    }
    // No program has a name: the target stands alone, without parentheses.
    let mut nameless = view.clone();
    nameless.sessions = vec![session(day(2026, 10, 20), None)];
    i18n::set_current(Lang::En);
    assert_eq!(subtitle(&nameless).as_deref(), Some("14 sessions since March 2024 · target — 10 reps"));
    // No target: the program alone.
    let mut untargeted = view.clone();
    untargeted.summary.as_mut().expect("a summary").advice = advice(AdviceKind::NoTarget);
    assert_eq!(subtitle(&untargeted).as_deref(), Some("14 sessions since March 2024 · program FB2"));
    // Neither: the count alone.
    untargeted.sessions.clear();
    assert_eq!(subtitle(&untargeted).as_deref(), Some("14 sessions since March 2024"));
    // The forms of the count, and of the reps.
    let mut counted = view;
    for (sessions, target, en, ru) in [
        (1, 1, "1 session since March 2024 · target — 1 rep (program FB2)", "1 занятие с марта 2024 · цель — 1 повтор (по программе FB2)"),
        (3, 3, "3 sessions since March 2024 · target — 3 reps (program FB2)", "3 занятия с марта 2024 · цель — 3 повтора (по программе FB2)"),
        (21, 21, "21 sessions since March 2024 · target — 21 reps (program FB2)", "21 занятие с марта 2024 · цель — 21 повтор (по программе FB2)"),
    ] {
        let summary = counted.summary.as_mut().expect("a summary");
        summary.sessions = sessions;
        summary.advice.target = Some(target);
        for (lang, text) in [(Lang::En, en), (Lang::Ru, ru)] {
            i18n::set_current(lang);
            assert_eq!(subtitle(&counted).as_deref(), Some(text), "{lang:?}");
        }
    }
}

#[test]
fn the_sets_of_a_session_are_a_compact_line() {
    let set = |weight_kg: Option<f64>, reps: Option<u16>| SetBriefView { weight_kg, reps };
    for lang in Lang::ALL {
        i18n::set_current(lang);
        let comma = pick(lang, "77.5×8", "77,5×8");
        let line =
            sets_text(&[set(Some(40.0), Some(10)), set(Some(60.0), Some(10)), set(Some(80.0), Some(10)), set(Some(90.0), Some(6)), set(Some(77.5), Some(8))]);
        assert_eq!(line, format!("40×10 · 60×10 · 80×10 · 90×6 · {comma}"), "{lang:?}");
        // No weight: the reps alone. Nothing at all: left out.
        assert_eq!(sets_text(&[set(None, Some(12)), set(Some(0.0), Some(10)), set(None, None)]), "12 · 10", "{lang:?}");
        // A weight and no reps.
        assert_eq!(sets_text(&[set(Some(40.0), None)]), "40", "{lang:?}");
        assert_eq!(sets_text(&[]), "", "{lang:?}");
    }
}

// The page in the window.

type Workout = (&'static str, Option<&'static str>, Vec<RecordedSet>);

fn squat(kg: f64, reps: u32) -> RecordedSet {
    strength_set(0, SQUAT, kg, reps, Some(10))
}

/// Seven squat sessions every three weeks, and the last two four days apart: the advice is to add weight. In September a
/// heavier set for fewer reps than the program asks.
fn squats() -> Vec<Workout> {
    vec![
        ("2026-07-06 08:00:00", Some("Legs"), vec![squat(70.0, 10)]),
        ("2026-07-27 08:00:00", Some("Legs"), vec![squat(70.0, 10)]),
        ("2026-08-17 08:00:00", Some("Legs"), vec![squat(75.0, 10)]),
        ("2026-09-07 08:00:00", Some("Legs"), vec![squat(75.0, 10)]),
        ("2026-09-28 08:00:00", Some("Legs"), vec![squat(80.0, 10), strength_set(1, SQUAT, 90.0, 6, Some(10))]),
        ("2026-10-20 08:00:00", Some("Legs"), vec![squat(80.0, 10)]),
        ("2026-10-24 08:00:00", Some("Legs"), vec![squat(80.0, 10)]),
    ]
}

fn page(lang: Lang, workouts: &[Workout], size: egui::Vec2) -> Harness<'static, GymgarApp> {
    let (mut app, _handles) = app_with_workouts(lang, workouts);
    app.open_exercise(Some("squat".to_owned()));
    harness_at_size(app, size)
}

fn harness_at_size(app: GymgarApp, size: egui::Vec2) -> Harness<'static, GymgarApp> {
    let mut harness = harness_sized(app, size);
    harness.state_mut().ui.section = Section::Progress;
    harness.run();
    harness
}

/// The circles of the last frame: (fill, stroke colour) of each.
fn circles(harness: &Harness<'_, GymgarApp>) -> Vec<(egui::Color32, egui::Color32)> {
    harness
        .output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Circle(circle) => Some((circle.fill, circle.stroke.color)),
            _ => None,
        })
        .collect()
}

#[test]
fn detail_shows_advice_chart_and_sessions() {
    let t = Tokens::SPORT;
    for lang in Lang::ALL {
        let harness = page(lang, &squats(), egui::vec2(1200.0, 1400.0));
        harness.get_by_role_and_label(Role::Heading, "Squat");
        harness.get_by_label(pick(
            lang,
            "7 sessions since July 2026 · target — 10 reps (program Legs)",
            "7 занятий с июля 2026 · цель — 10 повторов (по программе Legs)",
        ));
        // The card: the advice, and why.
        harness.get_by_label(pick(lang, "Can add: 85 kg", "Можно добавлять: 85 кг"));
        harness.get_by_label(pick(
            lang,
            "80 kg × 10 two sessions in a row — 20 and 24 October. With 85 kg expect about 8 reps, then build back up to 10.",
            "80 кг × 10 два занятия подряд — 20 и 24 октября. С 85 кг ждите около 8 повторов, дальше снова до 10.",
        ));
        // The chart: a filled circle for each of the four months and one in the legend; the heavier set of September is a
        // hollow one, and so is its swatch in the legend.
        let shown = circles(&harness);
        assert_eq!(shown.iter().filter(|(fill, _)| *fill == t.accent).count(), 4 + 1, "{lang:?}: the months and the legend");
        assert_eq!(shown.iter().filter(|(fill, stroke)| *fill == egui::Color32::TRANSPARENT && *stroke == t.text_dim).count(), 1 + 1, "{lang:?}");
        harness.get_by_label(pick(lang, "Weight at 10+ reps", "Вес на 10+ повторов"));
        harness.get_by_label(pick(lang, "Heavier, fewer reps", "Тяжелее, но меньше повторов"));
        // The sessions, newest first, each with what it did.
        let dates = ["24 October", "20 October", "28 September", "7 September", "17 August", "27 July", "6 July"];
        let ru_dates = ["24 октября", "20 октября", "28 сентября", "7 сентября", "17 августа", "27 июля", "6 июля"];
        let tops: Vec<f32> = dates.iter().zip(ru_dates).map(|(en, ru)| harness.get_by_role_and_label(Role::Button, pick(lang, en, ru)).rect().top()).collect();
        assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{lang:?}: newest first: {tops:?}");
        // What each did: the working set of two of them, the sets of every one.
        let count = |label: &str| harness.query_all_by_label(label).count();
        assert_eq!(count(pick(lang, "80 kg × 10", "80 кг × 10")), 2, "{lang:?}: the last two sessions");
        assert_eq!(count(pick(lang, "90 kg × 6", "90 кг × 6")), 1, "{lang:?}: the session with the heavy set");
        assert_eq!((count("70×10"), count("75×10"), count("80×10"), count("80×10 · 90×6")), (2, 2, 2, 1), "{lang:?}");
        harness.get_by_role_and_label(Role::Heading, pick(lang, "Sessions", "Занятия"));
    }
}

#[test]
fn a_session_opens_its_workout_in_activities() {
    for lang in Lang::ALL {
        let mut harness = page(lang, &squats(), egui::vec2(1200.0, 1400.0));
        click(&mut harness, pick(lang, "20 October", "20 октября"));
        let state = harness.state();
        // The sixth workout of the list.
        assert_eq!((state.ui.section, state.ui.activities.selected), (Section::Activities, Some(7_100_006)), "{lang:?}");
        assert_eq!(state.detail().map(|detail| detail.summary.id.0), Some(7_100_006), "{lang:?}: the workout is read");
        // The exercise is still open on Progress, for the way back.
        assert_eq!(state.ui.progress.open.as_deref(), Some("squat"), "{lang:?}");
    }
}

#[test]
fn back_returns_to_the_tiles() {
    for lang in Lang::ALL {
        let mut harness = page(lang, &squats(), egui::vec2(1200.0, 800.0));
        assert!(harness.query_by_role_and_label(Role::Button, "Squat").is_none(), "{lang:?}: the page, not the tiles");
        click(&mut harness, pick(lang, "Back", "Назад"));
        assert_eq!(harness.state().ui.progress.open, None, "{lang:?}");
        assert_eq!(harness.state().progress_detail(), None, "{lang:?}");
        harness.get_by_role_and_label(Role::Button, "Squat");
        assert!(harness.query_by_label(pick(lang, "Can add: 85 kg", "Можно добавлять: 85 кг")).is_none(), "{lang:?}");
    }
}

#[test]
fn the_open_exercise_that_vanished_closes() {
    // A deadlift as well, so that there is a tile left to open.
    let deadlift = ExerciseId { category: 8, name: Some(23) };
    let mut workouts = squats();
    workouts.push(("2026-10-22 08:00:00", None, vec![strength_set(0, deadlift, 60.0, 10, Some(10))]));
    let (app, _handles) = app_with_workouts(Lang::En, &workouts);
    let mut harness = harness_at(app, Section::Progress);
    click(&mut harness, "Squat");
    assert_eq!(harness.state().ui.progress.open.as_deref(), Some("squat"));
    harness.get_by_label("Can add: 85 kg");
    // Every squat set becomes a deadlift set, one workout after the other.
    for id in 7_100_001..=7_100_007 {
        let both = if id == 7_100_005 { json!([{ "recorded": 0 }, { "recorded": 1 }]) } else { json!([{ "recorded": 0 }]) };
        harness.state_mut().run("activity.editSets", json!({ "id": id, "sets": both, "exercise": "deadlift/romanian_deadlift" })).expect("move the sets");
    }
    tick(&mut harness);
    assert_eq!(harness.state().ui.progress.open, None);
    assert_eq!(harness.state().error(), None, "closing a page that has nothing to show is not an error");
    // The tiles are back, and the squat is not one of them.
    harness.get_by_role_and_label(Role::Button, "Romanian deadlift");
    assert!(harness.query_by_role_and_label(Role::Button, "Squat").is_none());
}

#[test]
fn an_exercise_that_cannot_be_read_says_why_beside_the_way_back() {
    let (mut app, _handles) = app_with_workouts(Lang::En, &squats());
    // A key no exercise has: the read fails, and the window has not yet looked at the list again.
    app.open_exercise(Some("no_such_exercise".to_owned()));
    let why = app.progress_detail_error().expect("the read failed").to_owned();
    let harness = harness_at_size(app, egui::vec2(1200.0, 800.0));
    harness.get_by_label(&why);
    harness.get_by_role_and_label(Role::Button, "Back");
}

#[test]
fn a_page_that_is_still_being_read_shows_the_way_back_and_the_name() {
    let (mut app, _handles) = app_with_workouts(Lang::En, &squats());
    app.ui.progress.open = Some("squat".to_owned());
    // Nothing has read the page yet (the window reads it when the user opens it, or on the next refresh).
    let harness = harness_at_size(app, egui::vec2(1200.0, 800.0));
    assert_eq!(harness.state().progress_detail(), None);
    harness.get_by_role_and_label(Role::Heading, "Squat");
    harness.get_by_role_and_label(Role::Button, "Back");
}

fn ui_page(view: ProgressView, size: egui::Vec2) -> Harness<'static, ProgressView> {
    let mut harness =
        crate::test_support::builder().with_size(size).build_ui_state(|ui, view: &mut ProgressView| crate::progress_chart::show(ui, view, 220.0), view);
    harness.run();
    harness
}

fn point(month: &str, weight_kg: f64, reps: u16, date: NaiveDate) -> PointView {
    PointView { month: month.to_owned(), weight_kg, reps, date, activity_id: ActivityId(1) }
}

/// A chart of `points` and `heavier` with `gaps`, for the exercise `view_with` builds.
fn chart_of(points: Vec<PointView>, heavier: Vec<PointView>, gaps: Vec<GapView>) -> ProgressView {
    ProgressView { points, heavier, gaps, ..view_with(advice(AdviceKind::Hold), working(80.0, &[10], day(2026, 10, 20))) }
}

#[test]
fn chart_draws_odd_series() {
    let t = Tokens::SPORT;
    let filled = |harness: &Harness<'_, ProgressView>| {
        harness.output().shapes.iter().filter(|clipped| matches!(&clipped.shape, Shape::Circle(circle) if circle.fill == t.accent)).count()
    };
    let hollow = |harness: &Harness<'_, ProgressView>| {
        harness
            .output()
            .shapes
            .iter()
            .filter(|clipped| matches!(&clipped.shape, Shape::Circle(circle) if circle.fill == egui::Color32::TRANSPARENT && circle.stroke.color == t.text_dim))
            .count()
    };
    let size = egui::vec2(900.0, 400.0);
    for lang in Lang::ALL {
        i18n::set_current(lang);
        // One point: a dot, in a chart that has room for the months that will come.
        let harness = ui_page(chart_of(vec![point("2026-10", 80.0, 10, day(2026, 10, 20))], Vec::new(), Vec::new()), size);
        assert_eq!((filled(&harness), hollow(&harness)), (1 + 1, 0), "{lang:?}: one point and the legend's");
        // Only circles: nothing reached the target, and the heaviest sets of two months are drawn.
        let harness =
            ui_page(chart_of(Vec::new(), vec![point("2026-09", 90.0, 6, day(2026, 9, 28)), point("2026-10", 95.0, 5, day(2026, 10, 20))], Vec::new()), size);
        assert_eq!((filled(&harness), hollow(&harness)), (1, 2 + 1), "{lang:?}: the legend has only what is drawn");
        // The chart of reps: the exercise never had a weight.
        let mut reps = chart_of(
            vec![point("2026-08", 0.0, 8, day(2026, 8, 3)), point("2026-09", 0.0, 10, day(2026, 9, 3)), point("2026-10", 0.0, 12, day(2026, 10, 20))],
            Vec::new(),
            Vec::new(),
        );
        reps.summary.as_mut().expect("a summary").chart_reps = None;
        let harness = ui_page(reps, size);
        assert_eq!(filled(&harness), 3 + 1, "{lang:?}");
        harness.get_by_label(pick(lang, "Best reps of the month", "Лучшие повторы месяца"));
        // A break that is going on: shaded to today, with no point after it.
        let harness = ui_page(
            chart_of(vec![point("2026-06", 75.0, 10, day(2026, 6, 16))], Vec::new(), vec![GapView { from: day(2026, 6, 16), to: None, days: 70 }]),
            size,
        );
        let shaded = |harness: &Harness<'_, ProgressView>| {
            harness
                .output()
                .shapes
                .iter()
                .filter(|clipped| matches!(&clipped.shape, Shape::Rect(rect) if rect.fill == t.warning.gamma_multiply(metrics::GAP_OPACITY)))
                .count()
        };
        assert_eq!(shaded(&harness), 1 + 1, "{lang:?}: the break and the legend's swatch");
        harness.get_by_label(pick(lang, "Break", "Перерыв"));
        // A break between two points, and a month that cannot be read: left out, not a panic.
        let harness = ui_page(
            chart_of(
                vec![point("2026-06", 75.0, 10, day(2026, 6, 16)), point("garbage", 99.0, 10, day(2026, 7, 1)), point("2026-09", 80.0, 10, day(2026, 9, 5))],
                Vec::new(),
                vec![GapView { from: day(2026, 6, 16), to: Some(day(2026, 9, 5)), days: 81 }],
            ),
            size,
        );
        assert_eq!((filled(&harness), shaded(&harness)), (2 + 1, 1 + 1), "{lang:?}");
        // Nothing to draw: a line that says so.
        let harness = ui_page(chart_of(Vec::new(), Vec::new(), Vec::new()), size);
        harness.get_by_label(pick(lang, "Nothing to chart yet", "Пока нечего показать на графике"));
        assert_eq!(filled(&harness), 0, "{lang:?}");
    }
}

#[test]
fn chart_survives_values_that_are_not_numbers() {
    let mut view = chart_of(
        vec![
            point("2026-08", f64::NAN, 10, day(2026, 8, 3)),
            point("2026-09", f64::INFINITY, 10, day(2026, 9, 3)),
            point("2026-10", 80.0, 10, day(2026, 10, 3)),
        ],
        vec![point("2026-10", f64::NEG_INFINITY, 4, day(2026, 10, 3))],
        vec![GapView { from: day(2026, 8, 3), to: Some(day(2026, 10, 3)), days: i64::MAX }],
    );
    view.summary.as_mut().expect("a summary").chart_reps = Some(10);
    let harness = ui_page(view, egui::vec2(900.0, 400.0));
    let drawn = harness.output().shapes.iter().filter(|clipped| matches!(&clipped.shape, Shape::Circle(circle) if circle.fill == Tokens::SPORT.accent)).count();
    assert_eq!(drawn, 1 + 1, "the one finite point and the legend's swatch");
}

#[test]
fn hovering_a_point_says_the_day_the_weight_and_the_reps() {
    let view = chart_of(
        vec![point("2026-08", 80.0, 10, day(2026, 8, 18)), point("2026-09", 82.5, 10, day(2026, 9, 20))],
        vec![point("2026-09", 90.0, 6, day(2026, 9, 28))],
        Vec::new(),
    );
    let mut harness = ui_harness(|ui, view: &mut ProgressView| crate::progress_chart::show(ui, view, 220.0), view);
    harness.run();
    // The second filled circle is the September point; the legend's swatch is the last one.
    let centers: Vec<egui::Pos2> = harness
        .output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Circle(circle) if circle.fill == Tokens::SPORT.accent => Some(circle.center),
            _ => None,
        })
        .collect();
    let (_august, september) = (centers.first().copied().expect("a point"), centers.get(1).copied().expect("a second point"));
    for (lang, text) in [(Lang::En, "20 September · 82.5 kg × 10"), (Lang::Ru, "20 сентября · 82,5 кг × 10")] {
        i18n::set_current(lang);
        harness.hover_at(september);
        harness.run();
        harness.get_by_label(text);
    }
}
