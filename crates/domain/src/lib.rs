//! Gymgar domain model: exercises, workouts, sets, training plans and units.
//!
//! L0: pure data plus validation. No I/O, no network, no UI, no workspace dependencies.
//! Units (mass, duration, distance) are typed values, never bare numbers; time goes through `chrono`.
