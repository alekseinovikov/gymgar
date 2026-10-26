//! The editor in the line of a set, the exercise picker and adding a set (activity import spec §2, items 9–11), driven
//! the way the user drives them: clicks and typing on the Activities screen, checked against what the engine stored.

use chrono::{DateTime, TimeZone, Utc};
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use gymgar_domain::{ActivityId, ExerciseId, Mass, RecordedSet, Reps, SetRef};
use gymgar_engine::testing::{TestConfig, seed_strength};
use gymgar_engine::{ActivityView, Outcome, SetView};
use serde_json::json;

use crate::GymgarApp;
use crate::i18n::Lang;
use crate::state::Section;
use crate::test_support::{app_seeded_with, harness_sized, pick};

/// The Romanian deadlift: the fixture's exercise with a title in Russian.
const DEADLIFT: ExerciseId = ExerciseId { category: 8, name: Some(23) };
const DEADLIFT_KEY: &str = "deadlift/romanian_deadlift";

/// A window tall enough for a whole workout and the picker.
const TALL: egui::Vec2 = egui::vec2(1200.0, 1400.0);

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 22, 17, minute, 0).single().expect("a valid time")
}

/// Set `index`, three minutes after the one before.
fn set(index: u16, reps: Option<u32>, kg: Option<f64>, exercise: Option<ExerciseId>) -> RecordedSet {
    RecordedSet {
        message_index: index,
        start: at(u32::from(index) * 3),
        duration_ms: 35_000,
        reps: reps.map(|n| Reps::new(n).expect("reps")),
        weight: kg.map(|kg| Mass::from_kg(kg).expect("a weight")),
        exercise,
        step_index: None,
        plan: None,
        hr_avg: Some(120 + index),
        hr_max: None,
        rest_after_ms: Some(125_000),
    }
}

/// Three Romanian deadlifts: 8 × 80, 7 × 80, 7 × 80.
fn deadlifts() -> Vec<RecordedSet> {
    vec![set(0, Some(8), Some(80.0), Some(DEADLIFT)), set(1, Some(7), Some(80.0), Some(DEADLIFT)), set(2, Some(7), Some(80.0), Some(DEADLIFT))]
}

/// Three sets the watch could not name.
fn unnamed() -> Vec<RecordedSet> {
    vec![set(0, Some(10), Some(20.0), None), set(1, Some(10), Some(20.0), None), set(2, Some(9), Some(20.0), None)]
}

/// The Activities screen on the fixtures and `sets` as a strength workout of their own, open beside the list.
fn opened(lang: Lang, sets: Vec<RecordedSet>) -> (Harness<'static, GymgarApp>, ActivityId) {
    let (app, _handles, id) = app_seeded_with(lang, TestConfig::default(), true, |_| {}, |session| seed_strength(session, sets));
    let mut harness = harness_sized(app, TALL);
    harness.state_mut().ui.section = Section::Activities;
    harness.state_mut().ui.activities.selected = Some(id.0);
    harness.run();
    (harness, id)
}

/// The activity as the engine has it now.
fn read(harness: &mut Harness<'static, GymgarApp>, id: ActivityId) -> ActivityView {
    match harness.state_mut().run("activity.get", json!({ "id": id.0 })) {
        Ok(Outcome::Done(value)) => serde_json::from_value(value).expect("an activity"),
        other => panic!("activity.get: {other:?}"),
    }
}

fn find(view: &ActivityView, set: SetRef) -> SetView {
    view.groups.iter().flat_map(|group| &group.sets).find(|found| found.set == set).cloned().unwrap_or_else(|| panic!("{set:?} is not shown"))
}

/// The exercise keys of the cards, in order, and the sets of each.
fn cards(view: &ActivityView) -> Vec<(Option<String>, Vec<SetRef>)> {
    view.groups.iter().map(|group| (group.exercise.as_ref().map(|exercise| exercise.id.clone()), group.sets.iter().map(|set| set.set).collect())).collect()
}

/// Clicks the line of set `number` of the `card`-th card (from 0), which opens its editor.
fn edit(harness: &mut Harness<'static, GymgarApp>, lang: Lang, card: usize, number: usize) {
    let label = pick(lang, "Edit set {}", "Изменить подход {}").replace("{}", &number.to_string());
    harness.get_all_by_role_and_label(Role::Button, &label).nth(card).unwrap_or_else(|| panic!("no {label} in card {card}")).click();
    harness.run();
}

