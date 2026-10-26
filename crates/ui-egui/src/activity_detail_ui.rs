//! The open activity, on the right of the Activities screen (activity import spec §2, items 7, 8, 12, 13 and 16; the
//! approved mock-ups `workouts-layout.html`, variant A, and `workouts-edit.html`).
//!
//! A strength workout has a header (its name, day and hours, the program the watch followed, its numbers and the action
//! that leaves it out of the statistics) and a card per exercise with the table of its sets: a corrected value in the
//! accent colour with the recorded one struck through after it, a set that does not count struck through with why and the
//! way back, a set the user added marked so. The sets without an exercise share a card with a warning border and "Choose
//! exercise"; an exercise's card ends with "+ Add set". A click on a set turns its line into the editor of
//! [`set_edit_ui`], which also has the exercise picker. When the sets are not there, it says why. Any other activity has
//! its header only.
//!
//! It only draws [`GymgarApp::detail`]: the cards, their order, the plans and whether each set counts are the engine's
//! answer (`activity.get`). A click is a command (`activity.setExcluded`, `activity.editSets`, those of the editor), run
//! after the frame is drawn; the window then reads the activity again.

use chrono::TimeDelta;
use egui::{
    Align, Color32, CursorIcon, Label, Layout, Rect, RichText, Role, ScrollArea, Sense, Shape, Sides, Stroke, TextStyle, Ui, UiBuilder, WidgetInfo, pos2, vec2,
};
use gymgar_domain::{CountReason, FileState, STRENGTH_TYPE_KEY, SetRef};
use gymgar_engine::{ActivityListItemView, ActivityView, ExerciseGroupView, ExerciseView, PlanView, SetView};
use serde_json::{Map, Value};

use crate::i18n::{self, Lang};
use crate::set_edit_ui::{self, Request};
use crate::state::SetEdit;
use crate::theme::{Tokens, metrics};
use crate::{GymgarApp, activities_ui, dates, numbers, tl, widgets};

/// What stands for a value that is not there.
const NO_VALUE: &str = "—";

/// What the user asked for while the activity was drawn. It runs once the frame is drawn, since running it reads the
/// activity again.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    /// `activity.setExcluded`: leave the whole activity out of the statistics, or take it back.
    Exclude(bool),
    /// `activity.editSets` with `counted` only: count the set (`Some(true)`), or hand it back to the empty-set rule (`None`).
    Count(SetRef, Option<bool>),
    /// Something of the editor of a set or of the exercise picker.
    Edit(Request),
}

/// Draws the selected activity in `ui` (the right side of the screen), or says that none is selected.
pub(crate) fn show(app: &mut GymgarApp, ui: &mut Ui) {
    app.open_selected();
    if let Some(text) = app.detail_error() {
        widgets::warning_box(ui, text);
        return;
    }
    // The editor and the picker are drawn from the state that keeps them, taken out of it for the frame since the activity
    // they draw over is borrowed from the app as well.
    let mut editing = app.ui.activities.editing.take();
    let mut picker = app.ui.activities.picker.take();
    let shown: &GymgarApp = app;
    let Some(detail) = shown.detail() else {
        ui.centered_and_justified(|ui| widgets::hint(ui, tl!("Select an activity")));
        return;
    };
    // A set the activity no longer has (an agent removed it meanwhile) has no line to edit.
    if editing.as_ref().is_some_and(|edit| set_edit_ui::find_set(Some(detail), edit.set).is_none()) {
        editing = None;
    }
    let id = detail.summary.id.0;
    let mut action = None;
    let panel = ui.max_rect();
    // Salted with the activity, so another activity opens at its top rather than where the last one was left.
    ScrollArea::vertical().id_salt(("activity-detail", id)).auto_shrink(false).show(ui, |ui| {
        // Never wider than the window, and no wider than a screen's content on a wide one.
        ui.set_max_width(metrics::CONTENT_MAX_WIDTH.min(ui.available_width()));
        header(shown, ui, &detail.summary, &mut action);
        if detail.summary.type_key == STRENGTH_TYPE_KEY {
            sets(ui, detail, &mut editing, &mut action);
        }
    });
    if let Some(picker) = picker.as_mut() {
        // Over the top right of the activity, as in the mock-up.
        let corner = pos2(panel.left() + metrics::CONTENT_MAX_WIDTH.min(panel.width()), panel.top() + metrics::POPOVER_TOP);
        let mut request = None;
        set_edit_ui::picker(ui.ctx(), corner, picker, &mut request);
        if let Some(request) = request {
            action = Some(Action::Edit(request));
        }
    }
    app.ui.activities.editing = editing;
    app.ui.activities.picker = picker;
    if let Some(action) = action {
        act(app, id, action);
    }
}

