use chrono::NaiveDate;
use egui_plot::PlotPoint;
use gymgar_domain::ActivityId;
use gymgar_engine::{AdviceKind, AdviceView, ExerciseView, ProgressItemView, SessionView, WorkingView};

use super::*;
use crate::i18n::{self, Lang};

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("a date")
}

fn point(month: &str, weight_kg: f64, reps: u16, date: NaiveDate) -> PointView {
    PointView { month: month.to_owned(), weight_kg, reps, date, activity_id: ActivityId(1) }
}

fn gap(from: NaiveDate, to: Option<NaiveDate>, days: i64) -> GapView {
    GapView { from, to, days }
}

fn working(date: NaiveDate) -> WorkingView {
    WorkingView { weight_kg: 80.0, reps: vec![10], date, activity_id: ActivityId(1) }
}

/// The answer for an exercise with `points`, `heavier` and `gaps`; `reps` makes it a chart of reps.
fn view(points: Vec<PointView>, heavier: Vec<PointView>, gaps: Vec<GapView>, reps: bool) -> ProgressView {
    let exercise = ExerciseView { id: "squat".to_owned(), english: "Squat".to_owned(), watch_title: None };
    let advice = AdviceView {
        kind: AdviceKind::Hold,
        target: Some(10),
        next_weight_kg: None,
        sessions_at_weight: None,
        met_once: false,
        basis: Vec::new(),
        break_days: None,
        before_break: None,
    };
    let last = working(day(2026, 10, 20));
    let summary = ProgressItemView {
        exercise: exercise.clone(),
        advice,
        last,
        sessions: 3,
        first_date: day(2026, 6, 1),
        dormant: false,
        chart_reps: if reps { None } else { Some(10) },
        sparkline: Vec::new(),
    };
    ProgressView { exercise, summary: Some(summary), step_kg: None, points, heavier, gaps, sessions: Vec::new() }
}

#[test]
fn a_month_is_a_count_of_months_and_only_a_month_is_one() {
    assert_eq!(month_index("2026-08"), Some(2026 * 12 + 7));
    assert_eq!(month_index("2026-01"), Some(2026 * 12));
    assert_eq!(month_index("2026-12"), Some(2026 * 12 + 11));
    assert_eq!(month_index("2027-01").zip(month_index("2026-12")).map(|(next, last)| next - last), Some(1), "months follow each other");
    for text in ["", "2026", "2026-00", "2026-13", "garbage", "-5-03", "2026-ab", "2026-", "x-08", "2026-08-01"] {
        assert_eq!(month_index(text), None, "{text:?}");
    }
}

#[test]
fn a_day_has_its_place_inside_the_slot_of_its_month() {
    let slot = |day: NaiveDate| x_of_date(day) - month_of(day) as f64;
    // The first day is near the left edge of the slot, the last near the right edge, and the days between are in order.
    assert!((slot(day(2026, 8, 1)) - (-0.5 + 0.5 / 31.0)).abs() < 1e-9);
    assert!((slot(day(2026, 8, 31)) - (0.5 - 0.5 / 31.0)).abs() < 1e-9);
    assert!(x_of_date(day(2026, 8, 31)) < x_of_date(day(2026, 9, 1)), "the next month's first day is after this month's last");
    assert!(x_of_date(day(2026, 8, 17)) < x_of_date(day(2026, 8, 18)));
    // The slots of the months of different lengths have the same width.
    for (date, days) in [(day(2024, 2, 10), 29.0), (day(2026, 2, 10), 28.0), (day(2026, 4, 10), 30.0), (day(2026, 12, 10), 31.0)] {
        assert_eq!(days_in_month(date), days, "{date}");
        let first = date.with_day(1).expect("the first of the month");
        assert!((slot(first) - (-0.5 + 0.5 / days)).abs() < 1e-9, "{date}");
    }
}