fn press(harness: &mut Harness<'static, GymgarApp>, label: &str) {
    harness.get_by_role_and_label(Role::Button, label).click();
    harness.run();
}

/// Replaces what the field `label` holds with `text`, as typing over it would.
fn fill(harness: &mut Harness<'static, GymgarApp>, label: &str, text: &str) {
    harness.get_by_role_and_label(Role::TextInput, label).set_text_value_accesskit(text);
    harness.run();
}

fn field(harness: &Harness<'static, GymgarApp>, label: &str) -> String {
    harness.get_by_role_and_label(Role::TextInput, label).accesskit_node().value().unwrap_or_default()
}

fn weight_field(lang: Lang) -> &'static str {
    pick(lang, "Weight", "Вес")
}

fn reps_field(lang: Lang) -> &'static str {
    pick(lang, "Reps", "Повторы")
}

fn save(lang: Lang) -> &'static str {
    pick(lang, "Save", "Сохранить")
}

fn search_field(lang: Lang) -> &'static str {
    pick(lang, "Search exercises", "Поиск упражнения")
}

fn deadlift_name(lang: Lang) -> &'static str {
    pick(lang, "Romanian deadlift", "Румынская становая тяга")
}

fn whole_card(lang: Lang) -> &'static str {
    pick(lang, "For all sets of the card", "Для всех подходов карточки")
}

fn ticked(harness: &Harness<'static, GymgarApp>, label: &str) -> String {
    format!("{:?}", harness.get_by_role_and_label(Role::CheckBox, label).accesskit_node().toggled())
}

fn editor_open(harness: &Harness<'static, GymgarApp>, lang: Lang) -> bool {
    harness.query_by_role_and_label(Role::TextInput, weight_field(lang)).is_some()
}

fn picker_open(harness: &Harness<'static, GymgarApp>, lang: Lang) -> bool {
    harness.query_by_role_and_label(Role::TextInput, search_field(lang)).is_some()
}

#[test]
fn inline_edit_saves_only_changed_fields() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, deadlifts());
        // The second set: 7 × 80. Its fields hold what it has now.
        edit(&mut harness, lang, 0, 2);
        assert_eq!(field(&harness, weight_field(lang)), "80", "{lang:?}");
        assert_eq!(field(&harness, reps_field(lang)), "7", "{lang:?}");
        // The mock-up's buttons, a recorded set's way back among them.
        for (en, ru) in [
            ("Save", "Сохранить"),
            ("Cancel", "Отмена"),
            ("Reset to watch values", "Вернуть как на часах"),
            ("Don't count this set", "Не учитывать подход"),
            ("Other exercise…", "Другое упражнение…"),
        ] {
            harness.get_by_role_and_label(Role::Button, pick(lang, en, ru));
        }
        assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Remove", "Убрать")).is_none(), "{lang:?}: only an added set is removed");

        fill(&mut harness, reps_field(lang), "8");
        press(&mut harness, save(lang));
        assert!(!editor_open(&harness, lang), "{lang:?}: saving closes the editor");
        let set = find(&read(&mut harness, id), SetRef::Recorded(1));
        let correction = set.correction.expect("a correction");
        assert_eq!((correction.reps, correction.weight_kg), (Some(8), None), "{lang:?}: the weight was not touched");
        harness.get_by_label(pick(lang, "was 7", "было 7"));

        // Saving what did not change changes nothing; "Cancel" drops what was typed.
        edit(&mut harness, lang, 0, 1);
        press(&mut harness, save(lang));
        assert!(!editor_open(&harness, lang), "{lang:?}");
        edit(&mut harness, lang, 0, 3);
        fill(&mut harness, weight_field(lang), "60");
        press(&mut harness, pick(lang, "Cancel", "Отмена"));
        assert!(!editor_open(&harness, lang), "{lang:?}");
        let view = read(&mut harness, id);
        for index in [0, 2] {
            assert_eq!(find(&view, SetRef::Recorded(index)).correction, None, "{lang:?}: set {index}");
        }
        assert_eq!(harness.state().error(), None, "{lang:?}");
    }
}

