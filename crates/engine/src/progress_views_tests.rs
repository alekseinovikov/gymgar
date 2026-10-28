//! Tests of the progress views: how the history becomes sessions, the order of the tiles and what each view carries.
//! Pure: the history is built by hand, nothing touches a database. A set is a `(kilograms, reps, planned reps)` triple;
//! 0.0 kg means "no weight recorded".

use chrono::{Datelike, NaiveDate, NaiveDateTime, TimeZone};
use gymgar_domain::progress::ExerciseSession;
use gymgar_domain::{ActivityId, Mass, Reps, SetRef};
use gymgar_sync::exercises::{english_name, exercise_key, parse_exercise_key};
use serde_json::json;

use super::*;

const BENCH: ExerciseId = ExerciseId { category: 0, name: None };
const CURL: ExerciseId = ExerciseId { category: 7, name: None };
const DEADLIFT: ExerciseId = ExerciseId { category: 8, name: None };
const PLANK: ExerciseId = ExerciseId { category: 19, name: None };
const PUSH_UP: ExerciseId = ExerciseId { category: 22, name: None };
const SQUAT: ExerciseId = ExerciseId { category: 28, name: None };

type Triples<'a> = &'a [(f64, u32, Option<u32>)];

fn kg(value: f64) -> Mass {
    Mass::from_kg(value).unwrap()
}

fn reps(value: u32) -> Reps {
    Reps::new(value).unwrap()
}

fn date(day: &str) -> NaiveDate {
    NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap()
}

fn local(day: &str, hour: u32) -> NaiveDateTime {
    date(day).and_hms_opt(hour, 0, 0).unwrap()
}

fn utc(day: &str, hour: u32) -> DateTime<Utc> {
    Utc.from_utc_datetime(&local(day, hour))
}

/// "Now" of the views.
fn today() -> DateTime<Utc> {
    utc("2026-10-27", 12)
}

/// The activity of the helpers below on `day`: one per day, so a set can be traced back to its day.
fn activity_id(day: &str) -> ActivityId {
    ActivityId(i64::from(date(day).num_days_from_ce()))
}

/// A set of activity 1 at 08:00 on 2026-10-20, to be changed with `..`.
fn hset(exercise: ExerciseId, weight_kg: f64, reps_done: u32) -> HistorySet {
    HistorySet {
        activity: ActivityId(1),
        start: utc("2026-10-20", 8),
        local_start: None,
        workout_name: None,
        set: SetRef::Recorded(0),
        exercise,
        weight: (weight_kg > 0.0).then(|| kg(weight_kg)),
        reps: Some(reps(reps_done)),
        plan_reps: None,
    }
}

/// The sets of one activity begun at 08:00 (UTC and local) on `day`.
fn activity(exercise: ExerciseId, day: &str, sets: Triples) -> Vec<HistorySet> {
    sets.iter()
        .enumerate()
        .map(|(index, &(weight, reps_done, plan))| HistorySet {
            activity: activity_id(day),
            start: utc(day, 8),
            local_start: Some(local(day, 8)),
            workout_name: Some(format!("W {day}")),
            set: SetRef::Recorded(index as u16),
            plan_reps: plan.map(reps),
            ..hset(exercise, weight, reps_done)
        })
        .collect()
}

/// One activity per listed day, in the order of the days.
fn days(exercise: ExerciseId, days: &[(&str, Triples)]) -> Vec<HistorySet> {
    days.iter().flat_map(|&(day, sets)| activity(exercise, day, sets)).collect()
}

/// What the store returns: everything in order of the start.
fn history(parts: Vec<Vec<HistorySet>>) -> Vec<HistorySet> {
    let mut all: Vec<HistorySet> = parts.into_iter().flatten().collect();
    all.sort_by_key(|set| (set.start, set.activity));
    all
}

fn list(history: Vec<HistorySet>) -> Vec<ProgressItemView> {
    list_view(&sessions_by_exercise(history), &HashMap::new(), today())
}