#[test]
fn a_break_crossing_two_points_dashes_the_line_between_them() {
    let points = vec![
        point("2026-06", 70.0, 10, day(2026, 6, 5)),
        point("2026-07", 72.5, 10, day(2026, 7, 5)),
        point("2026-10", 75.0, 10, day(2026, 10, 12)),
        point("2026-11", 77.5, 10, day(2026, 11, 3)),
    ];
    // Between the last session of July and the first of October, no point in August or September.
    let spans: Vec<GapSpan> = [gap(day(2026, 7, 25), Some(day(2026, 10, 8)), 75)].iter().filter_map(gap_span).collect();
    let marks = marks(&points, false);
    let lines = segments(&marks, &spans);
    let shape: Vec<(usize, bool)> = lines.iter().map(|segment| (segment.points.len(), segment.dashed)).collect();
    assert_eq!(shape, [(2, false), (2, true), (2, false)]);
    // The dashed stretch runs from the July point to the October one.
    let dashed = lines.get(1).expect("the dashed stretch");
    assert_eq!(dashed.points, [[marks.get(1).map_or(0.0, |m| m.x), 72.5], [marks.get(2).map_or(0.0, |m| m.x), 75.0]]);
    // No break: one solid line through everything.
    let lines = segments(&marks, &[]);
    assert_eq!(lines.iter().map(|segment| (segment.points.len(), segment.dashed)).collect::<Vec<_>>(), [(4, false)]);
    // A break before the first point, or one going on after the last, dashes nothing.
    let spans: Vec<GapSpan> = [gap(day(2026, 1, 5), Some(day(2026, 6, 2)), 148), gap(day(2026, 11, 3), None, 80)].iter().filter_map(gap_span).collect();
    assert_eq!(segments(&marks, &spans).iter().map(|segment| segment.dashed).collect::<Vec<_>>(), [false]);
}

#[test]
fn a_chart_of_one_point_or_of_two_that_a_break_separates_has_no_solid_line() {
    let one = vec![point("2026-10", 80.0, 10, day(2026, 10, 3))];
    assert!(segments(&marks(&one, false), &[]).is_empty(), "a dot, not a line");
    let two = vec![point("2026-06", 70.0, 10, day(2026, 6, 3)), point("2026-10", 80.0, 10, day(2026, 10, 3))];
    let spans: Vec<GapSpan> = [gap(day(2026, 6, 3), Some(day(2026, 10, 3)), 122)].iter().filter_map(gap_span).collect();
    assert_eq!(segments(&marks(&two, false), &spans).iter().map(|segment| segment.dashed).collect::<Vec<_>>(), [true]);
    assert!(segments(&[], &spans).is_empty());
}

#[test]
fn a_break_is_a_stretch_of_the_axis_and_one_going_on_ends_after_its_length() {
    let closed = gap_span(&gap(day(2026, 8, 20), Some(day(2026, 10, 5)), 46)).expect("a break");
    assert_eq!((closed.x0, closed.x1), (x_of_date(day(2026, 8, 20)), x_of_date(day(2026, 10, 5))));
    assert_eq!((closed.first_month, closed.last_month), (month_of(day(2026, 8, 20)), month_of(day(2026, 10, 5))));
    // Going on: 70 days after 16 June is 25 August.
    let open = gap_span(&gap(day(2026, 6, 16), None, 70)).expect("a break");
    assert_eq!(open.x1, x_of_date(day(2026, 8, 25)));
    // A break that cannot be placed is left out, not drawn wrong.
    assert!(gap_span(&gap(day(2026, 10, 5), Some(day(2026, 8, 20)), 46)).is_none(), "ends before it begins");
    assert!(gap_span(&gap(day(2026, 6, 16), None, -3)).is_none(), "a negative length");
    assert!(gap_span(&gap(day(2026, 6, 16), None, i64::MAX)).is_none(), "out of the calendar");
}

#[test]
fn the_axes_show_the_data_with_room_and_never_less_than_half_a_year() {
    let months = |from: &[(&str, f64)]| -> Vec<PointView> { from.iter().map(|(month, kg)| point(month, *kg, 10, day(2026, 1, 1))).collect() };
    let one = months(&[("2026-10", 80.0)]);
    let one = marks(&one, false);
    let x = month_index("2026-10").map_or(0.0, |index| index as f64);
    // A point has half a month on each side, and five and a half months before it: the axis spans six months.
    assert_eq!(x_bounds(&one, &[], &[]), (x - 5.5, x + 0.5));
    assert_eq!(y_bounds(&one, &[], false), (77.5, 82.5), "room of a plate's step around a lone weight");
    // A year of points: from the left edge of the first month to the right edge of the last.
    let year: Vec<PointView> = (1..=12).map(|month| point(&format!("2026-{month:02}"), 60.0 + f64::from(month) * 2.0, 10, day(2026, month, 1))).collect();
    let year = marks(&year, false);
    let first = month_index("2026-01").map_or(0.0, |index| index as f64);
    assert_eq!(x_bounds(&year, &[], &[]), (first - 0.5, first + 11.5));
    // A quarter of the range above and below it.
    assert_eq!(y_bounds(&year, &[], false), (62.0 - 5.5, 84.0 + 5.5));
    // The heavier sets count for the vertical axis, and so do the breaks for the horizontal one.
    let heavier = months(&[("2026-12", 100.0)]);
    let heavier = marks(&heavier, false);
    assert_eq!(y_bounds(&year, &heavier, false).1, 100.0 + (100.0 - 62.0) * 0.25);
    let open = gap_span(&gap(day(2027, 3, 1), None, 40)).expect("a break");
    assert_eq!(x_bounds(&year, &[], std::slice::from_ref(&open)).1, open.x1);
    // Not under zero, and room in a chart of reps.
    let light = months(&[("2026-10", 1.0)]);
    assert_eq!(y_bounds(&marks(&light, false), &[], false), (0.0, 3.5));
    let reps: Vec<PointView> = [8, 12].iter().zip(["2026-09", "2026-10"]).map(|(reps, month)| point(month, 0.0, *reps, day(2026, 1, 1))).collect();
    assert_eq!(y_bounds(&marks(&reps, true), &[], true), (6.5, 13.5));
    // Nothing: something that can be asked for.
    assert_eq!(x_bounds(&[], &[], &[]), (0.0, metrics::CHART_MIN_MONTHS));
    assert_eq!(y_bounds(&[], &[], false), (0.0, 1.0));
}