#[test]
fn a_weight_shown_rounded_is_not_sent_when_its_text_is_untouched() {
    // A watch writes a set's weight in sixteenths of a kilogram: 22.6875 kg is stored as 22 688 g and shown as 22.69.
    // Saving the reps must not "correct" the weight to what the field shows.
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, vec![set(0, Some(10), Some(22.6875), Some(DEADLIFT))]);
        edit(&mut harness, lang, 0, 1);
        let shown = pick(lang, "22.69", "22,69");
        assert_eq!(field(&harness, weight_field(lang)), shown, "{lang:?}");
        fill(&mut harness, reps_field(lang), "9");
        press(&mut harness, save(lang));
        let correction = find(&read(&mut harness, id), SetRef::Recorded(0)).correction.expect("a correction");
        assert_eq!((correction.reps, correction.weight_kg), (Some(9), None), "{lang:?}: only the reps were changed");

        // Typing over the weight what the field already showed is no change either.
        edit(&mut harness, lang, 0, 1);
        fill(&mut harness, weight_field(lang), shown);
        press(&mut harness, save(lang));
        assert!(!editor_open(&harness, lang), "{lang:?}");
        let set = find(&read(&mut harness, id), SetRef::Recorded(0));
        assert_eq!(set.correction.and_then(|correction| correction.weight_kg), None, "{lang:?}");
        assert_eq!(set.effective.weight_kg, Some(22.688), "{lang:?}: the recorded weight, to the gram");

        // A weight really typed is sent.
        edit(&mut harness, lang, 0, 1);
        fill(&mut harness, weight_field(lang), pick(lang, "22.7", "22,7"));
        press(&mut harness, save(lang));
        let correction = find(&read(&mut harness, id), SetRef::Recorded(0)).correction.expect("a correction");
        assert_eq!((correction.reps, correction.weight_kg), (Some(9), Some(22.7)), "{lang:?}");
    }
}

#[test]
fn comma_and_dot_both_parse() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, deadlifts());
        for (number, text) in [(2, "77,5"), (3, "82.5")] {
            edit(&mut harness, lang, 0, number);
            fill(&mut harness, weight_field(lang), text);
            press(&mut harness, save(lang));
            assert!(!editor_open(&harness, lang), "{lang:?} {text}");
        }
        let view = read(&mut harness, id);
        assert_eq!(find(&view, SetRef::Recorded(1)).effective.weight_kg, Some(77.5), "{lang:?}");
        assert_eq!(find(&view, SetRef::Recorded(2)).effective.weight_kg, Some(82.5), "{lang:?}");
        // The field shows a weight the way the language writes it.
        edit(&mut harness, lang, 0, 2);
        assert_eq!(field(&harness, weight_field(lang)), pick(lang, "77.5", "77,5"));
    }
}

#[test]
fn bad_input_shows_error_and_sends_nothing() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, deadlifts());
        let weight_error = pick(lang, "Enter a weight from 0 to 1000 kg.", "Введите вес от 0 до 1000 кг.");
        let reps_error = pick(lang, "Enter a whole number of reps from 0 to 1000.", "Введите целое число повторов от 0 до 1000.");
        let cases = [
            (weight_field(lang), "abc", weight_error),
            (weight_field(lang), "-5", weight_error),
            (weight_field(lang), "1000,5", weight_error),
            (weight_field(lang), "1,2,3", weight_error),
            (weight_field(lang), "1e3", weight_error),
            (weight_field(lang), "NaN", weight_error),
            (weight_field(lang), "", weight_error),
            (reps_field(lang), "1001", reps_error),
            (reps_field(lang), "7.5", reps_error),
            (reps_field(lang), "-1", reps_error),
            (reps_field(lang), "99999999999999999999", reps_error),
        ];
        for (name, text, error) in cases {
            edit(&mut harness, lang, 0, 2);
            fill(&mut harness, name, text);
            press(&mut harness, save(lang));
            // The editor stays, with the reason under the line, and the engine was not asked.
            assert!(editor_open(&harness, lang), "{lang:?} {text:?}");
            harness.get_by_label(error);
            assert_eq!(find(&read(&mut harness, id), SetRef::Recorded(1)).correction, None, "{lang:?} {text:?}");
            assert_eq!(harness.state().error(), None, "{lang:?} {text:?}: nothing for the banner");
            press(&mut harness, pick(lang, "Cancel", "Отмена"));
            assert!(harness.query_by_label(error).is_none(), "{lang:?}: the reason goes with the editor");
        }
        // A weight of zero and zero reps are values, not mistakes.
        edit(&mut harness, lang, 0, 2);
        fill(&mut harness, weight_field(lang), " 0 ");
        fill(&mut harness, reps_field(lang), "0");
        press(&mut harness, save(lang));
        let correction = find(&read(&mut harness, id), SetRef::Recorded(1)).correction.expect("a correction");
        assert_eq!((correction.weight_kg, correction.reps), (Some(0.0), Some(0)), "{lang:?}");
    }
}

