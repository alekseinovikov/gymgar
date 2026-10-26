//! Garmin Connect client over two channels:
//!
//! - `personal`: the official Personal API (the user's API key → access token). Read-only:
//!   activities and health data.
//! - `connect`: the undocumented Garmin Connect web API, used only for what `personal` cannot do:
//!   create, edit and delete workouts, and read a workout's current version before changing it.
//!   No background polling. It can break whenever Garmin changes it, so it has its own auth and
//!   errors, and a failure here never affects `personal`.
//! - `fit`: the activity file `personal` downloads (a ZIP with one `.FIT`): unpacking within size limits and decoding what a
//!   strength workout needs (sets, exercise titles, workout steps, the workout's name, heart rate). No network.
//!
//! Standalone: depends on no workspace crate. It speaks Garmin's own data model (DTOs that mirror
//! Garmin's JSON, unknown fields kept verbatim). Mapping to `gymgar-domain` lives in `gymgar-sync`.
//! Keys, tokens and passwords never appear in logs or error messages.

mod error;
pub mod fit;
pub mod personal;

pub use error::GarminError;
