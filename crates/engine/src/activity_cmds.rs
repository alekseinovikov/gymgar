//! The commands on imported activities: `activity.list` and `activity.get` read them, `activity.markSeen` takes the "new"
//! mark off, `activity.setExcluded` leaves one out of the statistics, and `activity.editSets`, `activity.resetSets`,
//! `activity.addSet` and `activity.removeAddedSet` correct its sets on top of what the watch recorded.
//!
//! They read and write the database only (never Garmin), so they work in every session, `gymgar-cli` included. Each
//! mutation is one store transaction, and what they answer is the activity as it is afterwards (`ActivityView`), so a caller
//! never needs a second call. The params come from the UI and from agents: the bounds are checked here, before the
//! store sees them, and every mistake (a value out of range, an activity or set that does not exist) is
//! [`EngineError::BadParams`]. `activity.get` never marks an activity seen: the mark is for the user's eyes.

use chrono::{DateTime, Datelike, Utc};
use gymgar_domain::{ActivityId, ExerciseId, Mass, Reps, SetRef};
use gymgar_store::{ActivityDetail, ActivityFilter, SetChange};
use gymgar_sync::exercises::parse_exercise_key;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::activity_views::{self, ActivityListView, ActivityView};
use crate::commands::{always, done};
use crate::params::{self, bad_params};
use crate::{CommandKind, CommandSpec, EngineError, Outcome, Session};

const LIST: &str = "activity.list";
const GET: &str = "activity.get";
const MARK_SEEN: &str = "activity.markSeen";
const SET_EXCLUDED: &str = "activity.setExcluded";
const EDIT_SETS: &str = "activity.editSets";
const RESET_SETS: &str = "activity.resetSets";
const ADD_SET: &str = "activity.addSet";
const REMOVE_ADDED_SET: &str = "activity.removeAddedSet";

/// How many activities `activity.list` gives when `limit` is not set, and the most it gives.
const LIST_DEFAULT_LIMIT: u32 = 100;
const LIST_MAX_LIMIT: u32 = 1000;
/// At most this many type keys in a filter, each at most this many characters.
const MAX_TYPES: usize = 50;
const MAX_TYPE_CHARS: usize = 64;
/// An exercise key is a few words (`deadlift/romanian_deadlift`); a longer text is not one.
const MAX_KEY_CHARS: usize = 100;
/// At most this many sets in one `editSets` or `resetSets`.
const MAX_SETS: usize = 200;

/// A set of an activity, as params name it. Its own type, not the domain's: it is the schema agents are given and carries its own bounds.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
enum SetRefParam {
    /// A set the watch recorded, by its message index in the file (as `activity.get` shows it).
    Recorded(u16),
    /// A set the user added, by its id (positive).
    Added(i64),
}

