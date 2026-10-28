//! `progress.list` and `progress.get`: how each exercise is going and whether to hold its weight or add to it.
//!
//! Both read the whole counted strength history ([`gymgar_store::Store::strength_history`]) and let `progress_views` turn it
//! into answers, with "today" from the session's clock. They change nothing and never reach Garmin, so they work in every
//! session, `gymgar-cli` included, and an agent sees what the user sees: the rules live in `gymgar_domain::progress`, not here.
//! What this module adds is the params: the filter and the page of the list, and the check that a key names an exercise.

use gymgar_domain::ExerciseId;
use gymgar_sync::exercises::{catalog, parse_exercise_key};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::commands::{always, done};
use crate::params::{self, bad_params};
use crate::progress_views::{AdviceKind, ProgressListView, full_view, list_view, sessions_by_exercise};
use crate::{CommandKind, CommandSpec, EngineError, Outcome, Session};

const LIST: &str = "progress.list";
const GET: &str = "progress.get";

/// How many tiles `progress.list` gives when `limit` is not set, and the most it gives.
const LIST_DEFAULT_LIMIT: u32 = 100;
const LIST_MAX_LIMIT: u32 = 1000;
/// An exercise key is a few words (`deadlift/romanian_deadlift`); a longer text is not one.
const MAX_KEY_CHARS: usize = 100;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ListParams {
    /// Only exercises with one of these statuses: `add`, `hold`, `break`, `bodyweight`, `noTarget`. Missing or empty means any.
    status: Option<Vec<AdviceKind>>,
    /// How many to return: 100 by default, 1 to 1000.
    #[schemars(range(min = 1, max = 1000))]
    limit: Option<u32>,
    /// How many of the first to skip, counted after `status` has been applied.
    offset: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct GetParams {
    /// The exercise, a key as `progress.list` or `exercise.search` gives it, for example `deadlift/romanian_deadlift`.
    exercise: String,
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: LIST,
            description: "Where the user's weights stand: one entry for every exercise with strength history, each with advice on whether to hold \
                          the weight or add to it. The working weight of an exercise in a session is the heaviest weight it was done with that day; \
                          the session's reps are the fewest reps among the sets at that weight. The target is the reps the Garmin workout planned. \
                          The status is `add` (the target was met in the last two sessions at the same weight, no more than 28 days apart: go up \
                          to nextWeightKg), `hold` (keep the weight), `break` (the last session was more than 28 days ago; this takes precedence \
                          over every other status), `bodyweight` (the working sets have no weight) or `noTarget` (no workout planned any reps, so \
                          there is nothing to compare with). Entries come in this order: add, then hold, bodyweight and noTarget, then break, then \
                          exercises not done for half a year; within a group the one done last first. Each entry has its latest session, how many \
                          sessions there were and the points of the last twelve months (sparkline, drawn for chartReps as in progress.get). \
                          Read-only. Use progress.get for one exercise with its chart and every session.",
            kind: CommandKind::Query,
            agent: true,
            destructive: false,
            params_schema: params::schema::<ListParams>,
            enabled: always,
            run: list,
        },
        CommandSpec {
            id: GET,
            description: "One exercise in full: its entry of progress.list (summary; null when the user has no strength history in it), the step its \
                          weight goes up by (stepKg), the chart, the pauses of more than 28 days (gaps), and every session, newest first, with its \
                          working sets, all its sets and the workout program's name. The chart is drawn for summary.chartReps reps: the target, or \
                          without one the most common reps of the working sets; it is null for an exercise that never had a weight, and then the \
                          chart is of reps. points has one point per month: the heaviest weight lifted for at least chartReps reps that month (in a \
                          chart of reps, the month's best reps with weightKg 0). heavier has a month's heaviest set with fewer than chartReps reps \
                          where that set is heavier than the month's point, so it appears also in months that have a point, and in months that \
                          have none; never in a chart of reps. The working weight is the heaviest weight of the exercise in a session, a session's \
                          reps are the fewest reps among the sets at that weight, and the target is the reps the Garmin workout planned. The advice \
                          carries what it rests on: the two sessions for `add` (basis), the session before the pause for `hold` after a break. \
                          `exercise` is a key from exercise.search or progress.list; an exercise the user never did answers with a null summary and \
                          nothing else. Read-only.",
            kind: CommandKind::Query,
            agent: true,
            destructive: false,
            params_schema: params::schema::<GetParams>,
            enabled: always,
            run: get,
        },
    ]
}

fn list(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let ListParams { status, limit, offset } = params::parse(LIST, params)?;
    let limit = limit.unwrap_or(LIST_DEFAULT_LIMIT);
    if !(1..=LIST_MAX_LIMIT).contains(&limit) {
        return Err(bad_params(LIST, format!("limit must be 1 to {LIST_MAX_LIMIT}, got {limit}")));
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let offset = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
    // An empty list of statuses names nothing to filter by, like an empty list of types in `activity.list`.
    let wanted = status.filter(|kinds| !kinds.is_empty());

    let by_exercise = sessions_by_exercise(session.store().strength_history()?);
    let titles = session.store().exercise_titles()?;
    let items = list_view(&by_exercise, &titles, session.now())
        .into_iter()
        .filter(|item| wanted.as_ref().is_none_or(|kinds| kinds.contains(&item.advice.kind)))
        .skip(offset)
        .take(limit)
        .collect();
    done(LIST, &ProgressListView { items })
}

fn get(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let GetParams { exercise } = params::parse(GET, params)?;
    if exercise.chars().count() > MAX_KEY_CHARS {
        return Err(bad_params(GET, format!("an exercise key has at most {MAX_KEY_CHARS} characters")));
    }
    let id = parse_exercise_key(&exercise).ok_or_else(|| bad_params(GET, format!("{exercise:?} is not an exercise key; exercise.search finds the keys")))?;

    let by_exercise = sessions_by_exercise(session.store().strength_history()?);
    // The catalog names the exercises a watch can record; the history may hold one it does not name (a newer watch), which is
    // still an exercise the user did. A key that is neither has nothing to answer about, and is most likely a typo.
    if !by_exercise.contains_key(&id) && !in_catalog(id) {
        return Err(bad_params(GET, format!("{exercise:?} names no exercise Garmin knows or the user has done; exercise.search finds the keys")));
    }
    let titles = session.store().exercise_titles()?;
    done(GET, &full_view(id, by_exercise.get(&id).map(Vec::as_slice), &titles, session.now()))
}

/// Whether Garmin's catalog names the exercise. Its "unknown" category is no exercise, though a key can spell it.
fn in_catalog(id: ExerciseId) -> bool {
    id.category != ExerciseId::UNKNOWN_CATEGORY && catalog().iter().any(|entry| entry.id == id)
}

#[cfg(test)]
#[path = "progress_cmds_tests.rs"]
mod tests;