/// Runs what the user asked for on activity `id`. A failure goes to the banner, or under the line being edited.
fn act(app: &mut GymgarApp, id: i64, action: Action) {
    let mut params = Map::new();
    params.insert("id".to_owned(), Value::from(id));
    let command = match action {
        Action::Edit(request) => return set_edit_ui::run(app, id, request),
        Action::Exclude(excluded) => {
            params.insert("excluded".to_owned(), Value::Bool(excluded));
            "activity.setExcluded"
        }
        Action::Count(set, counted) => {
            params.insert("sets".to_owned(), Value::Array(vec![set_param(set)]));
            params.insert("counted".to_owned(), counted.map_or(Value::Null, Value::Bool));
            "activity.editSets"
        }
    };
    if let Err(text) = app.run(command, Value::Object(params)) {
        app.report(&text);
    }
}

/// A set as the commands name it: `{"recorded": 4}` or `{"added": 2}`. Built by hand, not with `json!`, which unwraps inside.
pub(crate) fn set_param(set: SetRef) -> Value {
    let mut param = Map::new();
    match set {
        SetRef::Recorded(index) => param.insert("recorded".to_owned(), Value::from(index)),
        SetRef::Added(id) => param.insert("added".to_owned(), Value::from(id)),
    };
    Value::Object(param)
}

/// The name, with the action that leaves a strength workout out of the statistics beside it; the day, hours and
/// program; the numbers.
fn header(app: &GymgarApp, ui: &mut Ui, summary: &ActivityListItemView, action: &mut Option<Action>) {
    let strength = summary.type_key == STRENGTH_TYPE_KEY;
    let title = activities_ui::title(summary);
    // The button keeps its width, a long name is cut.
    Sides::new().shrink_left().truncate().show(
        ui,
        |ui| widgets::heading(ui, &title),
        |ui| {
            // Other activities never count, so there is nothing to leave out; one an agent left out can be taken back.
            if strength || summary.excluded {
                let label = if summary.excluded { tl!("Count this workout") } else { tl!("Don't count this workout") };
                if widgets::secondary_button(ui, label).clicked() {
                    *action = Some(Action::Exclude(!summary.excluded));
                }
            }
        },
    );
    widgets::hint(ui, &when(app, summary));
    ui.add_space(metrics::GAP / 2.0);
    stats(ui, summary, strength);
    ui.add_space(metrics::GAP / 2.0);
}

/// `Thursday, 23 October 2026 · 17:31–19:10 · following the program “Upper A”`, and `not counted` when it is left out.
fn when(app: &GymgarApp, summary: &ActivityListItemView) -> String {
    let start = app.local_time(summary.start);
    let end = summary
        .duration_ms
        .and_then(|ms| TimeDelta::try_milliseconds(i64::try_from(ms).ok()?))
        .and_then(|duration| summary.start.checked_add_signed(duration))
        .map(|end| app.local_time(end));
    let hours = match end {
        Some(end) => format!("{}–{}", dates::time(start), dates::time(end)),
        None => dates::time(start),
    };
    let mut parts = vec![dates::long_date(start.date()), hours];
    if let Some(program) = summary.workout_name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
        parts.push(tl!("following the program “{}”").replace("{}", program));
    }
    if summary.excluded {
        parts.push(tl!("not counted").to_owned());
    }
    parts.join(" · ")
}

