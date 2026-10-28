//! Gymgar domain model: exercises, workouts, sets, training plans and units.
//!
//! L0: pure data plus validation. No I/O, no network, no UI, no workspace dependencies.
//! Units (mass, duration, distance) are typed values, never bare numbers; time goes through `chrono`.

mod activity;
mod error;
mod exercise;
mod garmin;
mod language;
pub mod progress;
mod sets;
mod units;

pub use activity::{ActivityId, ActivitySummary, FileState, STRENGTH_TYPE_KEY, SyncErrorKind};
pub use error::DomainError;
pub use exercise::ExerciseId;
pub use garmin::{ConnectionStatus, GarminConnection, KeyTerm, SecretBackend, expiry_warning, key_expires_at};
pub use language::Language;
pub use sets::{AddedSet, CountReason, EMPTY_SET_MAX_MS, PlannedTarget, RecordedSet, SetCorrection, SetRef, StrengthRecord, count_reason, is_empty_set};
pub use units::{Mass, Reps};