fn item_of(items: &[ProgressItemView], exercise: ExerciseId) -> &ProgressItemView {
    items.iter().find(|item| item.exercise.id == exercise_key(exercise)).unwrap()
}

fn full(exercise: ExerciseId, history: Vec<HistorySet>) -> ProgressView {
    let by_exercise = sessions_by_exercise(history);
    full_view(exercise, by_exercise.get(&exercise).map(Vec::as_slice), &HashMap::new(), today())
}

fn working(weight_kg: f64, reps: &[u16], day: &str) -> WorkingView {
    WorkingView { weight_kg, reps: reps.to_vec(), date: date(day), activity_id: activity_id(day) }
}

#[test]
fn same_local_day_is_one_session() {
    let first = HistorySet {
        activity: ActivityId(1),
        start: utc("2026-10-20", 6),
        local_start: Some(local("2026-10-20", 9)),
        workout_name: Some("Push A".to_owned()),
        ..hset(BENCH, 60.0, 10)
    };
    let history = vec![
        first.clone(),
        HistorySet { set: SetRef::Recorded(1), reps: Some(reps(9)), ..first.clone() },
        HistorySet {
            activity: ActivityId(2),
            start: utc("2026-10-20", 18),
            local_start: Some(local("2026-10-20", 21)),
            workout_name: Some("Push B".to_owned()),
            ..hset(BENCH, 62.5, 8)
        },
    ];
    let sessions = sessions_by_exercise(history);
    assert_eq!(sessions.keys().copied().collect::<Vec<_>>(), [BENCH]);
    let bench = sessions.get(&BENCH).unwrap();
    assert_eq!(bench.len(), 1, "two activities on one day are one session");
    let session = bench.first().unwrap();
    assert_eq!(session.start, utc("2026-10-20", 6), "the earlier activity gives the start");
    assert_eq!(session.day, date("2026-10-20"));
    assert_eq!(session.workout_name.as_deref(), Some("Push A"), "the first activity of the day names the session");
    let seen: Vec<(i64, Option<f64>, Option<u16>)> =
        session.sets.iter().map(|set| (set.activity.0, set.weight.map(Mass::kg), set.reps.map(Reps::get))).collect();
    assert_eq!(seen, [(1, Some(60.0), Some(10)), (1, Some(60.0), Some(9)), (2, Some(62.5), Some(8))], "the sets of both, in the order of the history");
}

#[test]
fn the_session_day_is_the_local_day_else_the_utc_day() {
    // Late in the evening in UTC, already the next morning where the user was: the local day is the training day.
    let travelled = HistorySet { start: utc("2026-10-20", 23), local_start: Some(local("2026-10-21", 2)), ..hset(BENCH, 60.0, 10) };
    // No local time (Garmin sent none): the UTC date.
    let unplaced = HistorySet { activity: ActivityId(2), start: utc("2026-10-22", 23), local_start: None, ..hset(BENCH, 60.0, 10) };
    let sessions = sessions_by_exercise(vec![travelled, unplaced]);
    let days: Vec<NaiveDate> = sessions.get(&BENCH).unwrap().iter().map(|session| session.day).collect();
    assert_eq!(days, [date("2026-10-21"), date("2026-10-22")]);
}

#[test]
fn sessions_of_an_exercise_run_from_old_to_new() {
    // Two days of one exercise, handed over newest first and with a day split between two interleaved activities.
    let history = vec![
        HistorySet { start: utc("2026-10-21", 8), local_start: Some(local("2026-10-21", 8)), ..hset(BENCH, 62.5, 10) },
        HistorySet { activity: ActivityId(2), start: utc("2026-10-20", 8), local_start: Some(local("2026-10-20", 8)), ..hset(BENCH, 60.0, 10) },
        HistorySet { activity: ActivityId(3), start: utc("2026-10-21", 7), local_start: Some(local("2026-10-21", 7)), ..hset(BENCH, 60.0, 8) },
    ];
    let sessions = sessions_by_exercise(history);
    let bench = sessions.get(&BENCH).unwrap();
    let days: Vec<(NaiveDate, DateTime<Utc>)> = bench.iter().map(|session| (session.day, session.start)).collect();
    assert_eq!(days, [(date("2026-10-20"), utc("2026-10-20", 8)), (date("2026-10-21"), utc("2026-10-21", 7))]);
}