/// The numbers of the header: time, sets (strength) or distance (the others), average heart rate and calories.
fn stats(ui: &mut Ui, summary: &ActivityListItemView, strength: bool) {
    let or_none = |value: Option<String>| value.unwrap_or_else(|| NO_VALUE.to_owned());
    let mut items = vec![(tl!("Time"), or_none(summary.duration_ms.map(numbers::duration)))];
    if strength {
        // Counted once the file is read; a workout left out counts nothing, which the header already says.
        let sets = (summary.file_state == FileState::Downloaded && !summary.excluded).then(|| summary.counted_sets.to_string());
        items.push((tl!("Sets"), or_none(sets)));
    } else if let Some(m) = summary.distance_m.filter(|m| m.is_finite() && *m > 0.0) {
        items.push((tl!("Distance"), numbers::distance(m)));
    }
    items.push((tl!("Avg HR"), or_none(summary.avg_hr.map(|hr| hr.to_string()))));
    items.push((tl!("Calories"), or_none(summary.calories.map(|kcal| kcal.to_string()))));
    let t = Tokens::get(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = metrics::STAT_GAP;
        for (name, value) in items {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(Label::new(RichText::new(name).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim)).selectable(false));
                ui.add(Label::new(RichText::new(value).size(metrics::STAT_VALUE_SIZE).color(t.text).strong()).selectable(false));
            });
        }
    });
}

/// Why a strength workout shows no sets (spec §2, item 16), or `None` when it has them.
fn missing_sets(detail: &ActivityView) -> Option<&'static str> {
    match detail.summary.file_state {
        FileState::Pending => Some(tl!("Sets are loading…")),
        FileState::Unreadable => Some(tl!("Could not read the watch recording")),
        FileState::Missing | FileState::NotNeeded => Some(tl!("The recording has no sets")),
        // Review Focus 3: a file the watch wrote without a set in it.
        FileState::Downloaded => detail.groups.is_empty().then(|| tl!("The recording has no sets")),
    }
}

/// The sets of a strength workout: why there are none yet, then a card per exercise (sets the user added may be there
/// even when the watch's are not), an exercise's ending with the way to add a set the watch did not record.
fn sets(ui: &mut Ui, detail: &ActivityView, editing: &mut Option<SetEdit>, action: &mut Option<Action>) {
    if let Some(text) = missing_sets(detail) {
        widgets::hint(ui, text);
    }
    let t = Tokens::get(ui.ctx());
    for group in &detail.groups {
        let border = if group.exercise.is_some() { t.border } else { t.warning };
        widgets::card_with_border(ui, border, |ui| {
            card_header(ui, &t, group, action);
            table(ui, &t, group, editing, action);
            // A set has an exercise: the sets without one have no "+ Add set".
            if group.exercise.is_some() {
                ui.add_space(metrics::GAP / 2.0);
                if widgets::quiet_button(ui, tl!("+ Add set")).clicked() {
                    *action = Some(Action::Edit(set_edit_ui::add_to_card(group)));
                }
            }
        });
    }
}

/// The name of an exercise in the interface language (spec §7, «Упражнения и названия»): in Russian, the title the watch
/// gave it when that title is written in Cyrillic; otherwise, and always in English, the English name from Garmin's
/// catalog. Names are data: they never go through the catalog of translations.
pub(crate) fn exercise_name(exercise: &ExerciseView) -> &str {
    match (i18n::current(), exercise.watch_title.as_deref().map(str::trim)) {
        (Lang::Ru, Some(title)) if title.chars().any(is_cyrillic) => title,
        _ => &exercise.english,
    }
}

/// A letter of the Cyrillic and Cyrillic Supplement blocks, where the Russian letters are.
fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{052F}')
}

/// The name of the card's exercise, or "No exercise" in the warning colour with the way to name it; on the right, the
/// plan its sets followed.
fn card_header(ui: &mut Ui, t: &Tokens, group: &ExerciseGroupView, action: &mut Option<Action>) {
    // A line of text, unless the button makes it a row of controls.
    let sides = if group.exercise.is_some() { Sides::new().height(ui.text_style_height(&TextStyle::Body)) } else { Sides::new() };
    sides.shrink_left().truncate().show(
        ui,
        |ui| {
            let (name, color) = match &group.exercise {
                Some(exercise) => (exercise_name(exercise), t.text),
                None => (tl!("No exercise"), t.warning),
            };
            ui.add(Label::new(RichText::new(name).color(color).strong()).selectable(false));
        },
        |ui| {
            if group.exercise.is_none() && widgets::primary_button(ui, tl!("Choose exercise")).clicked() {
                *action = set_edit_ui::choose_for_card(group).map(Action::Edit);
            }
            if let Some(text) = group.plan.and_then(|plan| card_plan(group, plan)) {
                ui.add(Label::new(RichText::new(text).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim)).selectable(false));
            }
        },
    );
}

