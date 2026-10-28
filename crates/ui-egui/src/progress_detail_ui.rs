//! The page of one exercise, which a tile of Progress opens (exercise progress spec §2, item 3): its name and what it has
//! been (how many sessions since when, the target and the program), the advice with the reasons for it, the chart of the
//! weight month by month and the list of its sessions, newest first. A click on a session opens its workout in
//! Activities.
//!
//! It only draws [`GymgarApp::progress_detail`], the answer of `progress.get`: the advice, the points of the chart and
//! the sessions are the engine's. What the advice says in words, and how a session is written in a line, is
//! presentation. The texts of the advice are in the table of the spec (§7, "Тексты подсказки").

use egui::{Align, Color32, Label, Layout, Rect, Response, RichText, Role, ScrollArea, Ui, UiBuilder, WidgetInfo, pos2, vec2};
use gymgar_engine::{AdviceKind, ProgressItemView, ProgressView, SessionView, SetBriefView};

use crate::activity_detail_ui::exercise_name;
use crate::i18n::trn;
use crate::progress_ui::{advice_tone, can_add_title, working_text};
use crate::theme::{Tokens, metrics};
use crate::widgets::Tone;
use crate::{GymgarApp, dates, numbers, progress_chart, tl, widgets};

/// The target for which the user's rule expects about 8 reps with the next weight; for any other the advice says "a couple
/// of reps fewer" (spec §7, the note under the table).
const USUAL_TARGET: u16 = 10;

/// What the user asked for while the page was drawn. It runs once the frame is drawn, since both read the window's data
/// again.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    /// Back to the tiles.
    Back,
    /// Open the workout of the session with this activity id.
    Session(i64),
}

/// Draws the page of the exercise that is open: the way back, then the page, scrolled. An exercise that could not be read
/// says why; one that has not been read yet shows its name only.
pub(crate) fn show(app: &mut GymgarApp, ui: &mut Ui) {
    let mut action = None;
    if widgets::secondary_button(ui, tl!("Back")).clicked() {
        action = Some(Action::Back);
    }
    ui.add_space(metrics::GAP);
    // Each exercise keeps its own scroll: a page opened after another starts at its top, and one come back to (from a
    // workout, say) is where it was left.
    let key = app.ui.progress.open.clone().unwrap_or_default();
    ScrollArea::vertical().id_salt(("progress-detail", key)).auto_shrink(false).show(ui, |ui| match app.progress_detail() {
        Some(view) => action = action.or(page(ui, view)),
        None => unavailable(app, ui),
    });
    match action {
        Some(Action::Back) => app.open_exercise(None),
        Some(Action::Session(id)) => app.open_activity(id),
        None => {}
    }
}

/// What stands where the page would: the name of the exercise (the tile's, else its key) and why the page could not be
/// read, if it could not.
fn unavailable(app: &GymgarApp, ui: &mut Ui) {
    let key = app.ui.progress.open.as_deref().unwrap_or_default();
    let name = app.progress().iter().find(|item| item.exercise.id == key).map_or(key, |item| exercise_name(&item.exercise));
    widgets::page_title(ui, name, None);
    if let Some(text) = app.progress_detail_error() {
        ui.add_space(metrics::GAP);
        widgets::warning_box(ui, text);
    }
}

/// The page of `view`. Returns what the user clicked, if anything.
fn page(ui: &mut Ui, view: &ProgressView) -> Option<Action> {
    widgets::page_title(ui, exercise_name(&view.exercise), subtitle(view).as_deref());
    ui.add_space(metrics::GAP);
    let Some(item) = view.summary.as_ref() else {
        widgets::hint(ui, tl!("No sessions of this exercise yet"));
        return None;
    };
    if let Some((title, explanation)) = advice_texts(view) {
        advice_card(ui, item, &title, &explanation);
        ui.add_space(metrics::GAP);
    }
    widgets::card(ui, |ui| {
        widgets::card_title(ui, &chart_title(item));
        ui.add_space(metrics::GAP);
        progress_chart::show(ui, view, metrics::CHART_HEIGHT);
    });
    ui.add_space(metrics::GAP * 2.0);
    sessions(ui, view, item)
}

