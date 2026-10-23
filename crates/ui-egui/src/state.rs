//! The purely visual state of the window: which section is open and the draft of the connect form.
//!
//! Plain data with `serde`, so eframe can keep it between runs and automation can read it. The key the user is typing
//! is a draft that is never serialized and never printed. Everything else the screens show (the connection, the
//! language) belongs to the engine and is read from it, not kept here.

use std::fmt;

use gymgar_domain::KeyTerm;
use gymgar_engine::JobId;
use serde::{Deserialize, Serialize};

/// The window's visual state. Missing fields in stored data fall back to their defaults, so a state saved by another
/// version still loads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    pub section: Section,
    pub connect: ConnectForm,
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
    Settings,
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
    /// The user agreed to keep the key in a file because the system password store is unavailable.
    pub allow_file_store: bool,
    /// Why the last attempt failed, already in the user's language.
    #[serde(skip)]
    pub error: Option<String>,
    /// The key check this form waits for.
    #[serde(skip)]
    pub pending: Option<JobId>,
    /// The form is open to replace a key that is already connected.
    pub replacing: bool,
    /// "Disconnect" was clicked and awaits confirmation.
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
            .field("replacing", &self.replacing)
            .field("confirm_disconnect", &self.confirm_disconnect)
            .finish()
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
