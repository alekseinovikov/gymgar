//! Two-way sync between the local store and Garmin Connect.
//!
//! L2: owns the domain ⇄ Garmin mapping, change tracking and conflict resolution. Network work runs
//! off the UI thread; results come back as plain data for the engine to apply.
