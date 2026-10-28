use chrono::{NaiveDate, TimeZone, Utc};
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use gymgar_domain::{ActivityId, ExerciseId, RecordedSet};
use gymgar_engine::testing::{TestConfig, strength_set};
use gymgar_engine::{AdviceKind, AdviceView, ExerciseView, PointView, ProgressItemView, WorkingView};

use super::*;
use crate::i18n::{self, Lang};
use crate::state::{ProgressFilter, Section};
use crate::test_support::{app_in, app_seeded, app_with_workouts, click, harness_at, harness_sized, pick, seed_titled_workout};
use crate::theme::Tokens;

const SQUAT: ExerciseId = ExerciseId { category: 28, name: None };
const BENCH: ExerciseId = ExerciseId { category: 0, name: None };
const DEADLIFT: ExerciseId = ExerciseId { category: 8, name: None };
const CURL: ExerciseId = ExerciseId { category: 7, name: None };
const ROW: ExerciseId = ExerciseId { category: 23, name: None };

type Workout = (&'static str, Option<&'static str>, Vec<RecordedSet>);

/// One working set of an exercise: 0 is its message index, `plan` the reps the program asked for.
fn set(exercise: ExerciseId, kg: f64, reps: u32, plan: Option<u32>) -> RecordedSet {
    strength_set(0, exercise, kg, reps, plan)
}

/// Five exercises on 2026-10-27, one for each way a tile can look, listed in the order the tiles come in: a squat done twice
/// at 80 kg for the 10 reps asked (add weight), a bench press of 95 kg for 8 of 10 (hold), a deadlift whose program asked for
/// nothing (no target), a curl last done 40 days ago (a break) and a row 200 days ago (a break, and dormant).
fn five() -> Vec<Workout> {
    vec![
        ("2026-04-10 08:00:00", Some("Back"), vec![set(ROW, 55.0, 10, Some(10))]),
        ("2026-09-17 08:00:00", Some("Arms"), vec![set(CURL, 45.0, 10, Some(10))]),
        ("2026-10-20 07:55:00", Some("Legs"), vec![set(SQUAT, 80.0, 10, Some(10))]),
        ("2026-10-22 07:55:00", None, vec![set(DEADLIFT, 60.0, 10, None)]),
        ("2026-10-24 07:55:00", Some("Legs"), vec![set(SQUAT, 80.0, 10, Some(10))]),
        ("2026-10-26 07:55:00", Some("Push"), vec![set(BENCH, 95.0, 8, Some(10))]),
    ]
}

/// The English names of the five, in the order of the tiles.
const NAMES: [&str; 5] = ["Squat", "Bench press", "Deadlift", "Curl", "Row"];

fn tiles(lang: Lang) -> Harness<'static, GymgarApp> {
    let (app, _handles) = app_with_workouts(lang, &five());
    harness_at(app, Section::Progress)
}

/// Which of `texts` the screen shows as labels, in the order the accessibility tree lists them (the order they are drawn).
fn in_tree_order(harness: &Harness<'_, GymgarApp>, texts: &[&str]) -> Vec<String> {
    // A text is a node's label or, for the nodes that are text, its value.
    let text = |node: egui_kittest::Node<'_>| node.accesskit_node().label().or_else(|| node.accesskit_node().value());
    harness.root().children_recursive().filter_map(text).filter(|label| texts.contains(&label.as_str())).collect()
}

