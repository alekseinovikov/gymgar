use std::collections::HashMap;

use gymgar_domain::KeyTerm;
use gymgar_engine::JobId;

use super::*;

const KEY: &str = "gk_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn filled_in() -> UiState {
    UiState {
        section: Section::Settings,
        connect: ConnectForm {
            key: KEY.to_owned(),
            term: KeyTerm::Days90,
            allow_file_store: true,
            error: Some("Garmin did not accept the key.".to_owned()),
            pending: Some(JobId(7)),
            replacing: true,
            confirm_disconnect: true,
        },
    }
}

#[test]
fn a_new_state_opens_the_summary_with_a_one_year_term() {
    let state = UiState::default();
    assert_eq!(state.section, Section::Summary);
    assert_eq!(state.connect.term, KeyTerm::Days365);
    assert!(state.connect.key.is_empty());
    assert_eq!((state.connect.allow_file_store, state.connect.replacing, state.connect.confirm_disconnect), (false, false, false));
    assert_eq!((state.connect.error, state.connect.pending), (None, None));
}

/// What a restart brings back of `filled_in()`: the section and the term. Everything else is transient.
fn after_a_restart() -> UiState {
    UiState { section: Section::Settings, connect: ConnectForm { term: KeyTerm::Days90, ..ConnectForm::default() } }
}

#[test]
fn only_the_section_and_the_term_are_serialized() {
    let json = serde_json::to_string(&filled_in()).expect("serialize");
    assert!(!json.contains(KEY), "{json}");
    assert!(!json.contains("did not accept"), "{json}");
    // The key, the error and the pending job are state of the moment; the consent to keep the key in a file and the
    // two confirmations must be given again each time, so they do not come back armed after a restart either.
    for field in ["key", "error", "pending", "allowFileStore", "replacing", "confirmDisconnect"] {
        assert!(!json.contains(&format!("\"{field}\"")), "`{field}` is serialized: {json}");
    }

    let restored: UiState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored, after_a_restart());
}

#[test]
fn transient_fields_in_stored_json_are_ignored_not_restored() {
    // Data saved by an earlier build, which kept the flags, must not bring a consent or a confirmation back armed.
    let json = format!(
        r#"{{"section":"settings","connect":{{"key":"{KEY}","error":"x","pending":3,"allowFileStore":true,"replacing":true,"confirmDisconnect":true}}}}"#
    );
    let state: UiState = serde_json::from_str(&json).expect("stored transient fields are tolerated");
    assert_eq!(state, UiState { section: Section::Settings, ..UiState::default() });
}

#[test]
fn the_json_uses_the_names_of_the_commands() {
    let json = serde_json::to_value(filled_in()).expect("serialize");
    assert_eq!(json, serde_json::json!({ "section": "settings", "connect": { "term": 90 } }));
}

#[test]
fn missing_fields_fall_back_to_the_defaults() {
    assert_eq!(serde_json::from_str::<UiState>("{}").expect("empty"), UiState::default());
    let partial: UiState = serde_json::from_str(r#"{"connect":{"term":30}}"#).expect("partial");
    assert_eq!((partial.section, partial.connect.term), (Section::Summary, KeyTerm::Days30));
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
