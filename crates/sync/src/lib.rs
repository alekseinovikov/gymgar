//! Keeps the local store and Garmin Connect in step.
//!
//! L2: owns the Garmin ⇄ domain mapping. Activities are downloaded incrementally and idempotently.
//! Workouts sync both ways against the base version from the last sync; when both sides changed,
//! that is a conflict for the user to resolve, never an automatic overwrite. Network work runs off
//! the UI thread; results come back as plain data for the engine to apply.