fn shown_names(harness: &Harness<'_, GymgarApp>) -> Vec<&'static str> {
    NAMES.into_iter().filter(|name| harness.query_by_role_and_label(Role::Button, name).is_some()).collect()
}

#[test]
fn tiles_show_every_status() {
    for lang in Lang::ALL {
        let harness = tiles(lang);
        let statuses = [
            pick(lang, "can add 85 kg", "можно 85 кг"),
            pick(lang, "hold · 8 of 10", "держим · 8 из 10"),
            pick(lang, "no target", "нет цели"),
            pick(lang, "break · 5 wk", "перерыв · 5 нед."),
            pick(lang, "break · 28 wk", "перерыв · 28 нед."),
        ];
        // One tile of each, in the order of the answer: add, hold, no target, the break, then the dormant one last.
        assert_eq!(in_tree_order(&harness, &statuses), statuses, "{lang:?}");
        assert_eq!(shown_names(&harness), NAMES, "{lang:?}");
        // Under each name, the weight and reps of its last session.
        let working = [
            pick(lang, "80 kg × 10", "80 кг × 10"),
            pick(lang, "95 kg × 8", "95 кг × 8"),
            pick(lang, "60 kg × 10", "60 кг × 10"),
            pick(lang, "45 kg × 10", "45 кг × 10"),
            pick(lang, "55 kg × 10", "55 кг × 10"),
        ];
        assert_eq!(in_tree_order(&harness, &working), working, "{lang:?}");
    }
}

#[test]
fn only_the_tile_that_asks_to_add_has_the_accent_border() {
    let harness = tiles(Lang::En);
    let t = Tokens::SPORT;
    let accented = harness
        .output()
        .shapes
        .iter()
        .filter(|clipped| matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.fill == t.card && rect.stroke.color == t.accent))
        .count();
    assert_eq!(accented, 1, "one of the five tiles asks to add weight");
}

#[test]
fn the_tile_that_asks_to_add_has_a_chevron_before_its_status_and_the_others_have_none() {
    for lang in Lang::ALL {
        let harness = tiles(lang);
        let accent = Tokens::SPORT.accent;
        let chevrons: Vec<Vec<egui::Pos2>> = harness
            .output()
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Path(path) if path.points.len() == 3 && path.stroke.color == egui::epaint::ColorMode::Solid(accent) => Some(path.points.clone()),
                _ => None,
            })
            .collect();
        let status = harness.get_by_label(pick(lang, "can add 85 kg", "можно 85 кг")).rect();
        let [chevron] = chevrons.as_slice() else { panic!("{lang:?}: one chevron expected, got {chevrons:?}") };
        assert!(
            chevron.iter().all(|point| point.x < status.left() && point.y > status.top() && point.y < status.bottom()),
            "{lang:?}: {chevron:?} vs {status:?}"
        );
    }
}

#[test]
fn the_dormant_tile_is_dimmed() {
    let harness = tiles(Lang::En);
    let t = Tokens::SPORT;
    let faded = |color: egui::Color32| color.gamma_multiply(metrics::DIMMED_OPACITY);
    let rects: Vec<_> = harness
        .output()
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => Some((rect.fill, rect.stroke.color)),
            _ => None,
        })
        .collect();
    assert_eq!(rects.iter().filter(|rect| **rect == (faded(t.card), faded(t.border))).count(), 1, "the row nobody has done for half a year");
    assert_eq!(rects.iter().filter(|rect| **rect == (t.card, t.border)).count(), 3, "the other three that do not add");
}

#[test]
fn chips_count_and_filter() {
    for lang in Lang::ALL {
        let mut harness = tiles(lang);
        let all = pick(lang, "All", "Все");
        let add = pick(lang, "Can add · {}", "Можно добавлять · {}").replace("{}", "1");
        // The dormant row counts with the breaks.
        let on_break = pick(lang, "Break · {}", "Перерыв · {}").replace("{}", "2");
        let toggled =
            |harness: &Harness<'_, GymgarApp>, label: &str| format!("{:?}", harness.get_by_role_and_label(Role::Button, label).accesskit_node().toggled());
        assert_eq!(toggled(&harness, all), "Some(True)", "{lang:?}");
        assert_eq!((toggled(&harness, &add).as_str(), toggled(&harness, &on_break).as_str()), ("Some(False)", "Some(False)"), "{lang:?}");

        click(&mut harness, &on_break);
        assert_eq!(harness.state().ui.progress.filter, ProgressFilter::Break, "{lang:?}");
        assert_eq!(shown_names(&harness), ["Curl", "Row"], "{lang:?}: both breaks, the dormant one too");
        assert_eq!(toggled(&harness, &on_break), "Some(True)", "{lang:?}");

        click(&mut harness, &add);
        assert_eq!(harness.state().ui.progress.filter, ProgressFilter::Add, "{lang:?}");
        assert_eq!(shown_names(&harness), ["Squat"], "{lang:?}");

        click(&mut harness, all);
        assert_eq!(harness.state().ui.progress.filter, ProgressFilter::All, "{lang:?}");
        assert_eq!(shown_names(&harness), NAMES, "{lang:?}");
    }
}