/// The advice with its reasons, in a card: the accent border for what asks to add weight.
fn advice_card(ui: &mut Ui, item: &ProgressItemView, title: &str, explanation: &str) {
    let t = Tokens::get(ui.ctx());
    let tone = advice_tone(item.advice.kind);
    let border = if tone == Tone::Accent { t.accent } else { t.border };
    widgets::card_with_border(ui, border, |ui| {
        widgets::advice_title(ui, title, tone);
        ui.add_space(metrics::GAP / 2.0);
        ui.scope(|ui| {
            // Never wider than the card: `set_max_width` would let a long text run past a narrow window and widen the page.
            ui.set_max_width(metrics::ADVICE_TEXT_MAX_WIDTH.min(ui.available_width()));
            ui.add(Label::new(RichText::new(explanation).color(t.text)).selectable(false));
        });
    });
}

/// The title of the card of the chart: what it is the weight for, or that it shows reps.
fn chart_title(item: &ProgressItemView) -> String {
    match item.chart_reps {
        Some(reps) => trn(u64::from(reps), tl!("Weight at {} rep"), tl!("Weight at {} reps (2 to 4)"), tl!("Weight at {} reps")),
        None => tl!("Reps by month").to_owned(),
    }
}

/// The sessions, newest first, a line each. Returns the session the user clicked.
fn sessions(ui: &mut Ui, view: &ProgressView, item: &ProgressItemView) -> Option<Action> {
    widgets::heading(ui, tl!("Sessions"));
    ui.add_space(metrics::GAP / 2.0);
    let year = progress_chart::reference_year(view);
    let mut opened = None;
    for session in &view.sessions {
        if session_row(ui, session, item.chart_reps, year).clicked() {
            opened = Some(Action::Session(session.activity_id.0));
        }
    }
    opened
}

/// One session: its day, its program, what it did at its working weight and every set of it in a line. Screen readers
/// hear a button named by the day, described by the rest.
fn session_row(ui: &mut Ui, session: &SessionView, chart_reps: Option<u16>, year: i32) -> Response {
    let t = Tokens::get(ui.ctx());
    let date = dates::day_month(session.date, year);
    let program = session.workout_name.as_deref().unwrap_or_default();
    let working = working_text(&session.working, chart_reps);
    let sets = sets_text(&session.sets);
    let response = widgets::clickable_row(ui, metrics::SESSION_ROW_HEIGHT, |ui| {
        let line = ui.max_rect();
        let rest = (line.width() - metrics::SESSION_DATE_WIDTH - metrics::SESSION_WORKING_WIDTH).max(0.0);
        let program_width = rest * metrics::SESSION_PROGRAM_SHARE;
        let columns = [
            (metrics::SESSION_DATE_WIDTH, date.as_str(), t.text, false),
            (program_width, program, t.text_dim, false),
            (metrics::SESSION_WORKING_WIDTH, working.as_str(), t.text, true),
            (rest - program_width, sets.as_str(), t.text_dim, false),
        ];
        let mut left = line.left();
        for (width, text, color, strong) in columns {
            // A little less than its share, so that a text cut short does not run into the next column.
            let cell = Rect::from_min_size(pos2(left, line.top()), vec2((width - metrics::GAP).max(0.0), line.height()));
            left += width;
            cell_text(ui, cell, text, color, strong);
        }
    });
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), &date));
    let description = [program, working.as_str(), sets.as_str()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ");
    ui.ctx().accesskit_node_builder(response.id, |node| node.set_description(description));
    response
}

