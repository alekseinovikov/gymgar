//! The Activities screen (activity import spec §2, items 5–7 and 16; the approved mock-up, variant A): the list of what
//! Garmin sent on the left, the selected activity on the right ([`activity_detail_ui`]).
//!
//! The list's header has the title, "Refresh" (`garmin.sync`) and a line about the import (`garmin.status`): when it last
//! ended, how far a running one is, or why nothing new came. Under it, the chips filter the list, and the list is grouped
//! by week (from Monday, on the user's wall clock). Only the rows in view are drawn, so a history of thousands costs a
//! frame no more than a dozen.
//!
//! It only draws: the activities are what `activity.list` answered ([`GymgarApp::activities`]), and whether "Refresh" can
//! run is the engine's answer. Choosing which rows a chip shows and where a week begins is presentation of that answer,
//! not a rule of the data.

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use egui::{
    Align, CentralPanel, Color32, CursorIcon, Frame, Label, Layout, Margin, Panel, Rect, RichText, Role, ScrollArea, Sense, Spinner, Stroke, StrokeKind,
    TextStyle, Ui, UiBuilder, WidgetInfo, pos2, vec2,
};
use gymgar_domain::{ConnectionStatus, FileState, STRENGTH_TYPE_KEY, SyncErrorKind};
use gymgar_engine::{ActivityListItemView, GarminStatusView, SyncStatusView};
use serde_json::Value;

use crate::i18n::trn;
use crate::state::ListFilter;
use crate::theme::{Tokens, metrics};
use crate::{GymgarApp, activity_detail_ui, dates, garmin_ui, numbers, tl, widgets};

/// Draws the screen: the list as a panel beside the rail, the selected activity in the rest of the window.
pub(crate) fn show(app: &mut GymgarApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let padding = metrics::LIST_PADDING;
    Panel::left("activity-list")
        .exact_size(metrics::LIST_WIDTH)
        .resizable(false)
        // The list runs to the bottom edge of the window, scrolled.
        .frame(Frame::new().fill(t.bg).inner_margin(Margin { left: padding, right: padding, top: padding, bottom: 0 }))
        .show(ui, |ui| list_column(app, ui, &t));
    CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin::same(metrics::SCREEN_MARGIN))).show(ui, |ui| {
        app.banner(ui);
        activity_detail_ui::show(app, ui);
    });
}

/// The left column: the header, the chips, and the list or what stands for it.
fn list_column(app: &mut GymgarApp, ui: &mut Ui, t: &Tokens) {
    let status = app.status().cloned();
    header(app, ui, status.as_ref());
    ui.add_space(metrics::GAP / 2.0);
    chips(app, ui, status.as_ref().map_or(0, |status| status.sync.new_count));
    ui.add_space(metrics::GAP / 2.0);
    let connection = status.as_ref().map(|status| status.status);
    if connection == Some(ConnectionStatus::NotConnected) {
        not_connected(app, ui);
        ui.add_space(metrics::GAP);
    }
    if app.activities().is_empty() {
        // Not connected, the card above says it all; a connection that cannot be read says nothing it does not know.
        if connection.is_some_and(|status| status != ConnectionStatus::NotConnected) {
            let loading = status.as_ref().is_some_and(|status| status.sync.running) || app.startup_pending();
            widgets::hint(ui, if loading { tl!("Loading workouts from Garmin…") } else { tl!("No workouts yet. Refresh loads them from Garmin.") });
        }
        return;
    }
    list(app, ui, t);
}

/// The title, "Refresh" with the indicator of a running sync, and the line about the import.
fn header(app: &mut GymgarApp, ui: &mut Ui, status: Option<&GarminStatusView>) {
    let running = status.is_some_and(|status| status.sync.running);
    // Greyed out while a sync or the re-parse runs, while the key is checked, and without a connection.
    let can_refresh = app.is_enabled("garmin.sync");
    let mut refresh = false;
    ui.horizontal(|ui| {
        widgets::heading(ui, tl!("Activities"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            refresh = ui.add_enabled_ui(can_refresh, |ui| widgets::secondary_button(ui, tl!("Refresh"))).inner.clicked();
            if running {
                ui.add(Spinner::new().size(metrics::SPINNER_SIZE));
            }
        });
    });
    if let Some(status) = status {
        sync_line(app, ui, &status.sync);
    }
    if refresh && let Err(text) = app.run("garmin.sync", Value::Null) {
        app.report(&text);
    }
}