#[test]
fn only_strength_exercises_make_sessions() {
    let warm_up = parse_exercise_key("warm_up").unwrap();
    let cardio = parse_exercise_key("cardio").unwrap();
    let unknown = ExerciseId { category: ExerciseId::UNKNOWN_CATEGORY, name: Some(3) };
    let history = history(vec![
        activity(warm_up, "2026-10-20", &[(0.0, 10, None)]),
        activity(cardio, "2026-10-20", &[(0.0, 30, None)]),
        activity(unknown, "2026-10-20", &[(20.0, 10, None)]),
        activity(CURL, "2026-10-20", &[(20.0, 10, None)]),
    ]);
    let sessions = sessions_by_exercise(history);
    assert_eq!(sessions.keys().copied().collect::<Vec<_>>(), [CURL]);
}

#[test]
fn exercises_without_reps_are_left_out() {
    let nothing = |activity: i64| HistorySet { activity: ActivityId(activity), reps: None, ..hset(PLANK, 0.0, 0) };
    let history = history(vec![
        // Sets without reps, or with none done, are no progress to speak of.
        vec![nothing(1), HistorySet { activity: ActivityId(2), reps: Some(reps(0)), ..hset(PLANK, 0.0, 0) }],
        activity(CURL, "2026-10-20", &[(20.0, 10, Some(10))]),
    ]);
    let sessions = sessions_by_exercise(history.clone());
    assert_eq!(sessions.keys().copied().collect::<Vec<_>>(), [CURL]);
    let items = list(history);
    let keys: Vec<&str> = items.iter().map(|item| item.exercise.id.as_str()).collect();
    assert_eq!(keys, ["curl"]);
}

#[test]
fn one_set_with_reps_keeps_the_sets_without() {
    let history = history(vec![vec![
        HistorySet { reps: None, set: SetRef::Recorded(0), ..hset(CURL, 20.0, 0) },
        HistorySet { set: SetRef::Recorded(1), ..hset(CURL, 20.0, 10) },
    ]]);
    let sessions = sessions_by_exercise(history);
    let session = sessions.get(&CURL).unwrap().first().unwrap();
    assert_eq!(session.sets.len(), 2);
}

/// The five tiles of the spec in an order that is neither the ids' nor the dates': by id it would be the other way round.
fn five_tiles() -> Vec<HistorySet> {
    history(vec![
        // Two sessions at the same weight, both met the plan: add weight.
        days(SQUAT, &[("2026-10-20", &[(80.0, 10, Some(10))]), ("2026-10-24", &[(80.0, 10, Some(10))])]),
        // One session short of the plan: hold.
        days(PLANK, &[("2026-10-26", &[(60.0, 8, Some(10))])]),
        // A plan-less workout: nothing to compare with.
        days(DEADLIFT, &[("2026-10-22", &[(60.0, 10, None)])]),
        // 40 days ago: a break.
        days(CURL, &[("2026-09-17", &[(50.0, 10, Some(10))])]),
        // 200 days ago: dormant.
        days(BENCH, &[("2026-04-10", &[(50.0, 10, Some(10))])]),
    ])
}