/// `plan 4 × 8 · 80 kg`: the plan every recorded set of the card followed, with how many sets the watch recorded by it.
fn card_plan(group: &ExerciseGroupView, plan: PlanView) -> Option<String> {
    let recorded = group.sets.iter().filter(|set| !set.added).count();
    let text = plan_text(plan.reps.map(|reps| format!("{recorded} × {reps}")), plan.weight_kg)?;
    Some(tl!("plan {}").replace("{}", &text))
}

/// `8 · 80 kg`: the reps and the weight of a plan, those it has.
fn plan_text(reps: Option<String>, weight_kg: Option<f64>) -> Option<String> {
    let parts: Vec<String> = reps.into_iter().chain(weight_kg.map(numbers::kg)).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The columns of a table of sets, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Number,
    Weight,
    Reps,
    Plan,
    Hr,
    Time,
    Rest,
}

/// Where the columns of a table begin, from its left edge: one share of the width each ([`metrics::SET_COLUMNS`]), the
/// plan's only in a card whose sets have plans of their own.
struct Columns {
    /// The left edge of each column, and last the right edge of the table.
    edges: Vec<f32>,
}

impl Columns {
    fn new(width: f32, plan: bool) -> Columns {
        let shares: Vec<f32> =
            metrics::SET_COLUMNS.iter().enumerate().map(|(index, share)| if index == Column::Plan as usize && !plan { 0.0 } else { *share }).collect();
        let total = shares.iter().sum::<f32>().max(f32::EPSILON);
        let mut edges = vec![0.0];
        let mut x = 0.0;
        for share in shares {
            x += width * share / total;
            edges.push(x);
        }
        Columns { edges }
    }

    /// The cells of `row` from column `from` to column `to`, both included.
    fn span(&self, row: Rect, from: Column, to: Column) -> Rect {
        let left = self.edges.get(from as usize).copied().unwrap_or(0.0);
        let right = self.edges.get(to as usize + 1).copied().unwrap_or(row.width());
        Rect::from_min_max(pos2(row.left() + left, row.top()), pos2(row.left() + right, row.bottom()))
    }
}

/// Draws into the cells `rect`: left to right, centred on the line, cut at the cell's edge.
fn cell<R>(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = rect.shrink2(vec2(metrics::CELL_PADDING, 0.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing.x = metrics::CELL_PADDING / 2.0;
    add(&mut child)
}

/// One line of the table, as wide as the card.
fn line(ui: &mut Ui) -> Rect {
    ui.allocate_exact_size(vec2(ui.available_width(), metrics::SET_ROW_HEIGHT), Sense::hover()).0
}

/// The table of a card's sets: number, weight, reps, the plan when the sets have their own, heart rate, time and rest.
fn table(ui: &mut Ui, t: &Tokens, group: &ExerciseGroupView, editing: &mut Option<SetEdit>, action: &mut Option<Action>) {
    let plan = group.sets.iter().any(|set| set.plan.is_some());
    let card: Vec<SetRef> = group.sets.iter().map(|set| set.set).collect();
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let columns = Columns::new(ui.available_width(), plan);
        let rect = line(ui);
        let heading = [
            // A sign, not a word: the same in both languages, as in the mock-up.
            (Column::Number, "#"),
            (Column::Weight, tl!("Weight")),
            (Column::Reps, tl!("Reps")),
            (Column::Plan, tl!("Plan")),
            (Column::Hr, tl!("HR")),
            (Column::Time, tl!("Time")),
            (Column::Rest, tl!("Rest")),
        ];
        for (column, name) in heading.into_iter().filter(|(column, _)| plan || *column != Column::Plan) {
            cell(ui, columns.span(rect, column, column), |ui| {
                ui.add(Label::new(RichText::new(name).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim)).selectable(false));
            });
        }
        ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, t.border));
        for (index, set) in group.sets.iter().enumerate() {
            if index > 0 {
                // A faint line between two sets, fainter than the one under the heading.
                let y = ui.cursor().top();
                ui.painter().hline(rect.x_range(), y, Stroke::new(1.0, t.card.lerp_to_gamma(t.border, 0.5)));
            }
            let row = Row { columns: &columns, number: index.saturating_add(1), set, plan, card: &card };
            set_row(ui, t, &row, editing, action);
        }
    });
}

