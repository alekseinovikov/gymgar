//! The activity file the Personal API sends: a ZIP with one `.FIT` inside.
//!
//! [`unzip_activity`] takes the `.FIT` out within size limits; [`parse`] decodes the messages a strength workout needs (sets,
//! exercise titles, workout steps, the workout's name, heart rate) into DTOs in FIT's own vocabulary. Turning them into domain types
//! is `gymgar-sync`'s job, and nothing here touches the network. Both inputs are hostile: a broken, truncated, oversized or foreign
//! file is a [`FitError`], never a panic.
//!
//! `catalog` names the exercises a watch records (`deadlift/romanian_deadlift`).
//!
//! `testing` (feature `testing`, and this crate's own tests) writes FIT files and one-file ZIPs for tests.

pub mod catalog;
mod parse;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod unzip;

pub use parse::{FitActivity, FitSet, FitSetKind, FitStep, FitTitle, parse};
pub use unzip::unzip_activity;

/// Why an activity file could not be read. The text never quotes the file's content (names and notes are the user's data).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FitError {
    /// Not a ZIP, a damaged one, or not exactly one `.fit` inside.
    #[error("the activity archive cannot be read: {0}")]
    Zip(String),
    /// The `.FIT` is truncated, damaged or not a FIT file.
    #[error("the FIT file cannot be read: {0}")]
    Fit(String),
    /// The archive or the file inside it is larger than an activity file can reasonably be, or the file holds more sets,
    /// heart-rate readings or workout steps than one can.
    #[error("the activity file is larger than allowed")]
    TooLarge,
}
