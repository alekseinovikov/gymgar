//! The command registry: every user action as a [`CommandSpec`] with a stable `area.action` id.
//!
//! Commands live in `<area>_cmds.rs` modules, each exporting `specs()`; adding an area is one `v.extend(..)` line in
//! `REGISTRY`. The registry only describes commands: dispatch and its checks are [`Session::execute`].

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EngineError, Session, activity_cmds, exercise_cmds, garmin_cmds, settings_cmds, sync_cmds};

/// Who runs a command. Agents (MCP) may run only commands marked `agent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Caller {
    User,
    Agent,
}

/// What a command does, which decides how frontends treat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandKind {
    /// Reads only; safe to repeat.
    Query,
    /// Changes data in one step, synchronously.
    Mutation,
    /// Starts background work and answers at once with a [`JobId`].
    Job,
}

/// Identifies a background job started by a [`CommandKind::Job`] command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JobId(pub u64);

/// What a successful command returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The command finished; its result as JSON (a serialized result type, never hand-built JSON).
    Done(Value),
    /// The command started a background job; its result arrives later.
    Started(JobId),
}

/// The description of one command.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    /// Stable id, `area.action` in camelCase.
    pub id: &'static str,
    /// English description, shown to agents over MCP.
    pub description: &'static str,
    pub kind: CommandKind,
    /// Whether agents may run it.
    pub agent: bool,
    /// Whether it destroys data the user cannot get back; the UI confirms it, MCP marks it.
    pub destructive: bool,
    /// JSON Schema of the params; always an object schema.
    pub params_schema: fn() -> schemars::Schema,
    /// `Err(reason)` when the command cannot run in the current state.
    pub enabled: fn(&Session) -> Result<(), String>,
    /// Parses the params and runs the command.
    pub run: fn(&mut Session, Value) -> Result<Outcome, EngineError>,
}

static REGISTRY: LazyLock<Vec<CommandSpec>> = LazyLock::new(|| {
    let mut v = Vec::new();
    v.extend(settings_cmds::specs());
    v.extend(garmin_cmds::specs());
    v.extend(sync_cmds::specs());
    v.extend(activity_cmds::specs());
    v.extend(exercise_cmds::specs());
    v
});

/// Every command, in registration order.
pub fn all() -> Vec<&'static CommandSpec> {
    REGISTRY.iter().collect()
}

/// The command with this id.
pub fn find(id: &str) -> Option<&'static CommandSpec> {
    REGISTRY.iter().find(|spec| spec.id == id)
}

/// `enabled` of a command that can always run.
pub(crate) fn always(_: &Session) -> Result<(), String> {
    Ok(())
}

/// The result of command `cmd` as [`Outcome::Done`].
pub(crate) fn done<T: Serialize>(cmd: &str, result: &T) -> Result<Outcome, EngineError> {
    to_json(cmd, result).map(Outcome::Done)
}

/// The result of command `cmd` as JSON: the answer of a query or mutation, or of a finished job.
pub(crate) fn to_json<T: Serialize>(cmd: &str, result: &T) -> Result<Value, EngineError> {
    // Result types are plain structs (strings, numbers, dates), which serde_json always encodes. Should a future type
    // fail (a map with non-string keys), the command reports it instead of panicking. The engine has no "internal"
    // error variant, and the result is built from the database, so it is reported as a store error.
    serde_json::to_value(result).map_err(|error| EngineError::Store(format!("cannot encode the result of {cmd} as JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    /// The command table of the design spec (§7 "Commands"): id, kind, agent, destructive.
    const SPEC_TABLE: [(&str, CommandKind, bool, bool); 17] = [
        ("garmin.status", CommandKind::Query, true, false),
        ("garmin.connect", CommandKind::Job, false, false),
        ("garmin.verify", CommandKind::Job, false, false),
        ("garmin.disconnect", CommandKind::Job, false, true),
        ("garmin.sync", CommandKind::Job, false, false),
        ("activity.reparse", CommandKind::Job, false, false),
        ("activity.list", CommandKind::Query, true, false),
        ("activity.get", CommandKind::Query, true, false),
        ("activity.markSeen", CommandKind::Mutation, false, false),
        ("activity.setExcluded", CommandKind::Mutation, true, false),
        ("activity.editSets", CommandKind::Mutation, true, false),
        ("activity.resetSets", CommandKind::Mutation, true, false),
        ("activity.addSet", CommandKind::Mutation, true, false),
        ("activity.removeAddedSet", CommandKind::Mutation, true, false),
        ("exercise.search", CommandKind::Query, true, false),
        ("settings.get", CommandKind::Query, true, false),
        ("settings.setLanguage", CommandKind::Mutation, true, false),
    ];

    #[test]
    fn registry_flags_match_spec() {
        for spec in all() {
            let Some(&(_, kind, agent, destructive)) = SPEC_TABLE.iter().find(|(id, ..)| *id == spec.id) else {
                panic!("{} is not in the spec's command table", spec.id);
            };
            assert_eq!(spec.kind, kind, "{}", spec.id);
            assert_eq!(spec.agent, agent, "{}", spec.id);
            assert_eq!(spec.destructive, destructive, "{}", spec.id);
            assert!(!spec.description.is_empty(), "{}", spec.id);
            let schema = (spec.params_schema)();
            assert_eq!(schema.get("type"), Some(&json!("object")), "{}: {schema:?}", spec.id);
        }
        // Agents may never enter, check or remove the key, nor make the app go to Garmin, nor take the "new" mark off (it is for
        // the user's eyes); only disconnect destroys anything.
        for id in ["garmin.connect", "garmin.verify", "garmin.disconnect", "garmin.sync", "activity.reparse", "activity.markSeen"] {
            assert!(find(id).is_none_or(|spec| !spec.agent), "{id}");
        }
        assert_eq!(all().iter().filter(|spec| spec.destructive).map(|spec| spec.id).collect::<Vec<_>>(), ["garmin.disconnect"]);
        // Every command of the table is registered.
        for (id, ..) in SPEC_TABLE {
            assert!(find(id).is_some(), "{id} is not registered");
        }
    }

    #[test]
    fn ids_are_unique_area_action() {
        let mut seen = HashSet::new();
        for spec in all() {
            assert!(seen.insert(spec.id), "{} is registered twice", spec.id);
            let (area, action) = spec.id.split_once('.').unwrap_or_else(|| panic!("{} is not area.action", spec.id));
            for part in [area, action] {
                assert!(part.starts_with(|c: char| c.is_ascii_lowercase()), "{}", spec.id);
                assert!(part.chars().all(|c| c.is_ascii_alphanumeric()), "{}", spec.id);
            }
        }
        assert!(find("settings").is_none());
    }

    #[test]
    fn kinds_serialize_in_camel_case() {
        assert_eq!(serde_json::to_value([CommandKind::Query, CommandKind::Mutation, CommandKind::Job]).unwrap(), json!(["query", "mutation", "job"]));
    }
}