#[test]
fn the_grid_steps_are_round_numbers_that_leave_room_for_their_labels() {
    for (base, step) in [(0.3, 1.0), (1.0, 1.0), (1.2, 2.0), (2.5, 3.0), (3.0, 3.0), (4.0, 6.0), (7.0, 12.0), (13.0, 24.0), (1e9, 120.0), (f64::NAN, 1.0)] {
        assert_eq!(month_step(base), step, "{base}");
    }
    for (base, step) in [(0.2, 1.0), (1.0, 1.0), (1.5, 2.0), (2.2, 2.5), (3.0, 5.0), (6.0, 10.0), (12.0, 20.0), (23.0, 25.0), (40.0, 50.0), (60.0, 100.0)] {
        assert_eq!(nice_step(base), step, "{base}");
    }
    assert_eq!((nice_step(f64::NAN), nice_step(f64::INFINITY), nice_step(-4.0)), (1.0, 1.0, 1.0));
    let values = |marks: Vec<GridMark>| marks.iter().map(|mark| mark.value).collect::<Vec<_>>();
    assert_eq!(values(multiples((67.5, 82.5), 5.0)), [70.0, 75.0, 80.0]);
    assert_eq!(values(multiples((24300.5, 24312.5), 1.0)).len(), 12);
    assert_eq!(values(multiples((24300.5, 24312.5), 3.0)), [24303.0, 24306.0, 24309.0, 24312.0], "a quarter begins on a January, April, July or October");
    // Bounds that are not numbers, or a step that is not one, draw nothing; huge bounds are cut.
    for (bounds, step) in [((f64::NAN, 1.0), 1.0), ((0.0, f64::INFINITY), 1.0), ((0.0, 10.0), 0.0), ((0.0, 10.0), f64::NAN), ((0.0, 10.0), -1.0)] {
        assert!(multiples(bounds, step).is_empty(), "{bounds:?} {step}");
    }
    assert_eq!(multiples((0.0, 1e12), 1.0).len() as i64, MAX_MARKS + 1);
}

#[test]
fn a_month_label_has_the_year_where_a_year_begins_or_the_axis_does() {
    let at = |year: i64, month: i64| GridMark { value: (year * 12 + month - 1) as f64, step_size: 1.0 };
    let range = |from: f64| from..=from + 12.0;
    let far_left = range(0.0);
    for (lang, may, jan, aug) in [(Lang::En, "May", "Jan 2027", "Aug"), (Lang::Ru, "май", "янв 2027", "авг")] {
        i18n::set_current(lang);
        assert_eq!(x_label(at(2026, 5), &far_left), may, "{lang:?}: the month alone, nominative");
        assert_eq!(x_label(at(2027, 1), &far_left), jan, "{lang:?}: a January says its year");
        assert_eq!(x_label(at(2026, 8), &far_left), aug, "{lang:?}");
    }
    i18n::set_current(Lang::En);
    // The first label says its year; the one after it does not.
    let start = (2026 * 12 + 7) as f64 - 0.4;
    assert_eq!(x_label(at(2026, 8), &range(start)), "Aug 2026");
    assert_eq!(x_label(at(2026, 9), &range(start)), "Sep");
    // Counting by quarters, the first label in the range is the October (the July before it is left of the axis).
    let quarter = |month: i64| GridMark { value: (2026 * 12 + month - 1) as f64, step_size: 3.0 };
    assert_eq!(x_label(quarter(10), &range(start)), "Oct 2026");
    assert_eq!(x_label(quarter(13), &range(start)), "Jan 2027");
    assert_eq!(x_label(quarter(16), &range(start)), "Apr");
    // A mark between two months, or one that is not a number, has no label.
    assert_eq!(x_label(GridMark { value: 24313.5, step_size: 1.0 }, &far_left), "");
    assert_eq!(x_label(GridMark { value: f64::NAN, step_size: 1.0 }, &far_left), "");
}

