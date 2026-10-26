//! `exercise.search`: finds exercises by name, for the user who picks one for a set and for agents that need the keys the
//! other commands take.
//!
//! What the user has done comes first (the most used first), then the rest of Garmin's catalog in its own order. The
//! search looks at the English name and at the title the watch gave the exercise, ignoring case. It only reads.

use std::collections::HashSet;

use gymgar_domain::ExerciseId;
use gymgar_sync::exercises::{catalog, english_name, exercise_key};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::activity_views::{ExerciseHitView, ExerciseSearchView, exercise_view};
use crate::commands::{always, done};
use crate::params::{self, bad_params};
use crate::{CommandKind, CommandSpec, EngineError, Outcome, Session};

const SEARCH: &str = "exercise.search";

/// The longest query, in characters (not bytes).
const MAX_QUERY_CHARS: usize = 100;
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SearchParams {
    /// Part of a name, in English or as the watch titles it (any case); at most 100 characters. Empty lists only the exercises the user has done.
    query: String,
    /// How many to return: 20 by default, 1 to 100.
    #[schemars(range(min = 1, max = 100))]
    limit: Option<u32>,
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: SEARCH,
        description: "Search exercises by name (English, or the title the user's watch gave it; any case). The exercises the user has done come \
                      first, most used first, each with timesUsed (counted sets); then the rest of Garmin's catalog. An empty query lists only the \
                      exercises the user has done. The id of each hit is the key the other commands take.",
        kind: CommandKind::Query,
        agent: true,
        destructive: false,
        params_schema: params::schema::<SearchParams>,
        enabled: always,
        run: search,
    }]
}

fn search(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let SearchParams { query, limit } = params::parse(SEARCH, params)?;
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(bad_params(SEARCH, format!("query has at most {MAX_QUERY_CHARS} characters")));
    }
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(bad_params(SEARCH, format!("limit must be 1 to {MAX_LIMIT}, got {limit}")));
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let needle = query.trim().to_lowercase();
    let titles = session.store().exercise_titles()?;
    let usage = session.store().exercise_usage()?;

    let matches =
        |id: ExerciseId, english: &str| english.to_lowercase().contains(&needle) || titles.get(&id).is_some_and(|title| title.to_lowercase().contains(&needle));
    let hit = |id: ExerciseId, times_used: u32| ExerciseHitView { exercise: exercise_view(id, &titles), times_used };

    // The ones the user did, most used first; the key breaks a tie, so the order does not depend on the database's.
    let mut used: Vec<(String, ExerciseId, u32)> = usage.iter().map(|(&id, &times)| (exercise_key(id), id, times)).collect();
    used.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let mut items: Vec<ExerciseHitView> =
        used.into_iter().filter(|&(_, id, _)| needle.is_empty() || matches(id, &english_name(id))).take(limit).map(|(_, id, times)| hit(id, times)).collect();

    // Then the rest of the catalog; with no query, only what the user did.
    if !needle.is_empty() {
        let did: HashSet<ExerciseId> = usage.keys().copied().collect();
        for entry in catalog() {
            if items.len() >= limit {
                break;
            }
            if entry.id.category != ExerciseId::UNKNOWN_CATEGORY && !did.contains(&entry.id) && matches(entry.id, &entry.english) {
                items.push(hit(entry.id, 0));
            }
        }
    }
    done(SEARCH, &ExerciseSearchView { items })
}

#[cfg(test)]
#[path = "exercise_cmds_tests.rs"]
mod tests;
