//! The chart of one exercise on its page (exercise progress spec §2, item 3; the approved mock-up `chart-metric.html`,
//! variant A): the weight lifted for the reps the program asks, month by month, or the reps themselves for an exercise that
//! never had a weight.
//!
//! The line runs through the best set of each month (`points`), dashed across a break; a hollow circle is a set heavier
//! than the line's but for fewer reps (`heavier`); a break is shaded. Pointing at a point or a circle says its day, weight
//! and reps. The chart cannot be dragged or zoomed: it is a picture of the answer, not a tool to look around it with.
//!
//! It only draws what `progress.get` answered. The months are consecutive whole numbers on the horizontal axis, each month
//! a slot of width 1 with its point in the middle, so a day (of a break, which does not begin on the first) has its place
//! inside its month's slot. Everything the answer gives that cannot be drawn (a month that does not read, a value that is
//! not a number) is left out rather than trusted.

use std::ops::RangeInclusive;

use chrono::{Datelike, Days, Months, NaiveDate};
use egui::{Align2, FontId, Rangef, RichText, Sense, Stroke, Ui, Vec2, vec2};
use egui_plot::{AxisHints, GridInput, GridMark, HoverPosition, Line, LineStyle, MarkerShape, Plot, PlotPoint, PlotPoints, Points, Span, Text};
use gymgar_engine::{GapView, PointView, ProgressView};

use crate::i18n::trn;
use crate::theme::{Tokens, metrics};
use crate::{dates, numbers, tl, widgets};

/// The names of the two sets of marks, which the hover reads back: the plot says which of them is nearest by name.
const POINTS: &str = "points";
const HEAVIER: &str = "heavier";

/// How many grid lines of an axis are drawn at the most, whatever the bounds say: the bounds are the answer's, which is not
/// to be trusted with an allocation.
const MAX_MARKS: i64 = 256;

/// The steps a month axis counts by, in months: the labels are every month, every two, every quarter and so on, so that
/// a year always begins on a label.
const MONTH_STEPS: [f64; 8] = [1.0, 2.0, 3.0, 6.0, 12.0, 24.0, 60.0, 120.0];

/// A point or a circle: where it is, and the answer's point it stands for.
struct Mark<'a> {
    /// The month as a count of months since year 0.
    month: i64,
    x: f64,
    y: f64,
    point: &'a PointView,
}

/// A stretch of the line: the points it runs through, and whether it crosses a break.
struct Segment {
    points: Vec<[f64; 2]>,
    dashed: bool,
}

/// A break on the axis: the horizontal stretch it is shaded over, and the months it begins and ends in.
struct GapSpan {
    x0: f64,
    x1: f64,
    first_month: i64,
    last_month: i64,
}

/// Everything the plot draws, worked out from the answer.
struct Chart<'a> {
    /// The chart shows reps, not weight.
    reps_mode: bool,
    points: Vec<Mark<'a>>,
    heavier: Vec<Mark<'a>>,
    segments: Vec<Segment>,
    gaps: Vec<GapSpan>,
    x: (f64, f64),
    y: (f64, f64),
}

/// The month `2026-08` as a count of months since year 0, so that consecutive months are consecutive integers; `None`
/// for text that is not a month.
fn month_index(month: &str) -> Option<i64> {
    let (year, month) = month.split_once('-')?;
    let (year, month) = (year.parse::<i32>().ok()?, month.parse::<u32>().ok()?);
    (1..=12).contains(&month).then(|| i64::from(year) * 12 + i64::from(month) - 1)
}

fn month_of(day: NaiveDate) -> i64 {
    i64::from(day.year()) * 12 + i64::from(day.month0())
}

/// How many days the month of `day` has.
fn days_in_month(day: NaiveDate) -> f64 {
    let length = day.with_day(1).and_then(|first| first.checked_add_months(Months::new(1)).map(|next| next.signed_duration_since(first).num_days()));
    length.map_or(30.0, |days| days as f64)
}

/// Where `day` is on the axis: inside the slot of its month, from half a month before the month's number (its first day)
/// to half a month after it (its last).
fn x_of_date(day: NaiveDate) -> f64 {
    month_of(day) as f64 - 0.5 + (f64::from(day.day()) - 0.5) / days_in_month(day)
}

/// The marks of `points`, in month order. A month that does not read and a value that is not a number have none.
fn marks(points: &[PointView], reps_mode: bool) -> Vec<Mark<'_>> {
    let mut marks: Vec<Mark<'_>> = points
        .iter()
        .filter_map(|point| {
            let month = month_index(&point.month)?;
            let y = if reps_mode { f64::from(point.reps) } else { point.weight_kg };
            y.is_finite().then_some(Mark { month, x: month as f64, y, point })
        })
        .collect();
    marks.sort_by_key(|mark| mark.month);
    marks
}