#[test]
fn pointing_at_a_mark_says_its_day_weight_and_reps_and_elsewhere_says_nothing() {
    let points = vec![point("2026-08", 80.0, 10, day(2026, 8, 18)), point("2026-09", 82.5, 10, day(2026, 9, 20))];
    let heavier = vec![point("2025-12", 90.0, 6, day(2025, 12, 28))];
    let weights = view(points, heavier, Vec::new(), false);
    let drawn = chart(&weights);
    let near = |plot_name: &'static str, index: usize| HoverPosition::NearDataPoint { plot_name, position: PlotPoint::new(0.0, 0.0), index };
    for (lang, points, circle) in [
        (Lang::En, ["18 August · 80 kg × 10", "20 September · 82.5 kg × 10"], "28 December 2025 · 90 kg × 6"),
        (Lang::Ru, ["18 августа · 80 кг × 10", "20 сентября · 82,5 кг × 10"], "28 декабря 2025 · 90 кг × 6"),
    ] {
        i18n::set_current(lang);
        assert_eq!(hover_text(&drawn, 2026, &near(POINTS, 0)).as_deref(), points.first().copied(), "{lang:?}");
        assert_eq!(hover_text(&drawn, 2026, &near(POINTS, 1)).as_deref(), points.get(1).copied(), "{lang:?}");
        assert_eq!(hover_text(&drawn, 2026, &near(HEAVIER, 0)).as_deref(), Some(circle), "{lang:?}: another year says its year");
    }
    // Nowhere in particular, a line, or a mark that is not there.
    assert_eq!(hover_text(&drawn, 2026, &HoverPosition::Elsewhere { position: PlotPoint::new(1.0, 1.0) }), None);
    assert_eq!(hover_text(&drawn, 2026, &near("line-0", 0)), None);
    assert_eq!(hover_text(&drawn, 2026, &near(POINTS, 2)), None);
    assert_eq!(hover_text(&drawn, 2026, &near(HEAVIER, usize::MAX)), None);
    // A chart of reps says the reps, with the unit of the number.
    let reps = view(vec![point("2026-09", 0.0, 1, day(2026, 9, 20)), point("2026-10", 0.0, 12, day(2026, 10, 20))], Vec::new(), Vec::new(), true);
    let drawn = chart(&reps);
    for (lang, first, second) in [(Lang::En, "20 September · 1 rep", "20 October · 12 reps"), (Lang::Ru, "20 сентября · 1 повтор", "20 октября · 12 повторов")]
    {
        i18n::set_current(lang);
        assert_eq!(hover_text(&drawn, 2026, &near(POINTS, 0)).as_deref(), Some(first), "{lang:?}");
        assert_eq!(hover_text(&drawn, 2026, &near(POINTS, 1)).as_deref(), Some(second), "{lang:?}");
    }
}

#[test]
fn what_cannot_be_drawn_is_left_out_and_the_rest_is_in_month_order() {
    let points = vec![
        point("2026-10", 80.0, 10, day(2026, 10, 3)),
        point("garbage", 99.0, 10, day(2026, 7, 1)),
        point("2026-08", f64::NAN, 10, day(2026, 8, 3)),
        point("2026-09", 75.0, 10, day(2026, 9, 3)),
        point("2026-07", f64::INFINITY, 10, day(2026, 7, 3)),
    ];
    let shown = view(points, Vec::new(), Vec::new(), false);
    let drawn = chart(&shown);
    assert_eq!(drawn.points.iter().map(|mark| mark.y).collect::<Vec<_>>(), [75.0, 80.0], "the months that read, in order");
    // In a chart of reps the weight is not looked at, so the same points are all numbers.
    let reps = view(vec![point("2026-08", f64::NAN, 9, day(2026, 8, 3))], Vec::new(), Vec::new(), true);
    assert_eq!(chart(&reps).points.iter().map(|mark| mark.y).collect::<Vec<_>>(), [9.0]);
}

#[test]
fn dates_are_read_against_the_year_of_the_latest_session() {
    let mut shown = view(Vec::new(), Vec::new(), Vec::new(), false);
    // No sessions listed: the latest one of the summary.
    assert_eq!(reference_year(&shown), 2026);
    let session = |date: NaiveDate| SessionView {
        date,
        start: chrono::DateTime::UNIX_EPOCH,
        activity_id: ActivityId(1),
        workout_name: None,
        working: working(date),
        sets: Vec::new(),
    };
    shown.sessions = vec![session(day(2027, 1, 4)), session(day(2026, 12, 30))];
    assert_eq!(reference_year(&shown), 2027, "newest first");
    shown.sessions.clear();
    shown.summary = None;
    assert_eq!(reference_year(&shown), 0, "nothing to read against: every year is said");
}
