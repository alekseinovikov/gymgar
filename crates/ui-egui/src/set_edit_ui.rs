//! Correcting the sets of the open activity (activity import spec §2, items 9–11; the approved mock-up
//! `workouts-edit.html`): the editor a click opens in the line of a set, the exercise picker, and adding a set the watch
//! did not record.
//!
//! The editor has the set's weight and reps as fields and, under the line, "Save" (`activity.editSets` with only what
//! changed), "Cancel", the way back to the watch's values (`activity.resetSets`; a set the user added is removed instead,
//! `activity.removeAddedSet`), counting the set or not (`activity.editSets` with `counted`) and "Other exercise…". The
//! picker searches the exercises (`exercise.search`: when the text changes, never on every frame), those the user has
//! done first, and gives the chosen one to every set of a card or to one (`activity.editSets`), or adds a set of it after
//! the card's last (`activity.addSet`), whose line then opens for its weight and reps.
//!
//! What the user typed is read here before anything is sent: a weight takes a comma or a dot, and what is not a weight
//! or a count in range is refused under the line without asking the engine. The ranges are the domain's (`Mass`,
//! `Reps`), which the engine holds the commands to as well. A command that fails says why in the same place, in the
//! user's language. Drawing only collects what was asked ([`Request`]); [`run`] does it once the frame is drawn, since a
//! change reads the activity again.

use std::collections::HashSet;

use egui::{Context, Id, Key, Label, Pos2, Response, RichText, ScrollArea, Ui};
use gymgar_domain::{CountReason, Mass, Reps, SetRef};
use gymgar_engine::{ActivityView, ExerciseGroupView, ExerciseSearchView, Outcome, SetValuesView, SetView};
use serde_json::{Map, Value};

use crate::activity_detail_ui::{exercise_name, set_param};
use crate::i18n::trn;
use crate::state::{Picker, PickerTarget, SetEdit};
use crate::theme::{Tokens, metrics};
use crate::{GymgarApp, numbers, tl, widgets};

/// How many exercises a search shows: more than the picker's list shows at once (it scrolls), few enough for a frame.
const SEARCH_LIMIT: u32 = 50;
/// The longest query `exercise.search` takes, in characters.
const QUERY_MAX_CHARS: usize = 100;

/// What the user asked of the editor or the picker. It runs once the frame is drawn ([`run`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Request {
    /// Turn the line of this set into the editor: a click on it.
    Edit(SetRef),
    /// Send what changed in the editor.
    Save,
    /// Close the editor; what was typed goes.
    Cancel,
    /// `activity.resetSets`: the set as the watch recorded it.
    Reset,
    /// `activity.removeAddedSet`.
    Remove,
    /// `activity.editSets` with `counted` only.
    Count(Option<bool>),
    /// Open the picker and search for what it starts with.
    Pick(Box<Picker>),
    /// The text of the search changed.
    Search,
    /// Give the chosen exercise to the sets, or add the set.
    Choose,
    /// Close the picker.
    ClosePicker,
}

// ----- the editor

/// The weight cell of the line being edited: the field, the unit, and what the watch recorded struck through when the
/// user corrected it. Enter in the field saves.
pub(crate) fn weight_cell(ui: &mut Ui, edit: &mut SetEdit, recorded: Option<&str>, request: &mut Option<Request>) {
    let t = Tokens::get(ui.ctx());
    let field = widgets::number_field(ui, &mut edit.weight, tl!("Weight"));
    // The editor has just opened: the weight is what one corrects most.
    if std::mem::take(&mut edit.focus) {
        field.request_focus();
    }
    saves_on_enter(ui, &field, request);
    ui.add(Label::new(RichText::new(tl!("kg")).color(t.text_dim)).selectable(false));
    if let Some(recorded) = recorded {
        widgets::recorded_value(ui, recorded);
    }
}

/// The reps cell of the line being edited: the field, and what the watch recorded when the user corrected it.
pub(crate) fn reps_cell(ui: &mut Ui, edit: &mut SetEdit, recorded: Option<&str>, request: &mut Option<Request>) {
    let field = widgets::number_field(ui, &mut edit.reps, tl!("Reps"));
    saves_on_enter(ui, &field, request);
    if let Some(recorded) = recorded {
        widgets::recorded_value(ui, recorded);
    }
}