#[test]
fn the_filter_is_part_of_the_ui_state_that_is_kept() {
    let mut harness = tiles(Lang::En);
    click(&mut harness, "Break · 2");
    let kept = serde_json::to_value(&harness.state().ui).expect("the state serializes");
    assert_eq!(kept.pointer("/progress/filter"), Some(&serde_json::json!("break")));
    // A window that opens with it shows the tiles of the filter at once.
    let (mut app, _handles) = app_with_workouts(Lang::En, &five());
    app.ui.progress.filter = ProgressFilter::Add;
    let harness = harness_at(app, Section::Progress);
    assert_eq!(shown_names(&harness), ["Squat"]);
}

#[test]
fn a_filter_that_shows_nothing_says_so() {
    // Only a bench press: nothing asks to add weight, but the chip is there and was chosen last time.
    let workouts = vec![("2026-10-26 07:55:00", None, vec![set(BENCH, 95.0, 8, Some(10))])];
    for lang in Lang::ALL {
        let (mut app, _handles) = app_with_workouts(lang, &workouts);
        app.ui.progress.filter = ProgressFilter::Add;
        let harness = harness_at(app, Section::Progress);
        harness.get_by_label(pick(lang, "No exercises here", "Здесь нет упражнений"));
        harness.get_by_role_and_label(Role::Button, &pick(lang, "Can add · {}", "Можно добавлять · {}").replace("{}", "0"));
        assert!(shown_names(&harness).is_empty(), "{lang:?}");
    }
}

#[test]
fn a_tile_opens_its_exercise_and_back_returns_to_the_tiles() {
    for lang in Lang::ALL {
        let mut harness = tiles(lang);
        click(&mut harness, "Squat");
        assert_eq!(harness.state().ui.progress.open.as_deref(), Some("squat"), "{lang:?}");
        // Read at once, for the page that replaces the tiles.
        assert_eq!(harness.state().progress_detail().map(|view| view.exercise.id.as_str()), Some("squat"), "{lang:?}");
        // The page is open instead of the tiles: its title and the way back.
        harness.get_by_role_and_label(Role::Heading, "Squat");
        assert!(harness.query_by_role_and_label(Role::Button, "Bench press").is_none(), "{lang:?}: the tiles are gone");

        click(&mut harness, pick(lang, "Back", "Назад"));
        assert_eq!(harness.state().ui.progress.open, None, "{lang:?}");
        assert_eq!(shown_names(&harness), NAMES, "{lang:?}");
        // The tile is a toggle-less button that a screen reader hears with what it says.
        let description = harness.get_by_role_and_label(Role::Button, "Squat").accesskit_node().description();
        assert_eq!(description.as_deref(), Some(pick(lang, "80 kg × 10 · can add 85 kg", "80 кг × 10 · можно 85 кг")), "{lang:?}");
    }
}

#[test]
fn without_workouts_the_screen_says_what_will_appear() {
    for lang in Lang::ALL {
        let text = pick(lang, "Progress will appear here once strength workouts are loaded", "Здесь появится прогресс, когда загрузятся силовые тренировки");
        let settings = pick(lang, "Open Settings", "Открыть настройки");

        // Garmin is connected: the workouts are on their way, and there is nothing to do about it here.
        let (app, _handles) = app_with_workouts(lang, &[]);
        let harness = harness_at(app, Section::Progress);
        harness.get_by_label(text);
        assert!(harness.query_by_role_and_label(Role::Link, settings).is_none(), "{lang:?}");
        // No chips over nothing.
        assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "All", "Все")).is_none(), "{lang:?}");

        // Garmin is not connected: the way to connect it as well.
        let (app, _handles) = app_in(lang, TestConfig::default());
        let mut harness = harness_at(app, Section::Progress);
        harness.get_by_label(text);
        harness.get_by_role_and_label(Role::Link, settings).click();
        harness.run();
        assert_eq!(harness.state().ui.section, Section::Settings, "{lang:?}");
    }
}