#[test]
fn list_order_follows_the_tiles() {
    let items = list(five_tiles());
    let order: Vec<(&str, AdviceKind, bool)> = items.iter().map(|item| (item.exercise.id.as_str(), item.advice.kind, item.dormant)).collect();
    assert_eq!(
        order,
        [
            ("squat", AdviceKind::Add, false),
            ("plank", AdviceKind::Hold, false),
            ("deadlift", AdviceKind::NoTarget, false),
            ("curl", AdviceKind::Break, false),
            ("bench_press", AdviceKind::Break, true),
        ]
    );
    // What a tile says about the exercise itself.
    let squat = item_of(&items, SQUAT);
    assert_eq!((squat.sessions, squat.first_date, squat.last.date), (2, date("2026-10-20"), date("2026-10-24")));
    assert_eq!(squat.chart_reps, Some(10));
    assert_eq!(squat.exercise.english, english_name(SQUAT));
}

#[test]
fn a_tile_group_goes_by_the_last_session_then_by_key() {
    let items = list(history(vec![
        // Both on the same day: the key decides, and "c1000" sorts before "c900" though 900 is the smaller number.
        days(ExerciseId { category: 900, name: None }, &[("2026-10-25", &[(60.0, 10, Some(10))])]),
        days(ExerciseId { category: 1000, name: None }, &[("2026-10-25", &[(60.0, 10, Some(10))])]),
        // A day later, and without weight: the same group, and it comes first.
        days(PUSH_UP, &[("2026-10-26", &[(0.0, 15, Some(15))])]),
        // Older than both, but not yet a break.
        days(CURL, &[("2026-10-10", &[(20.0, 10, Some(10))])]),
    ]));
    let order: Vec<(&str, AdviceKind)> = items.iter().map(|item| (item.exercise.id.as_str(), item.advice.kind)).collect();
    assert_eq!(order, [("push_up", AdviceKind::Bodyweight), ("c1000", AdviceKind::Hold), ("c900", AdviceKind::Hold), ("curl", AdviceKind::Hold)]);
}

#[test]
fn dormant_starts_after_182_days() {
    // 182 days and four hours before "now", and one day more.
    let items = list(history(vec![days(CURL, &[("2026-04-28", &[(20.0, 10, Some(10))])]), days(BENCH, &[("2026-04-27", &[(20.0, 10, Some(10))])])]));
    assert!(!item_of(&items, CURL).dormant);
    assert!(item_of(&items, BENCH).dormant);
    // Both are breaks; only the older is dormant, so it goes last.
    let order: Vec<&str> = items.iter().map(|item| item.exercise.id.as_str()).collect();
    assert_eq!(order, ["curl", "bench_press"]);
}

#[test]
fn views_carry_the_basis() {
    let items = list(history(vec![
        // Add: the sessions the advice rests on, with the sets at the working weight.
        days(
            SQUAT,
            &[
                ("2026-10-13", &[(80.0, 10, Some(10)), (80.0, 10, Some(10)), (60.0, 12, Some(10))]),
                ("2026-10-20", &[(80.0, 10, Some(10)), (80.0, 11, Some(10))]),
            ],
        ),
        // Hold after a break: the length of the pause and the session before it.
        days(DEADLIFT, &[("2026-08-10", &[(80.0, 10, Some(10)), (80.0, 9, Some(10))]), ("2026-10-20", &[(70.0, 8, Some(10))])]),
    ]));

    let add = &item_of(&items, SQUAT).advice;
    assert_eq!(add.kind, AdviceKind::Add);
    assert_eq!(add.target, Some(10));
    assert_eq!(add.next_weight_kg, Some(85.0));
    assert_eq!(add.basis, [working(80.0, &[10, 10], "2026-10-13"), working(80.0, &[10, 11], "2026-10-20")]);
    assert_eq!((add.sessions_at_weight, add.met_once, add.break_days, add.before_break.clone()), (None, false, None, None));
    assert_eq!(item_of(&items, SQUAT).last, working(80.0, &[10, 11], "2026-10-20"));

    let hold = &item_of(&items, DEADLIFT).advice;
    assert_eq!(hold.kind, AdviceKind::Hold);
    assert_eq!((hold.target, hold.sessions_at_weight, hold.met_once), (Some(10), Some(1), false));
    assert_eq!(hold.break_days, Some(71));
    assert_eq!(hold.before_break, Some(working(80.0, &[10, 9], "2026-08-10")));
    assert_eq!((hold.next_weight_kg, hold.basis.clone()), (None, Vec::new()));
}