#[test]
fn reset_returns_watch_values() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, deadlifts());
        let corrected = json!({ "id": id.0, "sets": [{ "recorded": 1 }], "weightKg": 77.5, "reps": 8, "counted": false });
        harness.state_mut().run("activity.editSets", corrected).expect("correct a set");
        harness.run();
        edit(&mut harness, lang, 0, 2);
        press(&mut harness, pick(lang, "Reset to watch values", "Вернуть как на часах"));
        assert!(!editor_open(&harness, lang), "{lang:?}");
        let set = find(&read(&mut harness, id), SetRef::Recorded(1));
        assert_eq!(set.correction, None, "{lang:?}");
        assert_eq!((set.effective.weight_kg, set.effective.reps, set.counted), (Some(80.0), Some(7), true), "{lang:?}");
    }
}

#[test]
fn count_toggle_from_the_editor() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, deadlifts());
        edit(&mut harness, lang, 0, 1);
        press(&mut harness, pick(lang, "Don't count this set", "Не учитывать подход"));
        assert!(!find(&read(&mut harness, id), SetRef::Recorded(0)).counted, "{lang:?}");
        harness.get_by_label(pick(lang, "not counted", "не учитывается"));
        // The line of a set that does not count offers to count it again.
        edit(&mut harness, lang, 0, 1);
        press(&mut harness, pick(lang, "Count", "Учитывать"));
        assert!(find(&read(&mut harness, id), SetRef::Recorded(0)).counted, "{lang:?}");
    }
}

#[test]
fn assign_exercise_to_whole_group() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, unnamed());
        harness.get_by_label(pick(lang, "No exercise", "Без упражнения"));
        press(&mut harness, pick(lang, "Choose exercise", "Указать упражнение"));
        assert!(picker_open(&harness, lang), "{lang:?}");
        assert_eq!(ticked(&harness, whole_card(lang)), "Some(True)", "{lang:?}: the whole card by default");
        // "Choose" waits for a choice.
        assert!(harness.get_by_role_and_label(Role::Button, pick(lang, "Choose", "Выбрать")).accesskit_node().is_disabled(), "{lang:?}");

        fill(&mut harness, search_field(lang), "romanian");
        // What the user has done first, with how many times, then Garmin's catalog.
        let done = harness.get_by_label(pick(lang, "Done before", "Вы делали")).rect().top();
        let catalog = harness.get_by_label(pick(lang, "Garmin catalog", "Каталог Garmin")).rect().top();
        let option = harness.get_by_role_and_label(Role::Button, deadlift_name(lang)).rect().top();
        assert!(done < option && option < catalog, "{lang:?}: {done} {option} {catalog}");
        let times = harness.get_by_role_and_label(Role::Button, deadlift_name(lang)).accesskit_node().description().unwrap_or_default();
        assert!(times.ends_with(pick(lang, " times", " раз")) || times.ends_with(pick(lang, " time", " раза")), "{lang:?}: how many times: {times:?}");

        harness.get_by_role_and_label(Role::Button, deadlift_name(lang)).click();
        harness.run();
        press(&mut harness, pick(lang, "Choose", "Выбрать"));
        assert!(!picker_open(&harness, lang), "{lang:?}: choosing closes the picker");
        assert!(harness.query_by_label(pick(lang, "No exercise", "Без упражнения")).is_none(), "{lang:?}: the warning card is gone");
        harness.get_by_label(deadlift_name(lang));
        let view = read(&mut harness, id);
        let all = vec![SetRef::Recorded(0), SetRef::Recorded(1), SetRef::Recorded(2)];
        assert_eq!(cards(&view), vec![(Some(DEADLIFT_KEY.to_owned()), all)], "{lang:?}");
    }
}

