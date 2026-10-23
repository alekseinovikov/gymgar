//! The Gymgar engine façade: the open session, the command registry and background jobs.
//!
//! L3. Every user-visible action is a command with a stable id (`workout.create`) and typed,
//! JSON-serializable params. Every frontend goes through the same entry point: the egui UI, the
//! CLI and the MCP server. This is what keeps the UI thin and the app fully drivable by agents.
//! Nothing here may depend on a UI toolkit.
//!
//! The outside world (secret stores, Garmin, the async runtime, the clock) comes in as [`Services`], so the
//! engine depends on neither `gymgar-secrets` nor `gymgar-garmin` nor tokio, and [`testing`] can replace it all.

pub mod commands;
mod error;
mod garmin_cmds;
pub mod params;
mod services;
mod session;
mod settings_cmds;
// Also built for this crate's own unit tests, so `cargo test` runs them without `--features testing`.
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use commands::{Caller, CommandKind, CommandSpec, JobId, Outcome};
pub use error::EngineError;
pub use garmin_cmds::GarminStatusView;
pub use services::{BoxFuture, GarminAuth, GarminAuthError, SecretStore, SecretStoreError, SecretStores, Services, Spawner};
pub use session::Session;
pub use settings_cmds::SettingsView;