fn saves_on_enter(ui: &Ui, field: &Response, request: &mut Option<Request>) {
    // A one-line field gives the keyboard up on Enter.
    if field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
        *request = Some(Request::Save);
    }
}

/// The actions under the line being edited (the mock-up's row of buttons), and why the last one did not go through.
/// `card` is every set of the set's card, for "Other exercise…".
pub(crate) fn actions(ui: &mut Ui, edit: &SetEdit, set: &SetView, card: &[SetRef], request: &mut Option<Request>) {
    let t = Tokens::get(ui.ctx());
    widgets::toolbar(ui, |ui| {
        if widgets::primary_button(ui, tl!("Save")).clicked() {
            *request = Some(Request::Save);
        }
        if widgets::secondary_button(ui, tl!("Cancel")).clicked() {
            *request = Some(Request::Cancel);
        }
        // A recorded set goes back to the record; an added one has none, so it goes.
        let (label, back) = if set.added { (tl!("Remove"), Request::Remove) } else { (tl!("Reset to watch values"), Request::Reset) };
        if widgets::secondary_button(ui, label).clicked() {
            *request = Some(back);
        }
        let (label, counted) = match count_again(set.count_reason) {
            Some(counted) => (tl!("Count"), counted),
            None => (tl!("Don't count this set"), Some(false)),
        };
        if widgets::secondary_button(ui, label).clicked() {
            *request = Some(Request::Count(counted));
        }
        if widgets::secondary_button(ui, tl!("Other exercise…")).clicked() {
            // Only this set by default: the rest of the card is not what the user was correcting.
            let target = PickerTarget::Sets { card: card.to_vec(), one: set.set };
            *request = Some(Request::Pick(Box::new(picker_for(target, false, String::new(), None))));
        }
    });
    if let Some(error) = &edit.error {
        ui.add_space(metrics::GAP / 2.0);
        ui.label(RichText::new(error).color(t.warning));
    }
}

/// The `counted` that counts a set again when it does not count by itself: one the user left out goes back to the
/// empty-set rule (`None`, so a set that is not empty counts); an empty one is counted over the rule (`Some(true)`).
/// `None` for a set that is not left out by itself.
pub(crate) fn count_again(reason: CountReason) -> Option<Option<bool>> {
    match reason {
        CountReason::ExcludedByUser => Some(None),
        CountReason::Empty => Some(Some(true)),
        // A workout left out leaves its sets out with it; that is undone in its header.
        CountReason::Counted | CountReason::ActivityExcluded => None,
    }
}

// ----- the picker

/// What "Choose exercise" on the card of the sets without an exercise asks for: the picker for the whole card (spec
/// §2, item 10: by default), or for its first set alone. `None` for a card without sets.
pub(crate) fn choose_for_card(group: &ExerciseGroupView) -> Option<Request> {
    let card: Vec<SetRef> = group.sets.iter().map(|set| set.set).collect();
    let one = *card.first()?;
    Some(Request::Pick(Box::new(picker_for(PickerTarget::Sets { card, one }, true, String::new(), None))))
}

/// What "+ Add set" under a card asks for: the picker for a set after the card's last, opened on the card's exercise,
/// found and chosen.
pub(crate) fn add_to_card(group: &ExerciseGroupView) -> Request {
    let after = group.sets.last().map(|set| set.set);
    let query = group.exercise.as_ref().map(|exercise| exercise_name(exercise).chars().take(QUERY_MAX_CHARS).collect()).unwrap_or_default();
    Request::Pick(Box::new(picker_for(PickerTarget::NewSet { after }, false, query, group.exercise.clone())))
}

fn picker_for(target: PickerTarget, whole_group: bool, query: String, chosen: Option<gymgar_engine::ExerciseView>) -> Picker {
    Picker { target, query, whole_group, hits: Vec::new(), chosen, error: None, focus: true }
}