/// A break as a stretch of the axis. One going on has no end in the answer, only its length: it ends that many days after
/// it began. A break that cannot be placed (its end before its start, or out of the calendar) is not drawn.
fn gap_span(gap: &GapView) -> Option<GapSpan> {
    let end = match gap.to {
        Some(to) => to,
        None => gap.from.checked_add_days(Days::new(u64::try_from(gap.days).ok()?))?,
    };
    (end >= gap.from).then(|| GapSpan { x0: x_of_date(gap.from), x1: x_of_date(end), first_month: month_of(gap.from), last_month: month_of(end) })
}

/// The line through `points`: solid, and dashed from one point to the next where a break lies between their months.
fn segments(points: &[Mark<'_>], gaps: &[GapSpan]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut run: Vec<[f64; 2]> = Vec::new();
    let mut before: Option<&Mark<'_>> = None;
    for mark in points {
        let here = [mark.x, mark.y];
        match before {
            Some(earlier) if gaps.iter().any(|gap| gap.first_month >= earlier.month && gap.last_month <= mark.month) => {
                if run.len() >= 2 {
                    segments.push(Segment { points: std::mem::take(&mut run), dashed: false });
                }
                run.clear();
                segments.push(Segment { points: vec![[earlier.x, earlier.y], here], dashed: true });
                run.push(here);
            }
            _ => run.push(here),
        }
        before = Some(mark);
    }
    if run.len() >= 2 {
        segments.push(Segment { points: run, dashed: false });
    }
    segments
}

/// The stretch of the axis to show: every point with half a month on each side and every break, and at least
/// [`metrics::CHART_MIN_MONTHS`] months ending where the data ends, so that a chart of one point has room for the months
/// to come.
fn x_bounds(points: &[Mark<'_>], heavier: &[Mark<'_>], gaps: &[GapSpan]) -> (f64, f64) {
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for mark in points.iter().chain(heavier) {
        low = low.min(mark.x - 0.5);
        high = high.max(mark.x + 0.5);
    }
    for gap in gaps {
        low = low.min(gap.x0);
        high = high.max(gap.x1);
    }
    if !low.is_finite() || !high.is_finite() {
        return (0.0, metrics::CHART_MIN_MONTHS);
    }
    (low.min(high - metrics::CHART_MIN_MONTHS), high)
}

/// The stretch of the vertical axis to show: the values with room above and below, so that no point sits on the frame, and
/// not under zero. A chart of one value gets room around it too.
fn y_bounds(points: &[Mark<'_>], heavier: &[Mark<'_>], reps_mode: bool) -> (f64, f64) {
    let (low, high) = points.iter().chain(heavier).fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), mark| (low.min(mark.y), high.max(mark.y)));
    if !low.is_finite() || !high.is_finite() {
        return (0.0, 1.0);
    }
    // At least a plate's step of weight, or a rep or two.
    let at_least = if reps_mode { 1.5 } else { 2.5 };
    let room = ((high - low) * 0.25).max(at_least);
    ((low - room).max(0.0), high + room)
}

fn chart(view: &ProgressView) -> Chart<'_> {
    let reps_mode = view.summary.as_ref().is_some_and(|item| item.chart_reps.is_none());
    let points = marks(&view.points, reps_mode);
    let heavier = marks(&view.heavier, reps_mode);
    let gaps: Vec<GapSpan> = view.gaps.iter().filter_map(gap_span).collect();
    Chart {
        reps_mode,
        segments: segments(&points, &gaps),
        x: x_bounds(&points, &heavier, &gaps),
        y: y_bounds(&points, &heavier, reps_mode),
        points,
        heavier,
        gaps,
    }
}

/// The year the dates of an exercise are read against: the one of its latest session. A day of another year says its year.
pub(crate) fn reference_year(view: &ProgressView) -> i32 {
    view.sessions.first().map(|session| session.date).or_else(|| view.summary.as_ref().map(|item| item.last.date)).map_or(0, |day| day.year())
}

/// The first step of [`MONTH_STEPS`] that is at least `base` months, which is how many months fit the least space between
/// two grid lines; the longest if none is.
fn month_step(base: f64) -> f64 {
    if base.is_nan() {
        return 1.0;
    }
    MONTH_STEPS.iter().copied().find(|step| *step >= base - 1e-9).or_else(|| MONTH_STEPS.last().copied()).unwrap_or(1.0)
}

