//! Garmin Connect client: authentication, workouts, activities and their exercise sets.
//!
//! Standalone: depends on no workspace crate. It speaks Garmin's own data model (DTOs that mirror
//! Garmin's JSON, unknown fields kept verbatim so a round-trip loses nothing). Mapping to and from
//! `gymgar-domain` lives in `gymgar-sync`, not here.
