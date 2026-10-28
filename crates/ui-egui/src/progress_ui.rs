//! The Progress screen (exercise progress spec §2, items 2 and 7): a tile for every exercise the user has done, each with
//! the weight and reps of the last session, a small chart of the last twelve months and the advice in a word or two.
//!
//! The chips over the tiles filter them, and the tiles come in the order `progress.list` gave them (add weight first, the
//! exercises nobody has done for half a year last, dimmed). Clicking a tile opens its exercise.
//!
//! It only draws: what each tile says is the engine's answer ([`GymgarApp::progress`]). Which tiles a chip lets through, how
//! many columns fit and how an answer reads as words are presentation, not rules of the data. The page an opened tile shows
//! is [`progress_detail_ui`]'s.

use egui::{Label, RichText, Role, ScrollArea, Ui, WidgetInfo, vec2};
use gymgar_domain::ConnectionStatus;
use gymgar_engine::{AdviceKind, AdviceView, ProgressItemView, WorkingView};

use crate::activity_detail_ui::exercise_name;
use crate::i18n::trn;
use crate::state::{ProgressFilter, Section};
use crate::theme::{Tokens, metrics};
use crate::widgets::Tone;
use crate::{GymgarApp, numbers, progress_detail_ui, tl, widgets};

/// Draws the screen: the tiles, or the page of the exercise the user opened.
pub(crate) fn show(app: &mut GymgarApp, ui: &mut Ui) {
    if app.ui.progress.open.is_some() {
        progress_detail_ui::show(app, ui);
        return;
    }
    widgets::page_title(ui, tl!("Progress"), None);
    ui.add_space(metrics::GAP);
    if app.progress().is_empty() {
        empty(app, ui);
        return;
    }
    chips(app, ui);
    ui.add_space(metrics::GAP);
    tiles(app, ui);
}

/// Nothing to show yet: what will appear, and where to connect Garmin when it is not.
fn empty(app: &mut GymgarApp, ui: &mut Ui) {
    widgets::hint(ui, tl!("Progress will appear here once strength workouts are loaded"));
    let not_connected = app.status().is_some_and(|status| status.status == ConnectionStatus::NotConnected);
    if not_connected && ui.link(tl!("Open Settings")).clicked() {
        app.ui.section = Section::Settings;
    }
}

/// The chips "All", "Can add · N" and "Break · N". The dormant exercises are breaks and count as such.
fn chips(app: &mut GymgarApp, ui: &mut Ui) {
    let count = |kind: AdviceKind| app.progress().iter().filter(|item| item.advice.kind == kind).count().to_string();
    let add = tl!("Can add · {}").replace("{}", &count(AdviceKind::Add));
    let on_break = tl!("Break · {}").replace("{}", &count(AdviceKind::Break));
    let current = app.ui.progress.filter;
    ui.scope(|ui| {
        // A row of chips is as high as a chip, not as a button.
        ui.spacing_mut().interact_size.y = metrics::CHIP_HEIGHT;
        ui.spacing_mut().item_spacing.x = metrics::GAP * 0.75;
        ui.horizontal_wrapped(|ui| {
            for (filter, label) in [(ProgressFilter::All, tl!("All")), (ProgressFilter::Add, add.as_str()), (ProgressFilter::Break, on_break.as_str())] {
                if widgets::chip(ui, label, current == filter).clicked() {
                    app.ui.progress.filter = filter;
                }
            }
        });
    });
}

/// Whether `filter` lets the tile of `item` through.
fn filter_shows(filter: ProgressFilter, item: &ProgressItemView) -> bool {
    match filter {
        ProgressFilter::All => true,
        ProgressFilter::Add => item.advice.kind == AdviceKind::Add,
        ProgressFilter::Break => item.advice.kind == AdviceKind::Break,
    }
}

/// How many columns of tiles fit in `width`: as many as there are room for the narrowest tile, and one at the least.
fn columns_for(width: f32) -> usize {
    if !width.is_finite() {
        return 1;
    }
    // Float to count: truncated on purpose, and a negative or tiny width gives none, which one tile stands in for.
    (((width + metrics::GAP) / (metrics::TILE_MIN_WIDTH + metrics::GAP)).floor() as usize).max(1)
}

/// The tiles the chip lets through, in rows of as many as fit, scrolled. A click opens the exercise.
fn tiles(app: &mut GymgarApp, ui: &mut Ui) {
    let filter = app.ui.progress.filter;
    let shown: Vec<&ProgressItemView> = app.progress().iter().filter(|item| filter_shows(filter, item)).collect();
    if shown.is_empty() {
        widgets::hint(ui, tl!("No exercises here"));
        return;
    }
    let mut opened = None;
    ScrollArea::vertical().id_salt("progress-tiles").auto_shrink(false).show(ui, |ui| {
        let columns = columns_for(ui.available_width());
        for (row, items) in shown.chunks(columns).enumerate() {
            // One id for each row, since `columns` names its columns the same way every time.
            ui.push_id(row, |ui| {
                ui.columns(columns, |cells| {
                    for (cell, item) in cells.iter_mut().zip(items) {
                        if tile(cell, item).clicked() {
                            opened = Some(item.exercise.id.clone());
                        }
                    }
                });
            });
        }
    });
    if let Some(key) = opened {
        app.open_exercise(Some(key));
    }
}