#[test]
fn assign_to_one_set_only() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, unnamed());
        // "Other exercise…" from the second set's line: only that set by default.
        edit(&mut harness, lang, 0, 2);
        press(&mut harness, pick(lang, "Other exercise…", "Другое упражнение…"));
        assert!(picker_open(&harness, lang), "{lang:?}");
        assert_eq!(ticked(&harness, whole_card(lang)), "Some(False)", "{lang:?}");
        fill(&mut harness, search_field(lang), "romanian");
        harness.get_by_role_and_label(Role::Button, deadlift_name(lang)).click();
        harness.run();
        press(&mut harness, pick(lang, "Choose", "Выбрать"));
        let view = read(&mut harness, id);
        assert_eq!(
            cards(&view),
            vec![(None, vec![SetRef::Recorded(0), SetRef::Recorded(2)]), (Some(DEADLIFT_KEY.to_owned()), vec![SetRef::Recorded(1)])],
            "{lang:?}"
        );
        harness.get_by_label(pick(lang, "No exercise", "Без упражнения"));

        // "Choose exercise" with the box unticked: the card's first set only.
        press(&mut harness, pick(lang, "Choose exercise", "Указать упражнение"));
        harness.get_by_role_and_label(Role::CheckBox, whole_card(lang)).click();
        harness.run();
        assert_eq!(ticked(&harness, whole_card(lang)), "Some(False)", "{lang:?}");
        fill(&mut harness, search_field(lang), "romanian");
        harness.get_by_role_and_label(Role::Button, deadlift_name(lang)).click();
        harness.run();
        press(&mut harness, pick(lang, "Choose", "Выбрать"));
        let view = read(&mut harness, id);
        assert_eq!(
            cards(&view),
            vec![(Some(DEADLIFT_KEY.to_owned()), vec![SetRef::Recorded(0), SetRef::Recorded(1)]), (None, vec![SetRef::Recorded(2)])],
            "{lang:?}"
        );
    }
}

#[test]
fn add_and_remove_set() {
    for lang in Lang::ALL {
        let (mut harness, id) = opened(lang, deadlifts());
        // The search opens on the card's exercise, already chosen.
        press(&mut harness, pick(lang, "+ Add set", "+ Добавить подход"));
        assert!(picker_open(&harness, lang), "{lang:?}");
        assert_eq!(field(&harness, search_field(lang)), deadlift_name(lang), "{lang:?}");
        let option = format!("{:?}", harness.get_by_role_and_label(Role::Button, deadlift_name(lang)).accesskit_node().toggled());
        assert_eq!(option, "Some(True)", "{lang:?}: the card's exercise is chosen");
        assert!(harness.query_by_role_and_label(Role::CheckBox, whole_card(lang)).is_none(), "{lang:?}: a new set is one set");
        press(&mut harness, pick(lang, "Choose", "Выбрать"));

        // The set follows the last one of the card, and its line opens for the weight and the reps.
        let view = read(&mut harness, id);
        let sets = &cards(&view)[0].1;
        assert_eq!(sets.len(), 4, "{lang:?}");
        let Some(&added @ SetRef::Added(_)) = sets.last() else { panic!("{lang:?}: {sets:?}") };
        assert!(editor_open(&harness, lang), "{lang:?}");
        assert_eq!((field(&harness, weight_field(lang)), field(&harness, reps_field(lang))), (String::new(), String::new()));
        fill(&mut harness, weight_field(lang), "82,5");
        fill(&mut harness, reps_field(lang), "6");
        press(&mut harness, save(lang));
        let set = find(&read(&mut harness, id), added);
        assert_eq!((set.effective.weight_kg, set.effective.reps, set.counted), (Some(82.5), Some(6), true), "{lang:?}");
        harness.get_by_label(pick(lang, "added", "добавлен"));

        // An added set is removed, not reset.
        edit(&mut harness, lang, 0, 4);
        assert!(harness.query_by_role_and_label(Role::Button, pick(lang, "Reset to watch values", "Вернуть как на часах")).is_none(), "{lang:?}");
        press(&mut harness, pick(lang, "Remove", "Убрать"));
        assert!(!editor_open(&harness, lang), "{lang:?}");
        let view = read(&mut harness, id);
        assert_eq!(cards(&view), vec![(Some(DEADLIFT_KEY.to_owned()), vec![SetRef::Recorded(0), SetRef::Recorded(1), SetRef::Recorded(2)])], "{lang:?}");
        assert!(harness.query_by_label(pick(lang, "added", "добавлен")).is_none(), "{lang:?}");
    }
}