/// The line about the import (spec §2, item 6): how far a running sync is, else why the last one stopped, else when it
/// ended. A key that does not work leads to Settings, where it is replaced. Nothing before the first sync.
pub(crate) fn sync_line(app: &mut GymgarApp, ui: &mut Ui, sync: &SyncStatusView) {
    let t = Tokens::get(ui.ctx());
    if let Some(text) = progress_text(sync) {
        widgets::hint(ui, &text);
    } else if let Some(kind) = sync.last_error {
        let (text, settings) = error_text(kind);
        ui.label(RichText::new(text).color(t.warning));
        if settings && ui.link(tl!("Open Settings")).clicked() {
            garmin_ui::replace_key(app);
        }
    } else if let Some(at) = sync.last_success_at {
        let text = loaded_text(app, at);
        ui.scope(|ui| {
            // A line of text, not a row of controls: no taller than its text.
            ui.spacing_mut().interact_size.y = 0.0;
            ui.horizontal(|ui| {
                // As high as the line, so the dot sits at the middle of the text.
                let (rect, _) = ui.allocate_exact_size(vec2(metrics::NEW_DOT, ui.text_style_height(&TextStyle::Body)), Sense::hover());
                ui.painter().circle_filled(rect.center(), metrics::NEW_DOT / 2.0, t.accent);
                widgets::hint(ui, &text);
            });
        });
    }
}

/// How far the running sync is: `Loading: 12 of 340` (strength files), or `Loading…` while it reads the list. `None`
/// when no sync runs.
pub(crate) fn progress_text(sync: &SyncStatusView) -> Option<String> {
    if !sync.running {
        return None;
    }
    if sync.files_total == 0 {
        return Some(tl!("Loading…").to_owned());
    }
    Some(tl!("Loading: {} of {}").replacen("{}", &sync.files_done.to_string(), 1).replacen("{}", &sync.files_total.to_string(), 1))
}

/// Why the last sync brought nothing more (spec §7, «Ошибки загрузки»), and whether Settings is where it is fixed.
fn error_text(kind: SyncErrorKind) -> (&'static str, bool) {
    match kind {
        SyncErrorKind::KeyRejected => (tl!("Garmin does not accept the key. Replace it in Settings."), true),
        SyncErrorKind::KeyMissing => (tl!("The Garmin key was not found. Connect Garmin again."), true),
        SyncErrorKind::RateLimited => (tl!("Garmin has temporarily limited requests. Will continue later."), false),
        SyncErrorKind::Network => (tl!("No connection to Garmin. Showing what is already loaded."), false),
        SyncErrorKind::Unavailable => (tl!("Garmin is unavailable right now. Will continue later."), false),
        SyncErrorKind::Storage => (tl!("Could not save part of what Garmin sent. Will try again later."), false),
    }
}

/// `Loaded today at 19:12`, or the date and time of an earlier day.
fn loaded_text(app: &GymgarApp, at: DateTime<Utc>) -> String {
    let local = app.local_time(at);
    if local.date() == app.local_time(app.now()).date() {
        tl!("Loaded today at {}").replace("{}", &dates::time(local))
    } else {
        tl!("Loaded {}").replace("{}", &dates::date_time(local))
    }
}

/// The chips "All", "Strength" and "New · N" (N from the status, the same number as on the rail).
fn chips(app: &mut GymgarApp, ui: &mut Ui, new_count: u32) {
    let new = tl!("New · {}").replace("{}", &new_count.to_string());
    let current = app.ui.activities.filter;
    ui.scope(|ui| {
        // A row of chips is as high as a chip, not as a button.
        ui.spacing_mut().interact_size.y = metrics::CHIP_HEIGHT;
        ui.spacing_mut().item_spacing.x = metrics::GAP * 0.75;
        ui.horizontal_wrapped(|ui| {
            for (filter, label) in [(ListFilter::All, tl!("All")), (ListFilter::Strength, tl!("Strength")), (ListFilter::New, new.as_str())] {
                if widgets::chip(ui, label, current == filter).clicked() {
                    app.ui.activities.filter = filter;
                }
            }
        });
    });
}

