use std::collections::HashMap;

use gymgar_domain::{KeyTerm, SetRef};
use gymgar_engine::{ExerciseHitView, ExerciseView, JobId};

use super::*;

const KEY: &str = "gk_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SQUAT: &str = "squat/barbell_back_squat";

fn filled_in() -> UiState {
    UiState {
        section: Section::Settings,
        connect: ConnectForm {
            key: KEY.to_owned(),
            term: KeyTerm::Days90,
            allow_file_store: true,
            error: Some("Garmin did not accept the key.".to_owned()),
            pending: Some(JobId(7)),
            disconnecting: Some(JobId(8)),
            replacing: true,
            confirm_disconnect: true,
        },
        activities: ActivitiesState { selected: Some(1_000_001), filter: ListFilter::New, ..ActivitiesState::default() },
        progress: ProgressState { filter: ProgressFilter::Add, open: Some(SQUAT.to_owned()) },
    }
}

#[test]
fn a_new_state_opens_the_summary_with_a_one_year_term() {
    let state = UiState::default();
    assert_eq!(state.section, Section::Summary);
    assert_eq!(state.connect.term, KeyTerm::Days365);
    assert!(state.connect.key.is_empty());
    assert_eq!((state.connect.allow_file_store, state.connect.replacing, state.connect.confirm_disconnect), (false, false, false));
    assert_eq!((state.connect.error, state.connect.pending, state.connect.disconnecting), (None, None, None));
    // The progress tiles start on the whole list with no exercise open.
    assert_eq!(state.progress, ProgressState { filter: ProgressFilter::All, open: None });
}

/// What a restart brings back of `filled_in()`: the section, the term, the activity and filter of the list, and the filter and
/// open exercise of Progress. Everything else is transient.
fn after_a_restart() -> UiState {
    UiState {
        section: Section::Settings,
        connect: ConnectForm { term: KeyTerm::Days90, ..ConnectForm::default() },
        activities: ActivitiesState { selected: Some(1_000_001), filter: ListFilter::New, ..ActivitiesState::default() },
        progress: ProgressState { filter: ProgressFilter::Add, open: Some(SQUAT.to_owned()) },
    }
}

#[test]
fn only_the_section_the_term_and_the_list_are_serialized() {
    let json = serde_json::to_string(&filled_in()).expect("serialize");
    assert!(!json.contains(KEY), "{json}");
    assert!(!json.contains("did not accept"), "{json}");
    // The key, the error and the pending jobs are state of the moment; the consent to keep the key in a file and the
    // two confirmations must be given again each time, so they do not come back armed after a restart either.
    for field in ["key", "error", "pending", "disconnecting", "allowFileStore", "replacing", "confirmDisconnect"] {
        assert!(!json.contains(&format!("\"{field}\"")), "`{field}` is serialized: {json}");
    }

    let restored: UiState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored, after_a_restart());
}

#[test]
fn transient_fields_in_stored_json_are_ignored_not_restored() {
    // Data saved by an earlier build, which kept the flags, must not bring a consent or a confirmation back armed.
    let json = format!(
        r#"{{"section":"settings","connect":{{"key":"{KEY}","error":"x","pending":3,"disconnecting":4,"allowFileStore":true,"replacing":true,"confirmDisconnect":true}}}}"#
    );
    let state: UiState = serde_json::from_str(&json).expect("stored transient fields are tolerated");
    assert_eq!(state, UiState { section: Section::Settings, ..UiState::default() });
}

#[test]
fn the_json_uses_the_names_of_the_commands() {
    let json = serde_json::to_value(filled_in()).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "section": "settings",
            "connect": { "term": 90 },
            "activities": { "selected": 1_000_001_i64, "filter": "new" },
            "progress": { "filter": "add", "open": "squat/barbell_back_squat" },
        })
    );
    let json = serde_json::to_value(UiState { section: Section::Activities, ..UiState::default() }).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({ "section": "activities", "connect": { "term": 365 }, "activities": { "filter": "all" }, "progress": { "filter": "all" } })
    );
    let json = serde_json::to_value(UiState { section: Section::Progress, ..UiState::default() }).expect("serialize");
    assert_eq!(json["section"], "progress");
    for (filter, name) in [(ProgressFilter::All, "all"), (ProgressFilter::Add, "add"), (ProgressFilter::Break, "break")] {
        assert_eq!(serde_json::to_value(filter).expect("serialize"), name);
    }
}

#[test]
fn the_progress_section_and_its_state_survive_a_restart() {
    let state =
        UiState { section: Section::Progress, progress: ProgressState { filter: ProgressFilter::Break, open: Some(SQUAT.to_owned()) }, ..UiState::default() };
    let json = serde_json::to_string(&state).expect("serialize");
    assert_eq!(serde_json::from_str::<UiState>(&json).expect("deserialize"), state);

    let mut storage = MemoryStorage::default();
    state.save(&mut storage);
    assert_eq!(UiState::load(Some(&storage)), state);

    // Nothing open is not written, and comes back as nothing open.
    let closed = UiState { section: Section::Progress, progress: ProgressState { filter: ProgressFilter::Add, open: None }, ..UiState::default() };
    let json = serde_json::to_value(&closed).expect("serialize");
    assert_eq!(json["progress"], serde_json::json!({ "filter": "add" }));
    assert_eq!(serde_json::from_value::<UiState>(json).expect("deserialize"), closed);
}