#[test]
fn a_hold_without_a_break_has_no_pause_to_report() {
    let items = list(history(vec![days(SQUAT, &[("2026-10-13", &[(80.0, 10, Some(10))]), ("2026-10-20", &[(82.5, 10, Some(10))])])]));
    let hold = &item_of(&items, SQUAT).advice;
    assert_eq!(hold.kind, AdviceKind::Hold);
    assert_eq!((hold.sessions_at_weight, hold.met_once), (Some(1), true));
    assert_eq!((hold.break_days, hold.before_break.clone()), (None, None));
}

#[test]
fn the_other_advice_carries_only_what_it_says() {
    let items = list(five_tiles());
    let bare = |kind| AdviceView {
        kind,
        target: None,
        next_weight_kg: None,
        sessions_at_weight: None,
        met_once: false,
        basis: Vec::new(),
        break_days: None,
        before_break: None,
    };
    assert_eq!(item_of(&items, CURL).advice, AdviceView { break_days: Some(40), ..bare(AdviceKind::Break) });
    assert_eq!(item_of(&items, BENCH).advice, AdviceView { break_days: Some(200), ..bare(AdviceKind::Break) });
    assert_eq!(item_of(&items, DEADLIFT).advice, bare(AdviceKind::NoTarget));

    let items = list(history(vec![days(PUSH_UP, &[("2026-10-26", &[(0.0, 15, Some(15))])])]));
    assert_eq!(item_of(&items, PUSH_UP).advice, bare(AdviceKind::Bodyweight));
}

/// One session on the 15th of each of the 14 months from September 2025 to October 2026, at a kilogram more each time.
fn fourteen_months() -> Vec<HistorySet> {
    let parts = (0..14_u32)
        .map(|index| {
            let months = 2025 * 12 + 8 + index;
            let day = format!("{}-{:02}-15", months / 12, months % 12 + 1);
            activity(SQUAT, &day, &[(60.0 + f64::from(index), 10, Some(10))])
        })
        .collect();
    history(parts)
}

#[test]
fn sparkline_is_the_last_twelve_months() {
    let items = list(fourteen_months());
    let squat = item_of(&items, SQUAT);
    let months: Vec<&str> = squat.sparkline.iter().map(|point| point.month.as_str()).collect();
    assert_eq!(months, ["2025-11", "2025-12", "2026-01", "2026-02", "2026-03", "2026-04", "2026-05", "2026-06", "2026-07", "2026-08", "2026-09", "2026-10"]);
    // The point is the set that stands for its month, with where it came from.
    let first = squat.sparkline.first().unwrap();
    assert_eq!(first, &PointView { month: "2025-11".to_owned(), weight_kg: 62.0, reps: 10, date: date("2025-11-15"), activity_id: activity_id("2025-11-15") });
    assert_eq!((squat.sessions, squat.first_date), (14, date("2025-09-15")));

    // The chart of the exercise itself keeps all fourteen.
    let view = full(SQUAT, fourteen_months());
    assert_eq!(view.points.len(), 14);
    assert_eq!(view.points.first().map(|point| point.month.as_str()), Some("2025-09"));
}

#[test]
fn a_reps_chart_has_no_weight_to_draw() {
    let items = list(history(vec![days(PUSH_UP, &[("2026-10-13", &[(0.0, 15, Some(15)), (0.0, 12, Some(15))]), ("2026-10-20", &[(0.0, 14, Some(15))])])]));
    let push_up = item_of(&items, PUSH_UP);
    assert_eq!(push_up.chart_reps, None);
    assert_eq!(
        push_up.sparkline,
        [PointView { month: "2026-10".to_owned(), weight_kg: 0.0, reps: 14, date: date("2026-10-20"), activity_id: activity_id("2026-10-20") }]
    );
}