/// The exercise picker over the open activity, its top right corner at `corner`: the search, what it found (the
/// exercises the user has done with how many times, then Garmin's catalog), whether the exercise goes to the whole card,
/// and "Choose".
pub(crate) fn picker(ctx: &Context, corner: Pos2, picker: &mut Picker, request: &mut Option<Request>) {
    widgets::popover(ctx, Id::unique("exercise-picker"), corner, metrics::POPOVER_WIDTH, tl!("Choose exercise"), |ui| {
        let t = Tokens::get(ui.ctx());
        let search = widgets::search_field(ui, &mut picker.query, tl!("Search exercises"), QUERY_MAX_CHARS);
        if std::mem::take(&mut picker.focus) {
            search.request_focus();
        }
        if search.changed() {
            *request = Some(Request::Search);
        }
        ScrollArea::vertical()
            .id_salt("exercise-picker-hits")
            .max_height(metrics::POPOVER_LIST_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| hits(ui, picker));
        // A new set is one set.
        if matches!(picker.target, PickerTarget::Sets { .. }) {
            widgets::small_checkbox(ui, &mut picker.whole_group, tl!("For all sets of the card"));
        }
        if let Some(error) = &picker.error {
            ui.label(RichText::new(error).color(t.warning));
        }
        ui.horizontal(|ui| {
            if ui.add_enabled_ui(picker.chosen.is_some(), |ui| widgets::primary_button(ui, tl!("Choose"))).inner.clicked() {
                *request = Some(Request::Choose);
            }
            if widgets::secondary_button(ui, tl!("Cancel")).clicked() {
                *request = Some(Request::ClosePicker);
            }
        });
    });
}