/// One tile: the name, the last session's weight and reps, the chart and the advice. Screen readers hear a button named
/// by the exercise, described by what the tile says under it.
fn tile(ui: &mut Ui, item: &ProgressItemView) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let name = exercise_name(&item.exercise);
    let working = working_text(&item.last, item.chart_reps);
    let advice = advice_short(item);
    let tone = advice_tone(item.advice.kind);
    let values = sparkline_values(item);
    let line = if tone == Tone::Accent { t.accent } else { t.text_dim };
    let response = widgets::tile(ui, tone == Tone::Accent, item.dormant, |ui| {
        ui.spacing_mut().item_spacing.y = metrics::GAP / 2.0;
        ui.add(Label::new(RichText::new(name).color(t.text).strong()).truncate().selectable(false));
        ui.add(Label::new(RichText::new(&working).size(metrics::TILE_VALUE_SIZE).color(t.text).strong()).truncate().selectable(false));
        widgets::sparkline(ui, &values, line, vec2(ui.available_width(), metrics::SPARKLINE_HEIGHT));
        widgets::advice_label(ui, &advice, tone);
    });
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), name));
    ui.ctx().accesskit_node_builder(response.id, |node| node.set_description(format!("{working} · {advice}")));
    response
}

/// The colour a status is drawn in: the accent for what asks to add weight, the warning for a break, else dimmed.
pub(crate) fn advice_tone(kind: AdviceKind) -> Tone {
    match kind {
        AdviceKind::Add => Tone::Accent,
        AdviceKind::Break => Tone::Warning,
        AdviceKind::Hold | AdviceKind::Bodyweight | AdviceKind::NoTarget => Tone::Dim,
    }
}

/// The values of a tile's chart, old to new: the weights, or the reps when the chart is of reps (`chart_reps` is `None`:
/// the exercise never had a weight).
fn sparkline_values(item: &ProgressItemView) -> Vec<f32> {
    let of_weight = item.chart_reps.is_some();
    item.sparkline.iter().map(|point| if of_weight { point.weight_kg as f32 } else { f32::from(point.reps) }).collect()
}

/// What a session did, as a tile says it: `80 kg × 10`, `100 kg × 8, 8, 6` (every set at the working weight, in order),
/// or, for a session without weight, the reps (`12 reps`, `15, 15, 12 reps`). `chart_reps` makes no difference: a weight
/// of zero is shown as reps whatever the chart is drawn for.
pub(crate) fn working_text(last: &WorkingView, _chart_reps: Option<u16>) -> String {
    let Some((final_reps, earlier)) = last.reps.split_last() else {
        // No reps to show: the weight alone, or a dash for nothing.
        return if last.weight_kg > 0.0 { numbers::kg(last.weight_kg) } else { "—".to_owned() };
    };
    if last.weight_kg > 0.0 {
        let reps: Vec<String> = earlier.iter().chain([final_reps]).map(u16::to_string).collect();
        return format!("{} × {}", numbers::kg(last.weight_kg), reps.join(", "));
    }
    // The unit follows the last number, as it reads: "15, 15, 12 reps".
    let unit = trn(u64::from(*final_reps), tl!("{} rep"), tl!("{} reps (2 to 4)"), tl!("{} reps"));
    if earlier.is_empty() {
        return unit;
    }
    let reps: Vec<String> = earlier.iter().map(u16::to_string).collect();
    format!("{}, {unit}", reps.join(", "))
}

/// "Can add: 85 kg", the heading of the advice to add weight wherever it is shown; the bare "Can add weight" for an answer
/// without the weight to go to.
pub(crate) fn can_add_title(advice: &AdviceView) -> String {
    match advice.next_weight_kg {
        Some(weight) => tl!("Can add: {}").replace("{}", &numbers::kg(weight)),
        None => tl!("Can add weight").to_owned(),
    }
}

/// The status of a tile in a word or two. After a break it says so whatever the reps were, since the first session after
/// one starts the count again; an answer that lacks what its status rests on says the bare word.
pub(crate) fn advice_short(item: &ProgressItemView) -> String {
    let advice = &item.advice;
    match advice.kind {
        AdviceKind::Add => match advice.next_weight_kg {
            Some(weight) => tl!("can add {}").replace("{}", &numbers::kg(weight)),
            None => tl!("can add").to_owned(),
        },
        AdviceKind::Hold if advice.break_days.is_some() => tl!("after a break").to_owned(),
        AdviceKind::Hold if advice.met_once => tl!("hold · one more session").to_owned(),
        AdviceKind::Hold => match (item.last.reps.iter().min(), advice.target) {
            (Some(reps), Some(target)) => tl!("hold · {} of {}").replacen("{}", &reps.to_string(), 1).replacen("{}", &target.to_string(), 1),
            _ => tl!("hold").to_owned(),
        },
        AdviceKind::Break => match advice.break_days {
            Some(days) => tl!("break · {} wk").replace("{}", &(days / 7).to_string()),
            None => tl!("break").to_owned(),
        },
        AdviceKind::Bodyweight => tl!("bodyweight").to_owned(),
        AdviceKind::NoTarget => tl!("no target").to_owned(),
    }
}

#[cfg(test)]
#[path = "progress_ui_tests.rs"]
mod tests;