/// Without a connection: the way to Settings. What an earlier connection loaded is listed under it.
fn not_connected(app: &mut GymgarApp, ui: &mut Ui) {
    let open = widgets::card(ui, |ui| {
        ui.label(tl!("Garmin Connect is not connected."));
        ui.link(tl!("Open Settings")).clicked()
    });
    if open {
        app.ui.section = crate::Section::Settings;
    }
}

/// Whether `filter` lets `item` into the list.
pub(crate) fn filter_shows(filter: ListFilter, item: &ActivityListItemView) -> bool {
    match filter {
        ListFilter::All => true,
        ListFilter::Strength => is_strength(item),
        ListFilter::New => item.is_new,
    }
}

fn is_strength(item: &ActivityListItemView) -> bool {
    item.type_key == STRENGTH_TYPE_KEY
}

/// A line of the list: the heading of a week (by its Monday), or an activity (by its place in the app's list).
enum Row {
    Week(NaiveDate),
    Activity(usize),
}

impl Row {
    fn height(&self) -> f32 {
        match self {
            Row::Week(_) => metrics::WEEK_HEADING_HEIGHT,
            Row::Activity(_) => metrics::LIST_ROW_HEIGHT,
        }
    }
}

/// The activities the chip lets through, under the headings of their weeks, scrolled. A click selects a row.
fn list(app: &mut GymgarApp, ui: &mut Ui, t: &Tokens) {
    let filter = app.ui.activities.filter;
    let selected = app.ui.activities.selected;
    let today = app.local_time(app.now()).date();
    let (this_week, this_year) = (dates::monday_of(today), today.year());
    let mut rows = Vec::new();
    let mut week = None;
    for (index, item) in app.activities().iter().enumerate().filter(|(_, item)| filter_shows(filter, item)) {
        let monday = dates::monday_of(app.local_time(item.start).date());
        if week != Some(monday) {
            rows.push(Row::Week(monday));
            week = Some(monday);
        }
        rows.push(Row::Activity(index));
    }
    if rows.is_empty() {
        widgets::hint(ui, tl!("No workouts here"));
        return;
    }
    let total: f32 = rows.iter().map(Row::height).sum();
    let shown: &GymgarApp = app;
    let mut clicked = None;
    // `show_rows` needs rows of one height, and the headings of the weeks are lower than the activities: the list does
    // what it does with heights of its own, drawing only the rows that meet the viewport.
    ScrollArea::vertical().id_salt("activity-list").auto_shrink(false).show_viewport(ui, |ui, viewport| {
        ui.set_height(total);
        let origin = ui.max_rect().min;
        let width = ui.available_width();
        let mut top = 0.0;
        for row in &rows {
            let height = row.height();
            if top + height >= viewport.min.y && top <= viewport.max.y {
                let rect = Rect::from_min_size(origin + vec2(0.0, top), vec2(width, height));
                match row {
                    Row::Week(monday) => {
                        let name = if *monday == this_week { tl!("This week").to_owned() } else { dates::week(*monday, this_year) };
                        week_heading(ui, t, rect, &name);
                    }
                    Row::Activity(index) => {
                        if let Some(item) = shown.activities().get(*index)
                            && activity_row(shown, ui, t, rect, item, selected == Some(item.id.0))
                        {
                            clicked = Some(item.id.0);
                        }
                    }
                }
            }
            top += height;
        }
    });
    if let Some(id) = clicked {
        app.ui.activities.selected = Some(id);
    }
}

/// The heading of a week, small and in capitals, at the foot of its `rect`.
fn week_heading(ui: &mut Ui, t: &Tokens, rect: Rect, name: &str) {
    let inner = Rect::from_min_max(rect.min + vec2(metrics::ROW_PADDING, 0.0), rect.max - vec2(metrics::ROW_PADDING, metrics::GAP / 2.0));
    let mut heading = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::bottom_up(Align::Min)));
    heading.add(
        Label::new(RichText::new(name.to_uppercase()).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim).extra_letter_spacing(0.5)).truncate().selectable(false),
    );
}

