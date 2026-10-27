//! Keeps the local store and Garmin Connect in step.
//!
//! L2: owns the Garmin ⇄ domain mapping. Pure functions from Garmin's bytes to domain types, no network and no storage:
//!
//! - `list`: a page of the activity list into [`gymgar_domain::ActivitySummary`] plus the element's own JSON, which the engine stores.
//! - `strength`: a downloaded activity file (a ZIP with one `.FIT`) into a [`gymgar_domain::StrengthRecord`]: the active sets with
//!   their plan, rest and heart rate. A weight or a rep count the domain refuses becomes "absent" with a log line, never an error.
//! - `exercises`: the stable text keys of exercises and Garmin's catalog, in domain terms.
//! - `policy`: the import's pacing and stopping rules (page size, pauses, when to stop paging, what is "new").
//!
//! Activities are downloaded incrementally and idempotently. Workouts will sync both ways against the base version from the last
//! sync; when both sides changed, that is a conflict for the user to resolve, never an automatic overwrite. Network work runs off
//! the UI thread; results come back as plain data for the engine to apply.

mod error;
pub mod exercises;
pub mod list;
pub mod policy;
pub mod strength;

pub use error::SyncError;