#[test]
fn full_view_of_an_exercise_never_done() {
    for sessions in [None, Some(&[] as &[ExerciseSession])] {
        let view = full_view(SQUAT, sessions, &HashMap::new(), today());
        assert_eq!(view.exercise, ExerciseView { id: "squat".to_owned(), english: english_name(SQUAT), watch_title: None });
        assert_eq!(view.summary, None);
        assert_eq!(view.step_kg, None);
        assert!(view.points.is_empty() && view.heavier.is_empty() && view.gaps.is_empty() && view.sessions.is_empty());
    }
}

#[test]
fn the_watch_title_comes_with_the_exercise() {
    let titles = HashMap::from([(CURL, "Bizeps".to_owned())]);
    let by_exercise = sessions_by_exercise(days(CURL, &[("2026-10-20", &[(20.0, 10, Some(10))])]));
    let items = list_view(&by_exercise, &titles, today());
    assert_eq!(items.first().unwrap().exercise.watch_title.as_deref(), Some("Bizeps"));
    let view = full_view(CURL, by_exercise.get(&CURL).map(Vec::as_slice), &titles, today());
    assert_eq!(view.exercise.watch_title.as_deref(), Some("Bizeps"));
    assert_eq!(view.summary.unwrap().exercise.watch_title.as_deref(), Some("Bizeps"));
}

#[test]
fn sessions_are_newest_first_with_all_sets() {
    let mut october_13 = activity(SQUAT, "2026-10-13", &[(60.0, 10, Some(10)), (60.0, 10, Some(10))]);
    // A set the watch recorded with nothing in it, between the others.
    october_13.push(HistorySet { weight: None, reps: None, set: SetRef::Recorded(2), ..october_13.first().unwrap().clone() });
    let view = full(
        SQUAT,
        history(vec![
            activity(SQUAT, "2026-10-06", &[(40.0, 12, Some(10)), (60.0, 10, Some(10)), (60.0, 9, Some(10))]),
            october_13,
            activity(SQUAT, "2026-10-20", &[(62.5, 10, Some(10)), (62.5, 8, Some(10))]),
        ]),
    );
    let sessions: Vec<NaiveDate> = view.sessions.iter().map(|session| session.date).collect();
    assert_eq!(sessions, [date("2026-10-20"), date("2026-10-13"), date("2026-10-06")]);

    let brief = |weight_kg, reps| SetBriefView { weight_kg, reps };
    let newest = view.sessions.first().unwrap();
    assert_eq!(
        *newest,
        SessionView {
            date: date("2026-10-20"),
            start: utc("2026-10-20", 8),
            activity_id: activity_id("2026-10-20"),
            workout_name: Some("W 2026-10-20".to_owned()),
            working: working(62.5, &[10, 8], "2026-10-20"),
            sets: vec![brief(Some(62.5), Some(10)), brief(Some(62.5), Some(8))],
        }
    );
    let middle = view.sessions.get(1).unwrap();
    assert_eq!(middle.sets, [brief(Some(60.0), Some(10)), brief(Some(60.0), Some(10)), brief(None, None)]);
    let oldest = view.sessions.get(2).unwrap();
    assert_eq!(oldest.working, working(60.0, &[10, 9], "2026-10-06"), "the warm-up is not the working weight, but it is listed");
    assert_eq!(oldest.sets, [brief(Some(40.0), Some(12)), brief(Some(60.0), Some(10)), brief(Some(60.0), Some(9))]);
}