impl From<SetRefParam> for SetRef {
    fn from(set: SetRefParam) -> SetRef {
        match set {
            SetRefParam::Recorded(index) => SetRef::Recorded(index),
            SetRefParam::Added(id) => SetRef::Added(id),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ListParams {
    /// Only activities that started at or after this moment (RFC 3339, for example `2026-10-01T00:00:00Z`).
    from: Option<String>,
    /// Only activities that started before this moment (RFC 3339).
    to: Option<String>,
    /// Garmin's type keys, for example `strength_training`, `running`, `cycling`; missing or empty means any type. At most 50, each at most 64 characters.
    #[schemars(length(max = 50))]
    types: Option<Vec<String>>,
    /// Only strength activities (the only ones with sets).
    strength_only: Option<bool>,
    /// Only activities with a counted set of this exercise: a key from `exercise.search`, for example `deadlift/romanian_deadlift`.
    exercise: Option<String>,
    /// Only activities the user has not opened yet.
    only_new: Option<bool>,
    /// Whether to list the activities the user left out of the statistics (they are marked `excluded`). The default is true.
    include_excluded: Option<bool>,
    /// How many to return: 100 by default, 1 to 1000.
    #[schemars(range(min = 1, max = 1000))]
    limit: Option<u32>,
    /// How many of the newest to skip.
    offset: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct IdParams {
    /// Garmin's activity id, as `activity.list` shows it.
    id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SetExcludedParams {
    /// Garmin's activity id.
    id: i64,
    /// True leaves the activity out of the statistics (it stays in the list, marked); false takes it back.
    excluded: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EditSetsParams {
    /// Garmin's activity id (a strength activity).
    id: i64,
    /// 1 to 200 sets, as `activity.get` names them: `{"recorded": n}` or `{"added": id}`. All of them get the same change.
    #[schemars(length(min = 1, max = 200))]
    sets: Vec<SetRefParam>,
    /// The weight in kilograms, 0 to 1000.
    weight_kg: Option<f64>,
    /// The repetitions, 0 to 1000.
    reps: Option<u32>,
    /// The exercise, a key from `exercise.search` such as `curl/incline_dumbbell_biceps_curl`.
    exercise: Option<String>,
    /// True counts the set whatever the empty-set rule says, false leaves it out, null hands it back to the rule. Not given: the flag stays as it is.
    #[serde(default, deserialize_with = "given")]
    #[schemars(with = "Option<bool>")]
    counted: Option<Option<bool>>,
}

/// A field that is `null` is given (as "nothing"), unlike a field that is absent: the plain `Option<Option<_>>` cannot tell them apart.
fn given<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<bool>>, D::Error> {
    Option::<bool>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ResetSetsParams {
    /// Garmin's activity id (a strength activity).
    id: i64,
    /// 1 to 200 sets the watch recorded: `{"recorded": n}`. A set that was added by the user is removed with `activity.removeAddedSet`.
    #[schemars(length(min = 1, max = 200))]
    sets: Vec<SetRefParam>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AddSetParams {
    /// Garmin's activity id (a strength activity).
    id: i64,
    /// The exercise, a key from `exercise.search`.
    exercise: String,
    /// The weight in kilograms, 0 to 1000.
    weight_kg: Option<f64>,
    /// The repetitions, 0 to 1000.
    reps: Option<u32>,
    /// The set it comes after, as `activity.get` names it. Not given: it goes to the end of the activity.
    after: Option<SetRefParam>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RemoveAddedSetParams {
    /// Garmin's activity id (a strength activity).
    id: i64,
    /// The set to remove: `{"added": id}`. A set the watch recorded cannot be removed, only put back with `activity.resetSets`.
    set: SetRefParam,
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: LIST,
            description: "List the imported Garmin activities, newest first. Filters: from and to (RFC 3339), types (Garmin type keys), strengthOnly, \
                          exercise (a key from exercise.search: activities with a counted set of it), onlyNew, includeExcluded (default true; \
                          excluded activities are marked). limit defaults to 100 (at most 1000); page with offset. Each item has the summary, \
                          the number of counted sets and exercises, and the state of the file.",
            kind: CommandKind::Query,
            agent: true,
            destructive: false,
            params_schema: params::schema::<ListParams>,
            enabled: always,
            run: list,
        },
        CommandSpec {
            id: GET,
            description: "Read one activity in full: the summary and the sets grouped by exercise in order of first appearance. For each set: its key, \
                          what the watch recorded, the user's correction, the final values, whether it counts and why, the plan of the workout \
                          program, heart rate and rest. Does not mark the activity as seen.",
            kind: CommandKind::Query,
            agent: true,
            destructive: false,
            params_schema: params::schema::<IdParams>,
            enabled: always,
            run: get,
        },
        CommandSpec {
            id: MARK_SEEN,
            description: "Take the \"new\" mark off an activity (the user opened it). Answers with the activity's list item.",
            kind: CommandKind::Mutation,
            agent: false,
            destructive: false,
            params_schema: params::schema::<IdParams>,
            enabled: always,
            run: mark_seen,
        },
        CommandSpec {
            id: SET_EXCLUDED,
            description: "Leave a whole activity out of the statistics (excluded: true) or take it back in (false). It stays in the list. Answers with the activity.",
            kind: CommandKind::Mutation,
            agent: true,
            destructive: false,
            params_schema: params::schema::<SetExcludedParams>,
            enabled: always,
            run: set_excluded,
        },
        CommandSpec {
            id: EDIT_SETS,
            description: "Correct sets of a strength activity on top of the watch's record, which stays: for each of the given sets (recorded or added) \
                          set weightKg, reps, exercise (a key from exercise.search) and counted (true, false, or null to leave it to the empty-set rule). \
                          A field not given does not change. All or nothing. Answers with the activity.",
            kind: CommandKind::Mutation,
            agent: true,
            destructive: false,
            params_schema: params::schema::<EditSetsParams>,
            enabled: always,
            run: edit_sets,
        },
        CommandSpec {
            id: RESET_SETS,
            description: "Put recorded sets back as the watch recorded them: drops every correction of the given sets. Answers with the activity.",
            kind: CommandKind::Mutation,
            agent: true,
            destructive: false,
            params_schema: params::schema::<ResetSetsParams>,
            enabled: always,
            run: reset_sets,
        },
        CommandSpec {
            id: ADD_SET,
            description: "Add a set the watch did not record to a strength activity: exercise, optional weightKg and reps, and the set it comes after \
                          (not given: the end). An added set counts. Answers with the activity.",
            kind: CommandKind::Mutation,
            agent: true,
            destructive: false,
            params_schema: params::schema::<AddSetParams>,
            enabled: always,
            run: add_set,
        },
        CommandSpec {
            id: REMOVE_ADDED_SET,
            description: "Remove a set that was added with activity.addSet (it is only hidden, never lost). Sets the watch recorded are not removed \
                          this way. Answers with the activity.",
            kind: CommandKind::Mutation,
            agent: true,
            destructive: false,
            params_schema: params::schema::<RemoveAddedSetParams>,
            enabled: always,
            run: remove_added_set,
        },
    ]
}

// ----- checking params

/// Garmin's activity ids are positive.
fn activity_id(cmd: &str, id: i64) -> Result<ActivityId, EngineError> {
    if id > 0 { Ok(ActivityId(id)) } else { Err(bad_params(cmd, format!("id must be a positive activity id, got {id}"))) }
}

/// An RFC 3339 moment in UTC, in years 0 to 9999 (what the database can hold; an offset can carry a year out of them).
fn instant(cmd: &str, field: &str, text: &str) -> Result<DateTime<Utc>, EngineError> {
    let parsed = DateTime::parse_from_rfc3339(text)
        .map_err(|error| bad_params(cmd, format!("{field} is not an RFC 3339 date and time (like 2026-10-01T00:00:00Z): {error}")))?;
    let utc = parsed.with_timezone(&Utc);
    if !(0..=9999).contains(&utc.year()) {
        return Err(bad_params(cmd, format!("{field} is outside the years 0 to 9999")));
    }
    Ok(utc)
}

/// The exercise a key names. Garmin's "unknown" category is no exercise and is not accepted.
fn exercise_id(cmd: &str, key: &str) -> Result<ExerciseId, EngineError> {
    if key.chars().count() > MAX_KEY_CHARS {
        return Err(bad_params(cmd, format!("an exercise key has at most {MAX_KEY_CHARS} characters")));
    }
    match parse_exercise_key(key) {
        Some(exercise) if exercise.category != ExerciseId::UNKNOWN_CATEGORY => Ok(exercise),
        _ => Err(bad_params(cmd, format!("{key:?} is not an exercise key; exercise.search finds the keys"))),
    }
}

fn mass(cmd: &str, kg: f64) -> Result<Mass, EngineError> {
    Mass::from_kg(kg).map_err(|error| bad_params(cmd, format!("weightKg: {error}")))
}

fn reps(cmd: &str, count: u32) -> Result<Reps, EngineError> {
    Reps::new(count).map_err(|error| bad_params(cmd, format!("reps: {error}")))
}

/// A set ref of the params: an added set has a positive id.
fn set_ref(cmd: &str, set: SetRefParam) -> Result<SetRef, EngineError> {
    match set {
        SetRefParam::Added(id) if id <= 0 => Err(bad_params(cmd, format!("an added set has a positive id, got {id}"))),
        set => Ok(set.into()),
    }
}

/// 1 to 200 set refs.
fn set_refs(cmd: &str, sets: &[SetRefParam]) -> Result<Vec<SetRef>, EngineError> {
    if sets.is_empty() || sets.len() > MAX_SETS {
        return Err(bad_params(cmd, format!("sets must have 1 to {MAX_SETS} entries, got {}", sets.len())));
    }
    sets.iter().map(|&set| set_ref(cmd, set)).collect()
}

fn filter_of(params: ListParams) -> Result<ActivityFilter, EngineError> {
    let from = params.from.as_deref().map(|text| instant(LIST, "from", text)).transpose()?;
    let to = params.to.as_deref().map(|text| instant(LIST, "to", text)).transpose()?;
    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return Err(bad_params(LIST, "from is after to".to_owned()));
    }
    let type_keys = params.types.unwrap_or_default();
    if type_keys.len() > MAX_TYPES {
        return Err(bad_params(LIST, format!("types has at most {MAX_TYPES} entries, got {}", type_keys.len())));
    }
    if type_keys.iter().any(|key| key.chars().count() > MAX_TYPE_CHARS) {
        return Err(bad_params(LIST, format!("a type key has at most {MAX_TYPE_CHARS} characters")));
    }
    let exercise = params.exercise.as_deref().map(|key| exercise_id(LIST, key)).transpose()?;
    let limit = params.limit.unwrap_or(LIST_DEFAULT_LIMIT);
    if !(1..=LIST_MAX_LIMIT).contains(&limit) {
        return Err(bad_params(LIST, format!("limit must be 1 to {LIST_MAX_LIMIT}, got {limit}")));
    }
    Ok(ActivityFilter {
        from,
        to,
        type_keys,
        strength_only: params.strength_only.unwrap_or(false),
        exercise,
        only_new: params.only_new.unwrap_or(false),
        include_excluded: params.include_excluded.unwrap_or(true),
        limit,
        offset: params.offset.unwrap_or(0),
    })
}

// ----- reading what a command answers with

/// The activity in full, or `BadParams` when there is none.
fn load(session: &Session, cmd: &str, id: ActivityId) -> Result<ActivityDetail, EngineError> {
    let detail = session.store().activity_detail(id).map_err(|error| EngineError::from_store(cmd, error))?;
    detail.ok_or_else(|| bad_params(cmd, format!("not found: activity {}", id.0)))
}

/// The view of activity `id` as it is now.
fn view(session: &Session, cmd: &str, id: ActivityId) -> Result<ActivityView, EngineError> {
    let detail = load(session, cmd, id)?;
    let titles = session.store().exercise_titles()?;
    Ok(activity_views::build(detail, &titles))
}

/// Only a strength activity has sets, so only it can be corrected; the store would let a run be given sets.
fn require_strength(session: &Session, cmd: &str, id: ActivityId) -> Result<(), EngineError> {
    if load(session, cmd, id)?.row.summary.is_strength() {
        Ok(())
    } else {
        Err(bad_params(cmd, format!("activity {} is not a strength activity: only those have sets", id.0)))
    }
}

// ----- the commands

fn list(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let filter = filter_of(params::parse(LIST, params)?)?;
    let rows = session.store().list_activities(&filter).map_err(|error| EngineError::from_store(LIST, error))?;
    done(LIST, &ActivityListView { items: rows.iter().map(activity_views::list_item).collect() })
}

fn get(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let IdParams { id } = params::parse(GET, params)?;
    done(GET, &view(session, GET, activity_id(GET, id)?)?)
}

fn mark_seen(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let IdParams { id } = params::parse(MARK_SEEN, params)?;
    let id = activity_id(MARK_SEEN, id)?;
    session.store().mark_seen(id, session.now()).map_err(|error| EngineError::from_store(MARK_SEEN, error))?;
    done(MARK_SEEN, &activity_views::list_item(&load(session, MARK_SEEN, id)?.row))
}

fn set_excluded(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let SetExcludedParams { id, excluded } = params::parse(SET_EXCLUDED, params)?;
    let id = activity_id(SET_EXCLUDED, id)?;
    session.store().set_activity_excluded(id, excluded).map_err(|error| EngineError::from_store(SET_EXCLUDED, error))?;
    log::info!("activity {} {}", id.0, if excluded { "left out of the statistics" } else { "taken back into the statistics" });
    done(SET_EXCLUDED, &view(session, SET_EXCLUDED, id)?)
}

fn edit_sets(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let EditSetsParams { id, sets, weight_kg, reps: reps_count, exercise, counted } = params::parse(EDIT_SETS, params)?;
    let id = activity_id(EDIT_SETS, id)?;
    let sets = set_refs(EDIT_SETS, &sets)?;
    let change = SetChange {
        weight: weight_kg.map(|kg| mass(EDIT_SETS, kg)).transpose()?,
        reps: reps_count.map(|count| reps(EDIT_SETS, count)).transpose()?,
        exercise: exercise.as_deref().map(|key| exercise_id(EDIT_SETS, key)).transpose()?,
        counted,
    };
    if change == SetChange::default() {
        return Err(bad_params(EDIT_SETS, "nothing to change: give at least one of weightKg, reps, exercise and counted".to_owned()));
    }
    require_strength(session, EDIT_SETS, id)?;
    session.store().edit_sets(id, &sets, &change, session.now()).map_err(|error| EngineError::from_store(EDIT_SETS, error))?;
    done(EDIT_SETS, &view(session, EDIT_SETS, id)?)
}

fn reset_sets(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let ResetSetsParams { id, sets } = params::parse(RESET_SETS, params)?;
    let id = activity_id(RESET_SETS, id)?;
    let indexes = set_refs(RESET_SETS, &sets)?
        .into_iter()
        .map(|set| match set {
            SetRef::Recorded(index) => Ok(index),
            SetRef::Added(added) => {
                Err(bad_params(RESET_SETS, format!("added set {added} has no record to go back to; remove it with activity.removeAddedSet")))
            }
        })
        .collect::<Result<Vec<u16>, EngineError>>()?;
    require_strength(session, RESET_SETS, id)?;
    session.store().reset_sets(id, &indexes).map_err(|error| EngineError::from_store(RESET_SETS, error))?;
    done(RESET_SETS, &view(session, RESET_SETS, id)?)
}

fn add_set(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let AddSetParams { id, exercise, weight_kg, reps: reps_count, after } = params::parse(ADD_SET, params)?;
    let id = activity_id(ADD_SET, id)?;
    let exercise = exercise_id(ADD_SET, &exercise)?;
    let weight = weight_kg.map(|kg| mass(ADD_SET, kg)).transpose()?;
    let reps = reps_count.map(|count| reps(ADD_SET, count)).transpose()?;
    let after = after.map(|set| set_ref(ADD_SET, set)).transpose()?;
    require_strength(session, ADD_SET, id)?;
    session.store().add_set(id, after, exercise, weight, reps, session.now()).map_err(|error| EngineError::from_store(ADD_SET, error))?;
    done(ADD_SET, &view(session, ADD_SET, id)?)
}

fn remove_added_set(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let RemoveAddedSetParams { id, set } = params::parse(REMOVE_ADDED_SET, params)?;
    let id = activity_id(REMOVE_ADDED_SET, id)?;
    let added = match set_ref(REMOVE_ADDED_SET, set)? {
        SetRef::Added(added) => added,
        SetRef::Recorded(index) => {
            return Err(bad_params(
                REMOVE_ADDED_SET,
                format!("set {index} was recorded by the watch; only an added set can be removed (activity.resetSets puts a recorded one back)"),
            ));
        }
    };
    require_strength(session, REMOVE_ADDED_SET, id)?;
    session.store().remove_added_set(id, added, session.now()).map_err(|error| EngineError::from_store(REMOVE_ADDED_SET, error))?;
    done(REMOVE_ADDED_SET, &view(session, REMOVE_ADDED_SET, id)?)
}

#[cfg(test)]
#[path = "activity_edit_tests.rs"]
mod edit_tests;
#[cfg(test)]
#[path = "activity_cmds_tests.rs"]
mod tests;