#[test]
fn the_grid_has_as_many_columns_as_fit() {
    // The narrowest tile is 230 points, and 8 lie between two.
    for (width, columns) in [(0.0, 1), (-5.0, 1), (100.0, 1), (230.0, 1), (467.0, 1), (468.0, 2), (700.0, 2), (706.0, 3), (1096.0, 4), (5000.0, 21)] {
        assert_eq!(columns_for(width), columns, "{width}");
    }
    assert_eq!(columns_for(f32::NAN), 1);
    assert_eq!(columns_for(f32::INFINITY), 1, "a width that is not one");
}

#[test]
fn tiles_fill_the_window_in_rows_and_stay_inside_it() {
    for (size, first_row) in [(egui::vec2(1200.0, 800.0), 4), (egui::vec2(800.0, 500.0), 2), (egui::vec2(380.0, 800.0), 1)] {
        let (app, _handles) = app_with_workouts(Lang::En, &five());
        let mut harness = harness_sized(app, size);
        harness.state_mut().ui.section = Section::Progress;
        harness.run();
        let rects: Vec<egui::Rect> = NAMES.iter().map(|name| harness.get_by_role_and_label(Role::Button, name).rect()).collect();
        let first_top = rects.first().map(|rect| rect.top()).expect("a tile");
        assert_eq!(rects.iter().take_while(|rect| rect.top() == first_top).count(), first_row, "{size:?}: {rects:?}");
        // As wide as each other, and none runs out of the window.
        let widths: Vec<f32> = rects.iter().map(|rect| rect.width().round()).collect();
        assert!(widths.windows(2).all(|pair| pair[0] == pair[1]), "{size:?}: {widths:?}");
        assert!(rects.iter().all(|rect| rect.right() <= size.x), "{size:?}: {rects:?}");
        assert!(rects.iter().all(|rect| rect.width() >= 200.0), "{size:?}: {rects:?}");
        // Tiles of a row are as high as each other.
        let heights: Vec<f32> = rects.iter().map(|rect| rect.height().round()).collect();
        assert!(heights.windows(2).all(|pair| pair[0] == pair[1]), "{size:?}: {heights:?}");
    }
}

#[test]
fn a_long_name_is_cut_inside_its_tile() {
    // The watch's own title for a squat, as long as the longest names Garmin's programs have.
    let title = "Приседания со штангой на спине с паузой внизу и подъёмом на носки, узкая постановка ног".to_owned();
    let (mut app, _handles) = app_seeded(Lang::Ru, TestConfig::default(), true, |store| {
        let start = Utc.with_ymd_and_hms(2026, 10, 24, 6, 0, 0).single().expect("a valid time");
        seed_titled_workout(store, 7_200_001, start, vec![set(SQUAT, 80.0, 10, Some(10))], vec![(SQUAT, title.clone())]);
        seed_titled_workout(store, 7_200_002, start, vec![set(BENCH, 95.0, 8, Some(10))], Vec::new());
    });
    app.ui.section = Section::Progress;
    let mut harness = harness_sized(app, egui::vec2(800.0, 500.0));
    harness.run();
    let tile = harness.get_by_role_and_label(Role::Button, &title).rect();
    let name = harness.get_by_role_and_label(Role::Label, &title).rect();
    assert!(name.right() <= tile.right(), "the name runs out of its tile: {name:?} in {tile:?}");
    // One line, like every other name: as high as the line of a short name.
    let short = harness.get_by_role_and_label(Role::Label, "Bench press").rect();
    assert_eq!(name.height().round(), short.height().round(), "the name wraps");
}

// The strings of a tile, as a table in both languages.

fn working(weight_kg: f64, reps: &[u16]) -> WorkingView {
    WorkingView { weight_kg, reps: reps.to_vec(), date: NaiveDate::from_ymd_opt(2026, 10, 20).expect("a date"), activity_id: ActivityId(1) }
}