#[test]
fn full_view_carries_the_summary_the_chart_and_the_pauses() {
    let view = full(
        SQUAT,
        history(vec![
            // The 90 kg set is below the target of 10 reps: a circle in July, next to the point of 80 kg.
            days(SQUAT, &[("2026-07-01", &[(80.0, 10, Some(10)), (90.0, 5, Some(10))]), ("2026-09-01", &[(80.0, 10, Some(10))])]),
        ]),
    );
    let summary = view.summary.as_ref().unwrap();
    assert_eq!(summary.exercise, view.exercise);
    // The latest session is 56 days old: a break, which is also the length of the pause the chart shows.
    assert_eq!((summary.advice.kind, summary.advice.break_days, summary.sessions, summary.chart_reps), (AdviceKind::Break, Some(56), 2, Some(10)));
    assert_eq!(summary.last, working(80.0, &[10], "2026-09-01"));
    // The smallest step between the working weights (90 and 80 kg) is 10 kg; a step is at most 5.
    assert_eq!(view.step_kg, Some(5.0));
    let point =
        |month: &str, weight_kg, reps, day: &str| PointView { month: month.to_owned(), weight_kg, reps, date: date(day), activity_id: activity_id(day) };
    assert_eq!(view.points, [point("2026-07", 80.0, 10, "2026-07-01"), point("2026-09", 80.0, 10, "2026-09-01")]);
    assert_eq!(view.heavier, [point("2026-07", 90.0, 5, "2026-07-01")]);
    assert_eq!(
        view.gaps,
        [
            GapView { from: date("2026-07-01"), to: Some(date("2026-09-01")), days: 62 },
            // The pause still going on has no end.
            GapView { from: date("2026-09-01"), to: None, days: 56 },
        ]
    );
}

#[test]
fn the_views_are_camel_case_json_the_ui_reads_back() {
    let history = history(vec![days(SQUAT, &[("2026-10-13", &[(80.0, 10, Some(10))]), ("2026-10-20", &[(80.0, 10, Some(10))])])]);
    let by_exercise = sessions_by_exercise(history);
    let list = ProgressListView { items: list_view(&by_exercise, &HashMap::new(), today()) };
    let view = full_view(SQUAT, by_exercise.get(&SQUAT).map(Vec::as_slice), &HashMap::new(), today());

    let json = serde_json::to_value(&list).unwrap();
    let item = json.pointer("/items/0").unwrap();
    assert_eq!(item.pointer("/advice/kind"), Some(&json!("add")));
    assert_eq!(item.pointer("/advice/nextWeightKg"), Some(&json!(85.0)));
    assert_eq!(item.pointer("/advice/basis/0/activityId"), Some(&json!(activity_id("2026-10-13").0)));
    assert_eq!(item.pointer("/advice/basis/0/date"), Some(&json!("2026-10-13")));
    assert_eq!(item.pointer("/firstDate"), Some(&json!("2026-10-13")));
    assert_eq!(item.pointer("/sparkline/0/month"), Some(&json!("2026-10")));
    assert_eq!(item.pointer("/sparkline/0/weightKg"), Some(&json!(80.0)));
    assert_eq!(serde_json::from_value::<ProgressListView>(json).unwrap(), list);

    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json.pointer("/stepKg"), Some(&json!(2.5)));
    assert_eq!(json.pointer("/sessions/0/workoutName"), Some(&json!("W 2026-10-20")));
    assert_eq!(serde_json::from_value::<ProgressView>(json).unwrap(), view);

    for (kind, text) in [
        (AdviceKind::Add, "add"),
        (AdviceKind::Hold, "hold"),
        (AdviceKind::Break, "break"),
        (AdviceKind::Bodyweight, "bodyweight"),
        (AdviceKind::NoTarget, "noTarget"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(text));
        assert_eq!(serde_json::from_value::<AdviceKind>(json!(text)).unwrap(), kind);
    }
    assert!(serde_json::from_value::<AdviceKind>(json!("no_target")).is_err());
    // The filter a command takes: its schema lists exactly these words.
    let schema = serde_json::to_string(&schemars::schema_for!(AdviceKind)).unwrap();
    assert!(schema.contains("noTarget") && schema.contains("bodyweight"), "{schema}");
}