#[test]
fn search_runs_when_the_query_changes_not_every_frame() {
    let lang = Lang::En;
    let (mut harness, _id) = opened(lang, unnamed());
    press(&mut harness, "Choose exercise");
    // Opening searches once: the exercises the user has done.
    let hits = |harness: &Harness<'static, GymgarApp>| harness.state().ui.activities.picker.as_ref().map(|picker| picker.hits.len());
    assert!(hits(&harness).is_some_and(|count| count > 0));
    // Frames that change nothing ask nothing: hits put aside stay aside.
    if let Some(picker) = harness.state_mut().ui.activities.picker.as_mut() {
        picker.hits.clear();
    }
    for _ in 0..5 {
        harness.step();
    }
    harness.run();
    assert_eq!(hits(&harness), Some(0));
    fill(&mut harness, search_field(lang), "romanian");
    assert!(hits(&harness).is_some_and(|count| count > 0));
    // Nothing found says so.
    fill(&mut harness, search_field(lang), "no such exercise anywhere");
    harness.get_by_label("Nothing found");
    // "Cancel" closes the picker and changes nothing.
    press(&mut harness, "Cancel");
    assert!(!picker_open(&harness, lang));
}

#[test]
fn a_reason_in_one_language_goes_when_the_language_changes() {
    // Like the connect form's: a sentence in the language that is no longer the window's is stale.
    let (mut harness, _id) = opened(Lang::En, deadlifts());
    edit(&mut harness, Lang::En, 0, 2);
    fill(&mut harness, weight_field(Lang::En), "heavy");
    press(&mut harness, save(Lang::En));
    harness.get_by_label("Enter a weight from 0 to 1000 kg.");
    harness.state_mut().run("settings.setLanguage", json!({ "language": "ru" })).expect("switch to Russian");
    harness.run();
    assert!(editor_open(&harness, Lang::Ru), "the editor stays, with what was typed");
    assert_eq!(field(&harness, weight_field(Lang::Ru)), "heavy");
    assert!(harness.query_by_label("Enter a weight from 0 to 1000 kg.").is_none());
}

#[test]
fn another_activity_drops_the_editor_and_the_picker() {
    // Sets are named within their activity: an editor or a picker left open must not act on the next one.
    let lang = Lang::En;
    let (mut harness, _id) = opened(lang, deadlifts());
    edit(&mut harness, lang, 0, 2);
    press(&mut harness, "Other exercise…");
    assert!(editor_open(&harness, lang) && picker_open(&harness, lang));
    let other = harness.state().activities().iter().find(|item| item.name.as_deref() == Some("strength_training 1")).map(|item| item.id.0);
    harness.state_mut().ui.activities.selected = other;
    harness.run();
    assert!(!editor_open(&harness, lang) && !picker_open(&harness, lang));
    assert_eq!((&harness.state().ui.activities.editing, &harness.state().ui.activities.picker), (&None, &None));
}

#[test]
fn the_editor_and_the_picker_fit_the_smallest_window() {
    // The screen works in the minimal window, 800 × 500 (spec §7, «Интерфейс»): nothing runs past its right edge.
    for lang in Lang::ALL {
        let (app, _handles, id) = app_seeded_with(lang, TestConfig::default(), true, |_| {}, |session| seed_strength(session, deadlifts()));
        let mut harness = harness_sized(app, egui::vec2(800.0, 500.0));
        harness.state_mut().ui.section = Section::Activities;
        harness.state_mut().ui.activities.selected = Some(id.0);
        harness.run();
        edit(&mut harness, lang, 0, 2);
        press(&mut harness, pick(lang, "Other exercise…", "Другое упражнение…"));
        fill(&mut harness, search_field(lang), "deadlift");
        for node in harness.root().children_recursive() {
            let role = node.accesskit_node().role();
            if matches!(role, Role::Label | Role::Link | Role::Button | Role::Heading | Role::TextInput | Role::CheckBox) {
                let rect = node.rect();
                let name = node.accesskit_node().value().or(node.accesskit_node().label());
                assert!(rect.right() <= 800.5, "{lang:?}: {role:?} {name:?} runs to {}", rect.right());
            }
        }
        // The picker's buttons are within the window's height.
        let choose = harness.get_by_role_and_label(Role::Button, pick(lang, "Choose", "Выбрать")).rect();
        assert!(choose.bottom() <= 500.5, "{lang:?}: \"Choose\" ends at {}", choose.bottom());
    }
}