/// One activity in `rect`: the dot of a new one, its name and the line with its day and numbers. A long name is cut
/// with an ellipsis (Review Focus 5), never wider than the list. Returns whether it was clicked.
fn activity_row(app: &GymgarApp, ui: &mut Ui, t: &Tokens, rect: Rect, item: &ActivityListItemView, selected: bool) -> bool {
    let title = title(item);
    // A hair between two rows, so two highlighted ones do not merge.
    let rect = rect.shrink2(vec2(0.0, 1.0));
    let response = ui.interact(rect, ui.scope_id().with(("activity-row", item.id.0)), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, &title));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect(rect, t.radius_control, t.card, Stroke::new(1.0, t.border), StrokeKind::Inside);
        } else if response.hovered() {
            painter.rect_filled(rect, t.radius_control, t.card);
        }
        if response.has_focus() {
            painter.rect_stroke(rect, t.radius_control, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        }
        if item.is_new {
            let center = pos2(rect.left() + metrics::ROW_PADDING + metrics::NEW_DOT / 2.0, rect.top() + metrics::ROW_PADDING + metrics::NEW_DOT);
            painter.circle_filled(center, metrics::NEW_DOT / 2.0, t.accent);
        }
    }
    let left = rect.left() + metrics::ROW_PADDING + metrics::NEW_DOT + metrics::GAP;
    let text_rect = Rect::from_min_max(pos2(left, rect.top() + metrics::ROW_PADDING), pos2(rect.right() - metrics::ROW_PADDING, rect.bottom()));
    let mut text = ui.new_child(UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
    text.spacing_mut().item_spacing.y = 2.0;
    text.add(Label::new(RichText::new(&title).color(text_color(t, item))).truncate().selectable(false));
    text.add(Label::new(RichText::new(details(app, item)).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim)).truncate().selectable(false));
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// The colour of a row's name: full for a strength workout that counts, dimmed for the others (spec §2, items 7 and 13).
fn text_color(t: &Tokens, item: &ActivityListItemView) -> Color32 {
    if is_strength(item) && !item.excluded { t.text } else { t.text_dim }
}

/// What a row says under the name: `Thu, 23 Oct · 1:39 · 18 sets` for strength, `Wed, 22 Oct · 5.2 km · 28:41` for the
/// rest. The sets are counted once the file is read; an activity left out of the statistics says so instead.
pub(crate) fn details(app: &GymgarApp, item: &ActivityListItemView) -> String {
    let mut parts = vec![dates::short_date(app.local_time(item.start).date())];
    let duration = item.duration_ms.map(numbers::duration);
    if is_strength(item) {
        parts.extend(duration);
        if !item.excluded && item.file_state == FileState::Downloaded {
            parts.push(sets(item.counted_sets));
        }
    } else {
        parts.extend(item.distance_m.filter(|m| m.is_finite() && *m > 0.0).map(numbers::distance));
        parts.extend(duration);
    }
    if item.excluded {
        parts.push(tl!("not counted").to_owned());
    }
    parts.join(" · ")
}

/// `18 sets`. English never shows the middle form; its line in the catalog is the Russian one for 2 to 4.
fn sets(count: u32) -> String {
    trn(u64::from(count), tl!("{} set"), tl!("{} sets (2 to 4)"), tl!("{} sets"))
}

/// The name of an activity: its own, or the name of its type when it has none (Review Focus 5).
pub(crate) fn title(item: &ActivityListItemView) -> String {
    match item.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => type_name(&item.type_key),
    }
}

/// The name of one of Garmin's activity types: translated for the common ones, else the key made readable
/// (`e_bike_fitness` is `E bike fitness`).
pub(crate) fn type_name(key: &str) -> String {
    let known = match key {
        "strength_training" => tl!("Strength training"),
        "running" => tl!("Running"),
        "treadmill_running" => tl!("Treadmill running"),
        "cycling" => tl!("Cycling"),
        "indoor_cycling" => tl!("Indoor cycling"),
        "pilates" => tl!("Pilates"),
        "walking" => tl!("Walking"),
        "swimming" => tl!("Swimming"),
        "lap_swimming" => tl!("Pool swimming"),
        "hiking" => tl!("Hiking"),
        "yoga" => tl!("Yoga"),
        "cardio" => tl!("Cardio"),
        "hiit" => tl!("HIIT"),
        "indoor_rowing" => tl!("Indoor rowing"),
        _ => return readable(key),
    };
    known.to_owned()
}

/// A key as words: underscores become spaces, and the first letter a capital.
fn readable(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
#[path = "activities_ui_tests.rs"]
mod tests;