fn advice(kind: AdviceKind) -> AdviceView {
    AdviceView { kind, target: None, next_weight_kg: None, sessions_at_weight: None, met_once: false, basis: Vec::new(), break_days: None, before_break: None }
}

fn item(advice: AdviceView, last: WorkingView) -> ProgressItemView {
    ProgressItemView {
        exercise: ExerciseView { id: "squat".to_owned(), english: "Squat".to_owned(), watch_title: None },
        advice,
        last,
        sessions: 3,
        first_date: NaiveDate::from_ymd_opt(2026, 9, 1).expect("a date"),
        dormant: false,
        chart_reps: Some(10),
        sparkline: Vec::new(),
    }
}

/// A row of the table of `working_text`: the weight, the reps, what the chart is drawn for, and the text in English and in Russian.
type WorkingRow = (f64, &'static [u16], Option<u16>, &'static str, &'static str);

#[test]
fn working_text_is_weight_by_reps_or_just_reps() {
    let table: [WorkingRow; 14] = [
        (80.0, &[10], Some(10), "80 kg × 10", "80 кг × 10"),
        (100.0, &[8, 8, 6], Some(8), "100 kg × 8, 8, 6", "100 кг × 8, 8, 6"),
        (77.5, &[10], Some(10), "77.5 kg × 10", "77,5 кг × 10"),
        (82.25, &[9, 8], None, "82.25 kg × 9, 8", "82,25 кг × 9, 8"),
        // No weight: reps, with the unit in the form of the last number.
        (0.0, &[1], None, "1 rep", "1 повтор"),
        (0.0, &[3], None, "3 reps", "3 повтора"),
        (0.0, &[12], None, "12 reps", "12 повторов"),
        (0.0, &[21], None, "21 reps", "21 повтор"),
        (0.0, &[15, 15, 12], None, "15, 15, 12 reps", "15, 15, 12 повторов"),
        (0.0, &[12, 12, 22], None, "12, 12, 22 reps", "12, 12, 22 повтора"),
        // Without weight whatever the chart is drawn for (the exercise had a weight before).
        (0.0, &[10], Some(10), "10 reps", "10 повторов"),
        // Nothing to show of the reps.
        (80.0, &[], Some(10), "80 kg", "80 кг"),
        (0.0, &[], None, "—", "—"),
        (f64::NAN, &[10], None, "10 reps", "10 повторов"),
    ];
    for lang in Lang::ALL {
        i18n::set_current(lang);
        for (weight, reps, chart_reps, en, ru) in table {
            assert_eq!(working_text(&working(weight, reps), chart_reps), pick(lang, en, ru), "{lang:?}: {weight} × {reps:?}");
        }
    }
}

#[test]
fn advice_short_says_one_thing_per_status() {
    let hold = |target: u16, met_once: bool, break_days: Option<i64>| AdviceView {
        target: Some(target),
        sessions_at_weight: Some(2),
        met_once,
        break_days,
        ..advice(AdviceKind::Hold)
    };
    let table: Vec<(ProgressItemView, &str, &str)> = vec![
        (item(AdviceView { target: Some(10), next_weight_kg: Some(85.0), ..advice(AdviceKind::Add) }, working(80.0, &[10])), "can add 85 kg", "можно 85 кг"),
        (item(AdviceView { next_weight_kg: Some(82.5), ..advice(AdviceKind::Add) }, working(80.0, &[10])), "can add 82.5 kg", "можно 82,5 кг"),
        // The fewest reps of the last session over the target.
        (item(hold(10, false, None), working(90.0, &[8, 8, 6])), "hold · 6 of 10", "держим · 6 из 10"),
        (item(hold(8, false, None), working(100.0, &[7])), "hold · 7 of 8", "держим · 7 из 8"),
        (item(hold(10, true, None), working(80.0, &[10, 12])), "hold · one more session", "держим · ещё одно занятие"),
        // After a break, whatever else is true: the session starts the count anew.
        (item(hold(10, false, Some(63)), working(70.0, &[8])), "after a break", "после перерыва"),
        (item(hold(10, true, Some(63)), working(70.0, &[10])), "after a break", "после перерыва"),
        (item(AdviceView { break_days: Some(70), ..advice(AdviceKind::Break) }, working(75.0, &[10])), "break · 10 wk", "перерыв · 10 нед."),
        (item(AdviceView { break_days: Some(34), ..advice(AdviceKind::Break) }, working(75.0, &[10])), "break · 4 wk", "перерыв · 4 нед."),
        (item(advice(AdviceKind::Bodyweight), working(0.0, &[12])), "bodyweight", "без веса"),
        (item(advice(AdviceKind::NoTarget), working(60.0, &[10])), "no target", "нет цели"),
        // An answer that lacks what its status rests on still says something.
        (item(advice(AdviceKind::Add), working(80.0, &[10])), "can add", "можно добавлять"),
        (item(advice(AdviceKind::Hold), working(80.0, &[10])), "hold", "держим"),
        (item(hold(10, false, None), working(80.0, &[])), "hold", "держим"),
        (item(advice(AdviceKind::Break), working(80.0, &[10])), "break", "перерыв"),
    ];
    for lang in Lang::ALL {
        i18n::set_current(lang);
        for (item, en, ru) in &table {
            assert_eq!(advice_short(item), pick(lang, en, ru), "{lang:?}: {:?}", item.advice);
        }
    }
}

#[test]
fn the_tone_follows_the_advice() {
    for (kind, tone) in [
        (AdviceKind::Add, Tone::Accent),
        (AdviceKind::Hold, Tone::Dim),
        (AdviceKind::Bodyweight, Tone::Dim),
        (AdviceKind::NoTarget, Tone::Dim),
        (AdviceKind::Break, Tone::Warning),
    ] {
        assert_eq!(advice_tone(kind), tone, "{kind:?}");
    }
}

#[test]
fn the_sparkline_of_a_tile_is_the_chart_of_its_exercise() {
    let point = |month: &str, weight_kg: f64, reps: u16| PointView {
        month: month.to_owned(),
        weight_kg,
        reps,
        date: NaiveDate::from_ymd_opt(2026, 8, 1).expect("a date"),
        activity_id: ActivityId(1),
    };
    // A chart of weight at some reps: the weights.
    let weights = ProgressItemView {
        sparkline: vec![point("2026-07", 70.0, 10), point("2026-08", 75.0, 10)],
        chart_reps: Some(10),
        ..item(advice(AdviceKind::Hold), working(75.0, &[10]))
    };
    assert_eq!(sparkline_values(&weights), [70.0, 75.0]);
    // A chart of reps (the exercise never had a weight): the reps.
    let reps = ProgressItemView {
        sparkline: vec![point("2026-07", 0.0, 12), point("2026-08", 0.0, 15)],
        chart_reps: None,
        ..item(advice(AdviceKind::Bodyweight), working(0.0, &[15]))
    };
    assert_eq!(sparkline_values(&reps), [12.0, 15.0]);
    // An exercise nobody has done for a year has no points; its tile is as high as any other.
    assert!(sparkline_values(&item(advice(AdviceKind::Break), working(75.0, &[10]))).is_empty());
}

#[test]
fn a_dormant_exercise_without_a_chart_still_gets_its_tile() {
    // The row of `five` was last done 200 days ago: more than a year ago it would have no points at all.
    let workouts = vec![("2025-06-10 08:00:00", None, vec![set(ROW, 55.0, 10, Some(10))]), ("2026-10-26 07:55:00", None, vec![set(BENCH, 95.0, 8, Some(10))])];
    let (app, _handles) = app_with_workouts(Lang::En, &workouts);
    let harness = harness_at(app, Section::Progress);
    assert!(harness.state().progress().iter().any(|item| item.dormant && item.sparkline.is_empty()), "a dormant exercise with an empty chart");
    let rects: Vec<egui::Rect> = ["Bench press", "Row"].iter().map(|name| harness.get_by_role_and_label(Role::Button, name).rect()).collect();
    assert_eq!(rects.iter().map(|rect| rect.height().round()).collect::<Vec<_>>().windows(2).filter(|pair| pair[0] != pair[1]).count(), 0, "{rects:?}");
}