/// The first of 1, 2, 2.5 and 5 times a power of ten that is at least `base`, and at least 1: where the lines across the
/// vertical axis go, in kilograms or reps.
fn nice_step(base: f64) -> f64 {
    if !base.is_finite() || base <= 1.0 {
        return 1.0;
    }
    let power = 10f64.powf(base.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|multiple| multiple * power).find(|step| *step >= base - 1e-9).unwrap_or(10.0 * power)
}

/// The multiples of `step` inside `bounds`, as the marks of one thickness.
fn multiples(bounds: (f64, f64), step: f64) -> Vec<GridMark> {
    let (low, high) = bounds;
    if !(low.is_finite() && high.is_finite() && step > 0.0 && step.is_finite()) {
        return Vec::new();
    }
    let (first, last) = ((low / step).ceil(), (high / step).floor());
    if !(first.is_finite() && last.is_finite()) {
        return Vec::new();
    }
    // Float to count: the bounds are finite and the count is cut.
    let (first, last) = (first as i64, last as i64);
    (first..=last.min(first.saturating_add(MAX_MARKS))).map(|index| GridMark { value: index as f64 * step, step_size: step }).collect()
}

/// The label of a month mark: the month's short name, with its year when it is a January or the first label of the axis.
fn x_label(mark: GridMark, range: &RangeInclusive<f64>) -> String {
    let index = mark.value.round();
    if (mark.value - index).abs() > 1e-6 || !index.is_finite() {
        return String::new();
    }
    let index = index as i64;
    let (year, month) = (index.div_euclid(12), u32::try_from(index.rem_euclid(12) + 1).unwrap_or(1));
    let name = dates::month_label(month);
    let first = mark.value - mark.step_size < *range.start();
    if month == 1 || first { format!("{name} {year}") } else { name.to_owned() }
}

/// What pointing at a mark says: `20 September · 82.5 kg × 10`, or `20 September · 12 reps` in a chart of reps. Nothing
/// is said for a place where there is no mark.
fn hover_text(chart: &Chart<'_>, year: i32, hover: &HoverPosition<'_>) -> Option<String> {
    let HoverPosition::NearDataPoint { plot_name, index, .. } = hover else { return None };
    let marks = match *plot_name {
        POINTS => &chart.points,
        HEAVIER => &chart.heavier,
        _ => return None,
    };
    let point = marks.get(*index)?.point;
    let day = dates::day_month(point.date, year);
    Some(if chart.reps_mode {
        format!("{day} · {}", trn(u64::from(point.reps), tl!("{} rep"), tl!("{} reps (2 to 4)"), tl!("{} reps")))
    } else {
        format!("{day} · {} × {}", numbers::kg(point.weight_kg), point.reps)
    })
}

