//! The official Garmin Personal API: the user's API key is exchanged for a short-lived access token.
//!
//! Read-only. This module has no retry or backoff policy: callers decide what to do with a [`GarminError`].

mod auth;
pub mod jwt;

pub use auth::{Auth, TokenInfo};

/// Base URL of the Personal API.
pub const DEFAULT_BASE_URL: &str = "https://personal.connectapi.garmin.com";