#[test]
fn missing_fields_fall_back_to_the_defaults() {
    assert_eq!(serde_json::from_str::<UiState>("{}").expect("empty"), UiState::default());
    let partial: UiState = serde_json::from_str(r#"{"connect":{"term":30}}"#).expect("partial");
    assert_eq!((partial.section, partial.connect.term), (Section::Summary, KeyTerm::Days30));
    // A state saved before the activity list existed opens on the whole list with nothing selected.
    assert_eq!(partial.activities, ActivitiesState { selected: None, filter: ListFilter::All, ..ActivitiesState::default() });
    let partial: UiState = serde_json::from_str(r#"{"activities":{"filter":"strength"}}"#).expect("partial");
    assert_eq!(partial.activities, ActivitiesState { selected: None, filter: ListFilter::Strength, ..ActivitiesState::default() });
    // A state saved before Progress existed opens it on the whole list with nothing open.
    assert_eq!(partial.progress, ProgressState::default());
    let old: UiState = serde_json::from_str(r#"{"section":"activities","connect":{"term":90},"activities":{"selected":5,"filter":"new"}}"#).expect("old");
    assert_eq!((old.section, old.progress), (Section::Activities, ProgressState { filter: ProgressFilter::All, open: None }));
    let partial: UiState = serde_json::from_str(r#"{"progress":{"open":"squat/barbell_back_squat"}}"#).expect("partial");
    assert_eq!(partial.progress, ProgressState { filter: ProgressFilter::All, open: Some("squat/barbell_back_squat".to_owned()) });
    // A term the app does not offer is an error, not a silent fallback.
    assert!(serde_json::from_str::<UiState>(r#"{"connect":{"term":31}}"#).is_err());
}

#[test]
fn debug_output_hides_the_key() {
    let state = filled_in();
    for text in [format!("{state:?}"), format!("{:?}", state.connect), format!("{state:#?}")] {
        assert!(!text.contains(KEY), "{text}");
        assert!(!text.contains("0123456789abcdef"), "{text}");
    }
    // The rest of the form is still there for debugging.
    assert!(format!("{state:?}").contains("Days90"));
    // An empty draft is told apart from a filled one without revealing it.
    assert_ne!(format!("{:?}", ConnectForm::default()), format!("{:?}", filled_in().connect));
}

#[test]
fn edit_state_not_persisted() {
    // An edit left half-way and an open picker are state of the moment: neither is written, so neither comes back.
    let deadlift = ExerciseView { id: "deadlift/romanian_deadlift".to_owned(), english: "Romanian deadlift".to_owned(), watch_title: None };
    let mut state = filled_in();
    state.activities.editing = Some(SetEdit {
        set: SetRef::Recorded(3),
        weight: "77,5 typed".to_owned(),
        reps: "7".to_owned(),
        weight_shown: "80 shown".to_owned(),
        reps_shown: "7".to_owned(),
        error: Some("bad".to_owned()),
        focus: true,
    });
    state.activities.picker = Some(Picker {
        target: PickerTarget::Sets { card: vec![SetRef::Recorded(3), SetRef::Added(2)], one: SetRef::Added(2) },
        query: "curl typed".to_owned(),
        whole_group: true,
        hits: vec![ExerciseHitView { exercise: deadlift.clone(), times_used: 14 }],
        chosen: Some(deadlift),
        error: None,
        focus: false,
    });
    let json = serde_json::to_string(&state).expect("serialize");
    for text in ["editing", "picker", "77,5 typed", "80 shown", "curl typed", "romanian_deadlift", "\"recorded\"", "\"added\""] {
        assert!(!json.contains(text), "{text} is serialized: {json}");
    }
    let restored: UiState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!((&restored.activities.editing, &restored.activities.picker), (&None, &None));
    assert_eq!(restored, after_a_restart());

    let mut storage = MemoryStorage::default();
    state.save(&mut storage);
    let restored = UiState::load(Some(&storage));
    assert_eq!((restored.activities.editing, restored.activities.picker), (None, None));
}

#[derive(Default)]
struct MemoryStorage(HashMap<String, String>);

impl eframe::Storage for MemoryStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

#[test]
fn the_state_survives_a_restart_without_the_draft_key() {
    let mut storage = MemoryStorage::default();
    filled_in().save(&mut storage);
    assert!(storage.0.values().all(|stored| !stored.contains(KEY)), "the key reached the storage: {:?}", storage.0);

    let restored = UiState::load(Some(&storage));
    assert_eq!((restored.section, restored.connect.term), (Section::Settings, KeyTerm::Days90));
    assert!(restored.connect.key.is_empty());
    // The consent and the confirmations were given for that moment only.
    let form = &restored.connect;
    assert_eq!((form.allow_file_store, form.replacing, form.confirm_disconnect), (false, false, false));
    assert_eq!(restored, after_a_restart());
}

#[test]
fn loading_without_or_with_unreadable_storage_gives_the_defaults() {
    assert_eq!(UiState::load(None), UiState::default());
    let mut storage = MemoryStorage::default();
    assert_eq!(UiState::load(Some(&storage)), UiState::default());
    storage.0.insert(UiState::STORAGE_KEY.to_owned(), "(section: Nonsense".to_owned());
    assert_eq!(UiState::load(Some(&storage)), UiState::default());
}