/// One column of a session's line, in `cell`: its text, cut with an ellipsis when it does not fit.
fn cell_text(ui: &mut Ui, cell: Rect, text: &str, color: Color32, strong: bool) {
    ui.scope_builder(UiBuilder::new().max_rect(cell).layout(Layout::left_to_right(Align::Center)), |ui| {
        let text = RichText::new(text).color(color);
        ui.add(Label::new(if strong { text.strong() } else { text }).truncate().selectable(false));
    });
}

/// The sets of a session in a line, as a lifter writes them: `40×10 · 60×10 · 90×6`. A set without a weight is its reps,
/// one without reps its weight, and one with neither is left out.
pub(crate) fn sets_text(sets: &[SetBriefView]) -> String {
    sets.iter().filter_map(set_text).collect::<Vec<_>>().join(" · ")
}

fn set_text(set: &SetBriefView) -> Option<String> {
    let weight = set.weight_kg.filter(|kg| kg.is_finite() && *kg > 0.0);
    match (weight, set.reps) {
        (Some(kg), Some(reps)) => Some(format!("{}×{reps}", numbers::kg_value(kg))),
        (None, Some(reps)) => Some(reps.to_string()),
        (Some(kg), None) => Some(numbers::kg_value(kg)),
        (None, None) => None,
    }
}

/// The line under the name: how many sessions since when, then the target and the program that asked for it, or either of
/// them alone. `None` for an exercise without history.
pub(crate) fn subtitle(view: &ProgressView) -> Option<String> {
    let item = view.summary.as_ref()?;
    let sessions = trn(u64::from(item.sessions), tl!("{} session"), tl!("{} sessions (2 to 4)"), tl!("{} sessions"));
    let since = fill(tl!("{sessions} since {month}"), &[("{sessions}", &sessions), ("{month}", &dates::month_year(item.first_date))]);
    // The program of the newest session that has one.
    let program = view
        .sessions
        .iter()
        .find_map(|session| session.workout_name.as_deref().map(str::trim).filter(|name| !name.is_empty()))
        .map(|name| tl!("program {}").replace("{}", name));
    let target = item.advice.target.map(|target| tl!("target — {}").replace("{}", &reps(target)));
    let goal = match (target, program) {
        (Some(target), Some(program)) => Some(format!("{target} ({program})")),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    };
    Some(goal.map_or_else(|| since.clone(), |goal| format!("{since} · {goal}")))
}

/// `10 reps`, with the form of the noun for the number.
fn reps(count: u16) -> String {
    trn(u64::from(count), tl!("{} rep"), tl!("{} reps (2 to 4)"), tl!("{} reps"))
}

/// The weeks of a pause of `days`, whole ones.
fn weeks(days: i64) -> u64 {
    u64::try_from(days / 7).unwrap_or(0)
}

/// `template` with each `{name}` of `parts` replaced by its text.
fn fill(template: &str, parts: &[(&str, &str)]) -> String {
    parts.iter().fold(template.to_owned(), |text, (name, value)| text.replace(name, value))
}

/// What the advice card says: the title and the explanation, from the table of the spec (§7). `None` for an exercise
/// without history. Dates are written against the year of the latest session, so only one of another year says its year.
/// An answer that lacks what its status rests on gets a shorter text rather than a gap.
pub(crate) fn advice_texts(view: &ProgressView) -> Option<(String, String)> {
    let item = view.summary.as_ref()?;
    let year = progress_chart::reference_year(view);
    Some(match item.advice.kind {
        AdviceKind::Add => add_texts(item, year),
        AdviceKind::Hold => hold_texts(item, year),
        AdviceKind::Break => break_texts(item, year),
        AdviceKind::Bodyweight => (tl!("Bodyweight exercise").to_owned(), tl!("The chart shows reps.").to_owned()),
        AdviceKind::NoTarget => {
            (tl!("No target in the program").to_owned(), tl!("The advice will appear once the exercise is in a program that plans reps.").to_owned())
        }
    })
}