/// What a line says besides the values: that the user added the set, why it does not count, and the way back.
struct Note {
    text: String,
    /// The link that counts the set again, and the `counted` it sets.
    link: Option<(&'static str, Option<bool>)>,
}

fn note(set: &SetView) -> Option<Note> {
    let mut parts = Vec::new();
    if set.added {
        parts.push(tl!("added"));
    }
    let counted = set_edit_ui::count_again(set.count_reason);
    let link = match set.count_reason {
        // Back to the rule: a set that is not empty counts again.
        CountReason::ExcludedByUser => {
            parts.push(tl!("not counted"));
            counted.map(|counted| (tl!("restore"), counted))
        }
        // Spec §2, item 12: counted over the rule.
        CountReason::Empty => {
            parts.push(tl!("empty, not counted"));
            counted.map(|counted| (tl!("count it"), counted))
        }
        // A workout left out says so in its header, once.
        CountReason::Counted | CountReason::ActivityExcluded => None,
    };
    (!parts.is_empty()).then(|| Note { text: parts.join(" · "), link })
}

/// A line of the table and what it needs to know of its card.
struct Row<'a> {
    columns: &'a Columns,
    /// The set's number in its card, from 1.
    number: usize,
    set: &'a SetView,
    /// Whether the card's table has the plan's column.
    plan: bool,
    /// Every set of the card, for the picker the line's editor opens.
    card: &'a [SetRef],
}