/// What the search found, the exercises the user has done first; a click chooses one.
fn hits(ui: &mut Ui, picker: &mut Picker) {
    if picker.hits.is_empty() {
        // An empty search lists only what the user has done, which may be nothing yet.
        widgets::hint(ui, if picker.query.trim().is_empty() { tl!("Type the name of an exercise") } else { tl!("Nothing found") });
        return;
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut clicked = None;
    for (heading, used) in [(tl!("Done before"), true), (tl!("Garmin catalog"), false)] {
        let mut part = picker.hits.iter().filter(|hit| (hit.times_used > 0) == used).peekable();
        if part.peek().is_none() {
            continue;
        }
        ui.add_space(metrics::GAP / 2.0);
        widgets::list_heading(ui, heading);
        for hit in part {
            let times = used.then(|| trn(u64::from(hit.times_used), tl!("{} time"), tl!("{} times (2 to 4)"), tl!("{} times")));
            let selected = picker.chosen.as_ref().is_some_and(|chosen| chosen.id == hit.exercise.id);
            if widgets::option_row(ui, exercise_name(&hit.exercise), times.as_deref(), selected).clicked() {
                clicked = Some(hit.exercise.clone());
            }
        }
    }
    if clicked.is_some() {
        picker.chosen = clicked;
    }
}

// ----- doing what was asked

/// Does what the user asked of the editor or the picker on activity `id`.
pub(crate) fn run(app: &mut GymgarApp, id: i64, request: Request) {
    match request {
        Request::Edit(set) => open_editor(app, set),
        Request::Save => save(app, id),
        Request::Cancel => app.ui.activities.editing = None,
        Request::Reset => change_set(app, id, "activity.resetSets", |set, params| {
            params.insert("sets".to_owned(), Value::Array(vec![set_param(set)]));
        }),
        Request::Remove => change_set(app, id, "activity.removeAddedSet", |set, params| {
            params.insert("set".to_owned(), set_param(set));
        }),
        Request::Count(counted) => change_set(app, id, "activity.editSets", |set, params| {
            params.insert("sets".to_owned(), Value::Array(vec![set_param(set)]));
            params.insert("counted".to_owned(), counted.map_or(Value::Null, Value::Bool));
        }),
        Request::Pick(picker) => {
            app.ui.activities.picker = Some(*picker);
            search(app);
        }
        Request::Search => search(app),
        Request::Choose => choose(app, id),
        Request::ClosePicker => app.ui.activities.picker = None,
    }
}

/// The set `set` of the open activity, as last read.
pub(crate) fn find_set(detail: Option<&ActivityView>, set: SetRef) -> Option<&SetView> {
    detail?.groups.iter().flat_map(|group| &group.sets).find(|view| view.set == set)
}

/// Params naming activity `id`, for the rest to be added to. Built by hand, not with `json!`, which unwraps inside.
fn activity_params(id: i64) -> Map<String, Value> {
    let mut params = Map::new();
    params.insert("id".to_owned(), Value::from(id));
    params
}

/// Opens the editor on `set` with the values it has now: the weight as the language writes it, empty for none.
fn open_editor(app: &mut GymgarApp, set: SetRef) {
    let Some(view) = find_set(app.detail(), set) else { return };
    let weight = view.effective.weight_kg.map(numbers::kg_value).unwrap_or_default();
    let reps = view.effective.reps.map(|reps| reps.to_string()).unwrap_or_default();
    let (weight_shown, reps_shown) = (weight.clone(), reps.clone());
    app.ui.activities.editing = Some(SetEdit { set, weight, reps, weight_shown, reps_shown, error: None, focus: true });
}

/// Runs `command` on the set being edited, with the params `fill` adds to the activity's id.
fn change_set(app: &mut GymgarApp, id: i64, command: &str, fill: impl FnOnce(SetRef, &mut Map<String, Value>)) {
    let Some(set) = app.ui.activities.editing.as_ref().map(|edit| edit.set) else { return };
    let mut params = activity_params(id);
    fill(set, &mut params);
    let result = app.run(command, Value::Object(params));
    finish_edit(app, result);
}

/// The editor closes when its command went through, and says why when it did not.
fn finish_edit(app: &mut GymgarApp, result: Result<Outcome, String>) {
    match result {
        Ok(_) => app.ui.activities.editing = None,
        Err(text) => {
            if let Some(edit) = app.ui.activities.editing.as_mut() {
                edit.error = Some(text);
            }
        }
    }
}

/// "Save": `activity.editSets` with the fields that changed, after reading them; nothing changed closes the editor.
fn save(app: &mut GymgarApp, id: i64) {
    let Some(edit) = app.ui.activities.editing.as_ref() else { return };
    let set = edit.set;
    let Some(view) = find_set(app.detail(), set) else {
        // Gone meanwhile (an agent removed it): there is nothing to save to.
        app.ui.activities.editing = None;
        return;
    };
    match changes(edit, &view.effective) {
        Err(text) => {
            if let Some(edit) = app.ui.activities.editing.as_mut() {
                edit.error = Some(text.to_owned());
            }
        }
        Ok((None, None)) => app.ui.activities.editing = None,
        Ok((weight, reps)) => {
            let mut params = activity_params(id);
            params.insert("sets".to_owned(), Value::Array(vec![set_param(set)]));
            if let Some(weight) = weight {
                params.insert("weightKg".to_owned(), Value::from(weight.kg()));
            }
            if let Some(reps) = reps {
                params.insert("reps".to_owned(), Value::from(reps.get()));
            }
            let result = app.run("activity.editSets", Value::Object(params));
            finish_edit(app, result);
        }
    }
}

/// A field of the editor, read.
#[derive(Debug, PartialEq)]
enum Field<T> {
    Empty,
    /// A value in range.
    Value(T),
    /// Not a value, or out of range.
    Invalid,
}

/// What the editor's fields change in a set whose values are now `current`: the weight and the reps to send, `None`
/// for a field left as it is. A field whose text is the one it opened with is left as it is, whatever it reads as (the
/// weight is shown rounded); another text is read, and one that is not a value in range is refused with the reason, as
/// is one emptied, since no command takes a value away (zero is a value).
fn changes(edit: &SetEdit, current: &SetValuesView) -> Result<(Option<Mass>, Option<Reps>), &'static str> {
    let untouched = |text: &str, shown: &str| text.trim() == shown.trim();
    let weight = if untouched(&edit.weight, &edit.weight_shown) {
        None
    } else {
        change(read_kg(&edit.weight), current.weight_kg.and_then(|kg| Mass::from_kg(kg).ok()), tl!("Enter a weight from 0 to 1000 kg."))?
    };
    let reps = if untouched(&edit.reps, &edit.reps_shown) {
        None
    } else {
        change(read_reps(&edit.reps), current.reps.and_then(|n| Reps::new(u32::from(n)).ok()), tl!("Enter a whole number of reps from 0 to 1000."))?
    };
    Ok((weight, reps))
}

