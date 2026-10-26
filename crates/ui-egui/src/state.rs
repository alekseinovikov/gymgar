//! The purely visual state of the window: which section is open, the draft of the connect form, the activity list's
//! filter and selection, and the set being corrected with the exercise picker.
//!
//! Plain data with `serde`, so eframe can keep it between runs and automation can read it. Only the section, the key
//! term and the list's filter and selected activity are kept: the key the user is typing (never serialized, never
//! printed) and everything that is state of the moment (an error, a running check or disconnect, a consent, a
//! confirmation, a set being edited, an open picker) starts over with each run. What the screens show of the outside world (the connection, the language,
//! the activities) belongs to the engine and is read from it, not kept here.

use std::fmt;

use gymgar_domain::{KeyTerm, SetRef};
use gymgar_engine::{ExerciseHitView, ExerciseView, JobId};
use serde::{Deserialize, Serialize};

/// The window's visual state. Missing fields in stored data fall back to their defaults, so a state saved by another
/// version still loads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    pub section: Section,
    pub connect: ConnectForm,
    pub activities: ActivitiesState,
}

impl UiState {
    /// The key the state is kept under in eframe's storage.
    pub const STORAGE_KEY: &'static str = "gymgar-ui-state";

    /// The state saved by the last run; the default when there is none or it cannot be read.
    pub fn load(storage: Option<&dyn eframe::Storage>) -> UiState {
        storage.and_then(|storage| eframe::get_value(storage, Self::STORAGE_KEY)).unwrap_or_default()
    }

    /// Saves the state (the key draft excluded) for the next run.
    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, Self::STORAGE_KEY, self);
    }
}

/// A section the icon rail leads to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Section {
    #[default]
    Summary,
    Activities,
    Settings,
}

/// What the Activities screen shows: which activities the list holds and which one is open beside it, and what of it the
/// user is correcting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ActivitiesState {
    /// Garmin's id of the activity open beside the list. One that is no longer listed selects nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<i64>,
    pub filter: ListFilter,
    /// The set of the open activity whose line is an editor. Transient: an edit left half-way is not brought back.
    #[serde(skip)]
    pub editing: Option<SetEdit>,
    /// The exercise picker over the open activity. Transient.
    #[serde(skip)]
    pub picker: Option<Picker>,
}

/// The line of a set turned into an editor (activity import spec §2, item 9): the weight and the reps as the user types
/// them, checked only when they are saved.
#[derive(Debug, Clone, PartialEq)]
pub struct SetEdit {
    pub set: SetRef,
    /// The weight in kilograms, with a comma or a dot.
    pub weight: String,
    pub reps: String,
    /// What the fields showed when the editor opened. A field whose text is still this is not sent: the weight is shown
    /// rounded (to 10 g), and reading it back would "correct" a weight the user never touched.
    pub weight_shown: String,
    pub reps_shown: String,
    /// Why the last "Save" (or another action of the line) did not go through, in the user's language.
    pub error: Option<String>,
    /// The weight field takes the keyboard when it is next drawn: the editor has just opened.
    pub focus: bool,
}

/// The search for an exercise (spec §2, items 9–11): for the sets of a card, for one set, or for a set to add.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub target: PickerTarget,
    /// What the user searches for; the hits are the engine's answer for it.
    pub query: String,
    /// The exercise goes to every set of the card, not only to the one the picker was opened for.
    pub whole_group: bool,
    /// What `exercise.search` answered for `query`: the exercises the user has done first, then Garmin's catalog.
    pub hits: Vec<ExerciseHitView>,
    /// The exercise chosen in the list, which "Choose" gives to the sets. Always one of `hits`.
    pub chosen: Option<ExerciseView>,
    /// Why the search or the choice did not go through, in the user's language.
    pub error: Option<String>,
    /// The search field takes the keyboard when it is next drawn: the picker has just opened.
    pub focus: bool,
}

/// What the chosen exercise is for.
#[derive(Debug, Clone, PartialEq)]
pub enum PickerTarget {
    /// Sets of one card: all of `card` when the picker's `whole_group` is ticked, else only `one`, the set it was opened
    /// for.
    Sets { card: Vec<SetRef>, one: SetRef },
    /// A set the watch did not record, added after `after` (the last set of the card it was asked from).
    NewSet { after: Option<SetRef> },
}

/// The chips above the activity list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ListFilter {
    #[default]
    All,
    /// Only strength activities, the ones with sets.
    Strength,
    /// Only the activities the user has not opened yet.
    New,
}

/// The "connect Garmin" form of Settings: what the user has entered and what is going on with it.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConnectForm {
    /// The pasted key. Of any length, never serialized, never printed.
    #[serde(skip)]
    pub key: String,
    /// The lifetime the user chose on Garmin's page.
    pub term: KeyTerm,
    /// The user agreed to keep the key in a file because the system password store is unavailable. Transient: the
    /// consent is given again each time, so it never comes back ticked after a restart.
    #[serde(skip)]
    pub allow_file_store: bool,
    /// Why the last attempt failed, already in the user's language.
    #[serde(skip)]
    pub error: Option<String>,
    /// The key check this form waits for.
    #[serde(skip)]
    pub pending: Option<JobId>,
    /// The disconnect the card waits for, after the user confirmed it.
    #[serde(skip)]
    pub disconnecting: Option<JobId>,
    /// The form is open to replace a key that is already connected. Transient.
    #[serde(skip)]
    pub replacing: bool,
    /// "Disconnect" was clicked and awaits confirmation. Transient: a destructive confirmation must not come back
    /// armed after a restart.
    #[serde(skip)]
    pub confirm_disconnect: bool,
}

// By hand: a derived `Debug` would print the key.
impl fmt::Debug for ConnectForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectForm")
            .field("key", &if self.key.is_empty() { "<empty>" } else { "<hidden>" })
            .field("term", &self.term)
            .field("allow_file_store", &self.allow_file_store)
            .field("error", &self.error)
            .field("pending", &self.pending)
            .field("disconnecting", &self.disconnecting)
            .field("replacing", &self.replacing)
            .field("confirm_disconnect", &self.confirm_disconnect)
            .finish()
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
