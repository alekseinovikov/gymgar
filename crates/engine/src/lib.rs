//! The Gymgar engine façade: the open session, history and the command registry.
//!
//! L3. Every user-visible action is a command with a stable id (`workout.create`) and typed,
//! JSON-serializable params. Every frontend goes through the same entry point: the egui UI, the
//! CLI and the MCP server. This is what keeps the UI thin and the app fully drivable by agents.
//! Nothing here may depend on a UI toolkit.