/// "Can add: 85 kg", and the two sessions it rests on and what to expect with the new weight.
fn add_texts(item: &ProgressItemView, year: i32) -> (String, String) {
    let advice = &item.advice;
    let title = can_add_title(advice);
    let first = match advice.basis.as_slice() {
        [previous, last] => fill(
            tl!("{working} two sessions in a row — {dates}."),
            &[("{working}", &working_text(last, item.chart_reps)), ("{dates}", &dates::two_days(previous.date, last.date, year))],
        ),
        _ => fill(tl!("{working} two sessions in a row."), &[("{working}", &working_text(&item.last, item.chart_reps))]),
    };
    let explanation = match (advice.next_weight_kg, advice.target) {
        (Some(weight), Some(target)) => {
            let expect = if target == USUAL_TARGET {
                tl!("With {weight} expect about 8 reps, then build back up to {target}.")
            } else {
                tl!("With {weight} expect a couple of reps fewer, then build back up to {target}.")
            };
            format!("{first} {}", fill(expect, &[("{weight}", &numbers::kg(weight)), ("{target}", &target.to_string())]))
        }
        _ => first,
    };
    (title, explanation)
}

/// "Hold 90 kg" (or "… for one more session"): what was done last time against the target, and what it takes to add
/// weight; after a break, that the count began anew and what the session before it was.
fn hold_texts(item: &ProgressItemView, year: i32) -> (String, String) {
    let advice = &item.advice;
    let weight = numbers::kg(item.last.weight_kg);
    let title = if advice.met_once { tl!("Hold {} for one more session") } else { tl!("Hold {}") }.replace("{}", &weight);
    let working = working_text(&item.last, item.chart_reps);
    let date = dates::day_month(item.last.date, year);
    let said = |template: &str, target: Option<u16>| {
        fill(template, &[("{working}", &working), ("{date}", &date), ("{target}", &target.map(|target| target.to_string()).unwrap_or_default())])
    };
    let mut explanation = match (advice.met_once, advice.target) {
        (true, Some(target)) => said(tl!("{working} — {date}. Hit {target} once more and you can add weight."), Some(target)),
        // Several sets at the working weight: every one of them has to make the target.
        (false, Some(target)) if item.last.reps.len() > 1 => said(
            tl!("Last time {working} with a target of {target} ({date}). Add weight once every set at this weight hits {target} in two sessions in a row."),
            Some(target),
        ),
        (false, Some(target)) => {
            said(tl!("Last time {working} of {target} ({date}). Add weight once you hit {target} in two sessions in a row."), Some(target))
        }
        (_, None) => said(tl!("Last time {working} ({date})."), None),
    };
    if let Some(days) = advice.break_days {
        let count = trn(
            weeks(days),
            tl!("After a {}-week break, we count anew."),
            tl!("After a {}-week break we count anew. (2 to 4)"),
            tl!("After a {}-week break we count anew."),
        );
        explanation = format!("{explanation} {count}");
        if let Some(before) = &advice.before_break {
            explanation = format!("{explanation} {}", tl!("Before the break: {}.").replace("{}", &working_text(before, item.chart_reps)));
        }
    }
    (title, explanation)
}

/// "Break 10 weeks", the last session, and what the first one after the break will be.
fn break_texts(item: &ProgressItemView, year: i32) -> (String, String) {
    let title = match item.advice.break_days {
        Some(days) => trn(weeks(days), tl!("Break {} week"), tl!("Break {} weeks (2 to 4)"), tl!("Break {} weeks")),
        None => tl!("Break").to_owned(),
    };
    let explanation = fill(
        tl!("Last session {date}: {working}. The first session after the break will be the new starting point."),
        &[("{date}", &dates::day_month(item.last.date, year)), ("{working}", &working_text(&item.last, item.chart_reps))],
    );
    (title, explanation)
}

#[cfg(test)]
#[path = "progress_detail_ui_tests.rs"]
mod tests;