/// Draws the chart of `view`, its plot `height` points high, and a legend of what it draws under it. An answer with
/// nothing to draw says so.
pub(crate) fn show(ui: &mut Ui, view: &ProgressView, height: f32) {
    let chart = chart(view);
    if chart.points.is_empty() && chart.heavier.is_empty() {
        widgets::hint(ui, tl!("Nothing to chart yet"));
        return;
    }
    let t = Tokens::get(ui.ctx());
    let year = reference_year(view);
    let shade = t.warning.gamma_multiply(metrics::GAP_OPACITY);
    let font = FontId::proportional(metrics::SMALL_TEXT_SIZE);
    let x_axis = AxisHints::new_x()
        .formatter(x_label)
        .label_spacing(Rangef::new(metrics::CHART_MONTH_SPACING * 0.8, metrics::CHART_MONTH_SPACING))
        .tick_label_color(t.text_dim)
        .tick_label_font(font.clone());
    let y_axis = AxisHints::new_y().formatter(|mark, _| numbers::kg_value(mark.value)).tick_label_color(t.text_dim).tick_label_font(font);
    let (x, y) = (chart.x, chart.y);
    let shown = &chart;
    Plot::new(("progress-chart", view.exercise.id.as_str()))
        .height(height)
        .sense(Sense::hover())
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .allow_axis_zoom_drag(false)
        .show_crosshair(false)
        .show_background(false)
        .show_grid([false, true])
        .grid_color(t.border)
        .grid_spacing(Rangef::new(metrics::CHART_GRID_SPACING, 300.0))
        .set_margin_fraction(Vec2::ZERO)
        .include_x(x.0)
        .include_x(x.1)
        .include_y(y.0)
        .include_y(y.1)
        .custom_x_axes(vec![x_axis])
        .custom_y_axes(vec![y_axis])
        // The plot counts the least space between two grid lines for both axes: the months need more of it than the weights.
        .x_grid_spacer(|input: GridInput| {
            multiples(input.bounds, month_step(input.base_step_size * f64::from(metrics::CHART_MONTH_SPACING / metrics::CHART_GRID_SPACING)))
        })
        .y_grid_spacer(|input: GridInput| multiples(input.bounds, nice_step(input.base_step_size)))
        .label_formatter(move |hover| hover_text(shown, year, hover))
        .show(ui, move |plot_ui| {
            for gap in &shown.gaps {
                plot_ui.span(Span::new(String::new(), gap.x0..=gap.x1).fill(shade).border_width(0.0));
                // The word, where the break is wide enough to hold it.
                if gap.x1 - gap.x0 >= 1.5 {
                    let label = RichText::new(tl!("Break")).size(metrics::SMALL_TEXT_SIZE).color(t.warning.gamma_multiply(0.8));
                    plot_ui.text(Text::new("break", PlotPoint::new((gap.x0 + gap.x1) / 2.0, shown.y.1), label).anchor(Align2::CENTER_TOP));
                }
            }
            for (index, segment) in shown.segments.iter().enumerate() {
                let line = Line::new(format!("line-{index}"), PlotPoints::from(segment.points.clone())).allow_hover(false);
                let line = if segment.dashed {
                    line.color(t.accent.gamma_multiply(0.6)).width(metrics::CHART_LINE_WIDTH * 0.75).style(LineStyle::Dashed { length: metrics::CHART_DASH })
                } else {
                    line.color(t.accent).width(metrics::CHART_LINE_WIDTH)
                };
                plot_ui.line(line);
            }
            let circles = |marks: &[Mark<'_>]| PlotPoints::from(marks.iter().map(|mark| [mark.x, mark.y]).collect::<Vec<[f64; 2]>>());
            plot_ui.points(
                Points::new(HEAVIER, circles(&shown.heavier)).shape(MarkerShape::Circle).radius(metrics::CHART_POINT_RADIUS).color(t.text_dim).filled(false),
            );
            plot_ui.points(
                Points::new(POINTS, circles(&shown.points)).shape(MarkerShape::Circle).radius(metrics::CHART_POINT_RADIUS).color(t.accent).filled(true),
            );
        });
    ui.add_space(metrics::GAP / 2.0);
    legend(ui, &chart, view);
}

/// What the chart draws, one swatch and a few words for each: the points (with the reps they were lifted for), the hollow
/// circles and the shaded breaks. Only what is on the chart is in it.
fn legend(ui: &mut Ui, chart: &Chart<'_>, view: &ProgressView) {
    let t = Tokens::get(ui.ctx());
    let reps = view.summary.as_ref().and_then(|item| item.chart_reps);
    let points = match (chart.reps_mode, reps) {
        (true, _) => tl!("Best reps of the month").to_owned(),
        (false, Some(reps)) => tl!("Weight at {}+ reps").replace("{}", &reps.to_string()),
        (false, None) => tl!("Weight").to_owned(),
    };
    let shade = t.warning.gamma_multiply(metrics::GAP_OPACITY);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = metrics::STAT_GAP;
        swatch(ui, &points, |painter, rect| {
            painter.circle_filled(rect.center(), metrics::LEGEND_SWATCH / 2.0, t.accent);
        });
        if !chart.heavier.is_empty() {
            swatch(ui, tl!("Heavier, fewer reps"), |painter, rect| {
                painter.circle_stroke(rect.center(), metrics::LEGEND_SWATCH / 2.0 - 0.75, Stroke::new(1.5, t.text_dim));
            });
        }
        if !chart.gaps.is_empty() {
            swatch(ui, tl!("Break"), |painter, rect| {
                painter.rect_filled(rect, 2.0, shade);
            });
        }
    });
}

/// One entry of the legend: a swatch drawn by `paint` in a square, then `text`.
fn swatch(ui: &mut Ui, text: &str, paint: impl FnOnce(&egui::Painter, egui::Rect)) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = metrics::GAP / 2.0;
        let (rect, _) = ui.allocate_exact_size(vec2(metrics::LEGEND_SWATCH, metrics::LEGEND_SWATCH), Sense::hover());
        if ui.is_rect_visible(rect) {
            paint(ui.painter(), rect);
        }
        ui.add(egui::Label::new(RichText::new(text).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim)).selectable(false));
    });
}

#[cfg(test)]
#[path = "progress_chart_tests.rs"]
mod tests;