/// The value to send for `field` in place of `current`: `None` when it is the same.
fn change<T: PartialEq>(field: Field<T>, current: Option<T>, error: &'static str) -> Result<Option<T>, &'static str> {
    match (field, current) {
        (Field::Empty, None) => Ok(None),
        (Field::Value(value), Some(current)) if value == current => Ok(None),
        (Field::Value(value), _) => Ok(Some(value)),
        (Field::Empty, Some(_)) | (Field::Invalid, _) => Err(error),
    }
}

/// A weight as typed: digits with at most one decimal comma or dot, 0 to 1000 kg. No sign and no exponent: nobody types
/// them for a weight, and `f64`'s parser would take `inf` and `1e3`.
fn read_kg(text: &str) -> Field<Mass> {
    let text = text.trim();
    if text.is_empty() {
        return Field::Empty;
    }
    let number = text.replace(',', ".");
    let digits = number.chars().filter(char::is_ascii_digit).count();
    let points = number.chars().filter(|&c| c == '.').count();
    if digits == 0 || points > 1 || digits + points != number.chars().count() {
        return Field::Invalid;
    }
    number.parse::<f64>().ok().and_then(|kg| Mass::from_kg(kg).ok()).map_or(Field::Invalid, Field::Value)
}

/// A count of reps as typed: digits only, 0 to 1000.
fn read_reps(text: &str) -> Field<Reps> {
    let text = text.trim();
    if text.is_empty() {
        return Field::Empty;
    }
    if !text.chars().all(|c| c.is_ascii_digit()) {
        return Field::Invalid;
    }
    // Too many digits for a `u32` is out of range too.
    text.parse::<u32>().ok().and_then(|n| Reps::new(n).ok()).map_or(Field::Invalid, Field::Value)
}

/// `exercise.search` for the picker's text: the hits replace the last ones, and the chosen exercise stays chosen only
/// when it is still among them, so "Choose" never gives an exercise the list does not show.
fn search(app: &mut GymgarApp) {
    let Some(query) = app.ui.activities.picker.as_ref().map(|picker| picker.query.clone()) else { return };
    let mut params = Map::new();
    params.insert("query".to_owned(), Value::String(query));
    params.insert("limit".to_owned(), Value::from(SEARCH_LIMIT));
    let found = match app.run("exercise.search", Value::Object(params)) {
        Ok(Outcome::Done(value)) => serde_json::from_value::<ExerciseSearchView>(value).map(|view| view.items).map_err(|error| {
            log::error!("cannot read the answer of exercise.search: {error}");
            tl!("Something went wrong. Please try again.").to_owned()
        }),
        Ok(Outcome::Started(_)) => Err(tl!("Something went wrong. Please try again.").to_owned()),
        Err(text) => Err(text),
    };
    let Some(picker) = app.ui.activities.picker.as_mut() else { return };
    match found {
        Ok(hits) => {
            picker.chosen = picker.chosen.take().filter(|chosen| hits.iter().any(|hit| hit.exercise.id == chosen.id));
            picker.hits = hits;
            picker.error = None;
        }
        Err(text) => {
            picker.hits.clear();
            picker.chosen = None;
            picker.error = Some(text);
        }
    }
}

/// "Choose": the exercise goes to the sets (`activity.editSets`), or a set of it is added after the card's last
/// (`activity.addSet`) and its line opens, since it has no weight or reps yet.
fn choose(app: &mut GymgarApp, id: i64) {
    let Some(picker) = app.ui.activities.picker.as_ref() else { return };
    let Some(exercise) = picker.chosen.as_ref().map(|exercise| exercise.id.clone()) else { return };
    let mut params = activity_params(id);
    params.insert("exercise".to_owned(), Value::String(exercise));
    let (command, adding) = match &picker.target {
        PickerTarget::Sets { card, one } => {
            let sets = if picker.whole_group { card.iter().copied().map(set_param).collect() } else { vec![set_param(*one)] };
            params.insert("sets".to_owned(), Value::Array(sets));
            ("activity.editSets", false)
        }
        PickerTarget::NewSet { after } => {
            if let Some(after) = after {
                params.insert("after".to_owned(), set_param(*after));
            }
            ("activity.addSet", true)
        }
    };
    // The sets added before, to tell the new one by.
    let before = added_sets(app.detail());
    match app.run(command, Value::Object(params)) {
        Ok(_) => {
            app.ui.activities.picker = None;
            if adding && let Some(set) = added_sets(app.detail()).difference(&before).next().copied() {
                open_editor(app, set);
            }
        }
        Err(text) => {
            if let Some(picker) = app.ui.activities.picker.as_mut() {
                picker.error = Some(text);
            }
        }
    }
}

/// The sets the user added to the open activity.
fn added_sets(detail: Option<&ActivityView>) -> HashSet<SetRef> {
    detail.into_iter().flat_map(|detail| &detail.groups).flat_map(|group| &group.sets).filter(|set| set.added).map(|set| set.set).collect()
}

#[cfg(test)]
#[path = "set_edit_ui_tests.rs"]
mod tests;