/// One set: its number and values, and its note in place of the time and rest when it fits there, else on a line of its
/// own under them. A click turns the line into the editor: the weight and the reps become fields, and the actions on the
/// set and why the last one failed come under it, on the backing of the mock-up's editor.
fn set_row(ui: &mut Ui, t: &Tokens, row: &Row<'_>, editing: &mut Option<SetEdit>, action: &mut Option<Action>) {
    let Row { columns, number, set, plan, card } = *row;
    let mut edit = editing.as_mut().filter(|edit| edit.set == set.set);
    let off = matches!(set.count_reason, CountReason::ExcludedByUser | CountReason::Empty);
    let color = if set.count_reason == CountReason::Counted { t.text } else { t.text_dim };
    // A set that does not count is struck through (spec §2, items 8 and 12).
    let plain = |text: &str| {
        let text = RichText::new(text).color(color);
        Label::new(if off { text.strikethrough() } else { text }).selectable(false)
    };
    // The backing goes under the line once its height is known; the shape is reserved first so it is painted below.
    let backing = ui.painter().add(Shape::Noop);
    let top = ui.cursor().top();
    let height = if edit.is_some() { metrics::EDIT_ROW_HEIGHT } else { metrics::SET_ROW_HEIGHT };
    let rect = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover()).0;
    let mut highlighted = edit.is_some();
    if edit.is_none() {
        // Under the links of the line, which take their own clicks.
        let line = ui.interact(rect, ui.make_persistent_id(("set-line", set.set)), Sense::click());
        line.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), tl!("Edit set {}").replace("{}", &number.to_string())));
        highlighted = line.hovered();
        if line.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            *action = Some(Action::Edit(Request::Edit(set.set)));
        }
    }
    cell(ui, columns.span(rect, Column::Number, Column::Number), |ui| ui.add(plain(&number.to_string())));

    let correction = set.correction.as_ref();
    let recorded = set.recorded.as_ref();
    let weight = set.effective.weight_kg.map_or_else(|| NO_VALUE.to_owned(), numbers::kg);
    let recorded_weight = correction
        .and_then(|correction| correction.weight_kg)
        .and(recorded)
        .map(|recorded| recorded.weight_kg.map_or_else(|| NO_VALUE.to_owned(), numbers::kg_value));
    let reps = set.effective.reps.map_or_else(|| NO_VALUE.to_owned(), |reps| reps.to_string());
    let recorded_reps = correction
        .and_then(|correction| correction.reps)
        .and(recorded)
        .map(|recorded| recorded.reps.map_or_else(|| NO_VALUE.to_owned(), |reps| reps.to_string()));
    let mut request = None;
    if let Some(edit) = edit.as_deref_mut() {
        cell(ui, columns.span(rect, Column::Weight, Column::Weight), |ui| set_edit_ui::weight_cell(ui, edit, recorded_weight.as_deref(), &mut request));
        cell(ui, columns.span(rect, Column::Reps, Column::Reps), |ui| set_edit_ui::reps_cell(ui, edit, recorded_reps.as_deref(), &mut request));
    } else {
        for (column, value, was) in [(Column::Weight, weight, recorded_weight), (Column::Reps, reps, recorded_reps)] {
            cell(ui, columns.span(rect, column, column), |ui| match was {
                Some(was) if !off => widgets::set_value(ui, &value, Some(&was)),
                _ => {
                    ui.add(plain(&value));
                }
            });
        }
    }
    if plan && let Some(text) = set.plan.and_then(|plan| plan_text(plan.reps.map(|reps| reps.to_string()), plan.weight_kg)) {
        cell(ui, columns.span(rect, Column::Plan, Column::Plan), |ui| {
            ui.add(Label::new(RichText::new(text).size(metrics::SMALL_TEXT_SIZE).color(t.text_dim)).selectable(false));
        });
    }
    let hr = set.hr_avg.map_or_else(|| NO_VALUE.to_owned(), |hr| hr.to_string());
    cell(ui, columns.span(rect, Column::Hr, Column::Hr), |ui| ui.add(plain(&hr)));

    // The editor has the actions of the note under the line, so the note gives way to the time and the rest.
    let note = if edit.is_some() { None } else { note(set) };
    let fits = note.as_ref().is_some_and(|note| note_width(ui, note) <= columns.span(rect, Column::Time, Column::Rest).width() - 2.0 * metrics::CELL_PADDING);
    if let (Some(note), true) = (&note, fits) {
        cell(ui, columns.span(rect, Column::Time, Column::Rest), |ui| show_note(ui, t, set.set, note, action));
    } else {
        let duration = set.duration_ms.map_or_else(|| NO_VALUE.to_owned(), |ms| numbers::duration(u64::from(ms)));
        let rest = set.rest_after_ms.map_or_else(|| NO_VALUE.to_owned(), |ms| numbers::duration(u64::from(ms)));
        cell(ui, columns.span(rect, Column::Time, Column::Time), |ui| ui.add(plain(&duration)));
        cell(ui, columns.span(rect, Column::Rest, Column::Rest), |ui| ui.add(plain(&rest)));
        if let Some(note) = &note {
            // A narrow window: the note under the values, from the weight on.
            let below = line(ui);
            cell(ui, columns.span(below, Column::Weight, Column::Rest), |ui| show_note(ui, t, set.set, note, action));
        }
    }
    if let Some(edit) = edit {
        ui.add_space(metrics::GAP / 2.0);
        set_edit_ui::actions(ui, edit, set, card, &mut request);
        ui.add_space(metrics::GAP / 2.0);
    }
    if let Some(request) = request {
        *action = Some(Action::Edit(request));
    }
    if highlighted {
        let bottom = ui.cursor().top();
        ui.painter().set(backing, Shape::rect_filled(Rect::from_x_y_ranges(rect.x_range(), top..=bottom), 0.0, t.edit_row));
    }
}

/// How wide `note` is on one line.
fn note_width(ui: &Ui, note: &Note) -> f32 {
    let font = TextStyle::Body.resolve(ui.style());
    let width = |text: &str| ui.painter().layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER).size().x;
    let spacing = metrics::CELL_PADDING / 2.0;
    let link = note.link.map_or(0.0, |(link, _)| 2.0 * spacing + width("·") + width(link));
    width(&note.text) + link
}

/// The note in italics, and its link: `empty, not counted · count it`.
fn show_note(ui: &mut Ui, t: &Tokens, set: SetRef, note: &Note, action: &mut Option<Action>) {
    ui.add(Label::new(RichText::new(&note.text).italics().color(t.text_dim)).selectable(false));
    if let Some((link, counted)) = note.link {
        ui.add(Label::new(RichText::new("·").color(t.text_dim)).selectable(false));
        if ui.link(link).clicked() {
            *action = Some(Action::Count(set, counted));
        }
    }
}

#[cfg(test)]
#[path = "activity_detail_ui_tests.rs"]
mod tests;
