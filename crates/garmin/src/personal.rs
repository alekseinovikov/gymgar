//! The official Garmin Personal API: the user's API key is exchanged for a short-lived access token.
//!
//! Read-only. [`Auth`] turns the key into tokens; [`Client`] fetches data with them and, after a 401, exchanges the key once and
//! tries once more. Beyond that there is no retry or backoff policy: callers decide what to do with a [`GarminError`].

mod activities;
mod auth;
mod client;
mod http;
pub mod jwt;

pub use activities::{ActivityListItem, ActivityTypeDto, parse_activity_page};
pub use auth::{Auth, TokenInfo};
pub use client::Client;

/// Base URL of the Personal API.
pub const DEFAULT_BASE_URL: &str = "https://personal.connectapi.garmin.com";
