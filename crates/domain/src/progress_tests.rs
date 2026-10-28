//! Tests of the progress rule: the working sets of a session, the target, the weight step, the advice table and the chart series.
//! Sessions are hand-made from `(kilograms, reps, planned reps)` triples; 0.0 kg means "no weight recorded".

use chrono::{Datelike, Days, NaiveDate, TimeZone};

use super::*;

fn kg(value: f64) -> Mass {
    Mass::from_kg(value).unwrap()
}

fn grams(value: i64) -> Mass {
    Mass::from_grams(value).unwrap()
}

fn reps(value: u32) -> Reps {
    Reps::new(value).unwrap()
}

fn date(day: &str) -> NaiveDate {
    NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap()
}

fn at(day: &str, hour: u32) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date(day).and_hms_opt(hour, 0, 0).unwrap())
}

/// "Now" of the advice table.
fn today() -> DateTime<Utc> {
    at("2026-10-27", 12)
}

/// One activity per day, so a set can be traced back to its day by the activity id.
fn activity(day: NaiveDate) -> ActivityId {
    ActivityId(i64::from(day.num_days_from_ce()))
}

fn set(day: NaiveDate, index: u16, weight: f64, reps_done: u32, plan: Option<u32>) -> ProgressSet {
    ProgressSet {
        activity: activity(day),
        set: SetRef::Recorded(index),
        weight: if weight == 0.0 { None } else { Some(kg(weight)) },
        reps: Some(reps(reps_done)),
        plan_reps: plan.map(reps),
    }
}

fn session_on(day: NaiveDate, sets: &[(f64, u32, Option<u32>)]) -> ExerciseSession {
    let sets = sets.iter().enumerate().map(|(index, &(weight, reps_done, plan))| set(day, index as u16, weight, reps_done, plan)).collect();
    ExerciseSession { start: Utc.from_utc_datetime(&day.and_hms_opt(8, 0, 0).unwrap()), day, workout_name: None, sets }
}

fn session(day: &str, sets: &[(f64, u32, Option<u32>)]) -> ExerciseSession {
    session_on(date(day), sets)
}

/// A session of a workout whose plan asks for 10 reps everywhere.
fn planned(day: &str, sets: &[(f64, u32)]) -> ExerciseSession {
    let sets: Vec<(f64, u32, Option<u32>)> = sets.iter().map(|&(weight, reps_done)| (weight, reps_done, Some(10))).collect();
    session(day, &sets)
}

/// One set per session, on consecutive days, with no plan.
fn sessions_at(weights: &[f64]) -> Vec<ExerciseSession> {
    weights
        .iter()
        .enumerate()
        .map(|(index, &weight)| session_on(date("2026-01-01").checked_add_days(Days::new(index as u64)).unwrap(), &[(weight, 10, None)]))
        .collect()
}

#[test]
fn constants_are_the_values_of_the_spec() {
    assert_eq!((BREAK_DAYS, DORMANT_DAYS, ADD_PERCENT), (28, 182, 5));
    assert_eq!((STEP_MIN_G, STEP_MAX_G, STEP_DEFAULT_G), (1_000, 5_000, 2_500));
    assert_eq!(SPARKLINE_MONTHS, 12);
}

#[test]
fn days_between_counts_whole_days_and_is_never_negative() {
    assert_eq!(days_between(at("2026-10-01", 8), at("2026-10-29", 8)), 28);
    assert_eq!(days_between(at("2026-10-01", 8), at("2026-10-29", 7)), 27);
    assert_eq!(days_between(at("2026-10-01", 8), at("2026-10-01", 20)), 0);
    assert_eq!(days_between(at("2026-10-30", 8), at("2026-10-27", 12)), 0);
}

#[test]
fn working_sets_take_the_fewest_reps_at_the_top_weight() {
    let working = working_sets(&session("2026-10-01", &[(40.0, 10, None), (60.0, 10, None), (80.0, 10, None), (90.0, 6, None)])).unwrap();
    assert_eq!((working.weight, working.reps, working.all_reps.clone()), (kg(90.0), 6, vec![6]));
    assert_eq!(working.set.set, SetRef::Recorded(3));

    let working = working_sets(&session("2026-10-01", &[(100.0, 8, None), (100.0, 8, None), (100.0, 6, None)])).unwrap();
    assert_eq!((working.weight, working.reps, working.all_reps.clone()), (kg(100.0), 6, vec![8, 8, 6]));
    assert_eq!(working.set.set, SetRef::Recorded(2));

    // Without any weight the working weight is 0 and every set is a working set.
    let working = working_sets(&session("2026-10-01", &[(0.0, 12, None), (0.0, 10, None)])).unwrap();
    assert_eq!((working.weight, working.reps, working.all_reps.clone()), (grams(0), 10, vec![12, 10]));
    assert_eq!(working.set.set, SetRef::Recorded(1));

    // Of equally low sets the first one in the workout stands for the session.
    let working = working_sets(&session("2026-10-01", &[(100.0, 6, None), (100.0, 8, None), (100.0, 6, None)])).unwrap();
    assert_eq!((working.reps, working.set.set), (6, SetRef::Recorded(0)));
}

#[test]
fn a_working_set_without_reps_counts_as_zero_reps() {
    let mut one = session("2026-10-01", &[(100.0, 8, None), (100.0, 8, None)]);
    one.sets[1].reps = None;
    let working = working_sets(&one).unwrap();
    assert_eq!((working.reps, working.all_reps.clone(), working.set.set), (0, vec![8, 0], SetRef::Recorded(1)));
}

#[test]
fn an_empty_session_has_no_working_sets() {
    assert_eq!(working_sets(&session("2026-10-01", &[])), None);
}

#[test]
fn target_comes_from_the_plan() {
    // The plan of the working set.
    assert_eq!(target_reps(&[session("2026-10-01", &[(80.0, 10, Some(10))])]), Some(10));
    // The working set has no plan: the most frequent plan of the other sets.
    let neighbours = session("2026-10-01", &[(80.0, 10, None), (60.0, 10, Some(12)), (60.0, 10, Some(12)), (60.0, 10, Some(10))]);
    assert_eq!(target_reps(&[neighbours]), Some(12));
    // A tie goes to the larger number.
    let tie = session("2026-10-01", &[(80.0, 10, None), (60.0, 10, Some(10)), (60.0, 10, Some(12))]);
    assert_eq!(target_reps(&[tie]), Some(12));
    // No plan anywhere.
    assert_eq!(target_reps(&[session("2026-10-01", &[(80.0, 10, None)])]), None);
    assert_eq!(target_reps(&[]), None);
    // A plan of 0 reps is not a plan.
    assert_eq!(target_reps(&[session("2026-10-01", &[(80.0, 10, Some(0))])]), None);
    let zero_then_neighbour = session("2026-10-01", &[(80.0, 10, Some(0)), (60.0, 10, Some(12))]);
    assert_eq!(target_reps(&[zero_then_neighbour]), Some(12));
    // The last session has no plan: the previous one does.
    let older = session("2026-10-01", &[(80.0, 10, Some(8))]);
    let newer = session("2026-10-08", &[(80.0, 10, None)]);
    assert_eq!(target_reps(&[older, newer]), Some(8));
    // The latest plan wins over an older one.
    let older = session("2026-10-01", &[(80.0, 10, Some(8))]);
    let newer = session("2026-10-08", &[(80.0, 10, Some(10))]);
    assert_eq!(target_reps(&[older, newer]), Some(10));
}

#[test]
fn step_and_next_weight() {
    // (working weights over the history, expected step in grams, weight, expected next weight in kg)
    let table: [(&[f64], i64, f64, f64); 5] = [
        (&[100.0, 105.0, 90.0, 80.0], 5_000, 80.0, 85.0),
        (&[24.0, 25.0, 28.0, 35.0], 1_000, 24.0, 26.0),
        (&[48.0, 50.0, 52.0, 56.0], 2_000, 52.0, 56.0),
        (&[80.0], 2_500, 80.0, 85.0),
        // 40 and 40.5: the difference of 0.5 kg is below the floor.
        (&[40.0, 40.5], 1_000, 40.5, 43.0),
    ];
    for (weights, step_g, weight, next) in table {
        let step = weight_step(&sessions_at(weights));
        assert_eq!(step.grams(), step_g, "step of {weights:?}");
        assert_eq!(next_weight(kg(weight), step), Some(kg(next)), "next after {weight} kg with a step of {step_g} g");
    }
    // Differences of 10 and 20 kg are capped at 5 kg.
    assert_eq!(weight_step(&sessions_at(&[60.0, 70.0, 90.0])).grams(), 5_000);
    // Fewer than two different working weights: the default; no weight and repeated weights do not count.
    assert_eq!(weight_step(&sessions_at(&[])).grams(), 2_500);
    assert_eq!(weight_step(&sessions_at(&[0.0, 80.0])).grams(), 2_500);
    assert_eq!(weight_step(&sessions_at(&[80.0, 80.0, 80.0])).grams(), 2_500);
    // The step is the smallest difference of all, not the latest one.
    assert_eq!(weight_step(&sessions_at(&[50.0, 60.0, 62.0, 100.0])).grams(), 2_000);

    // No next weight when it does not fit in a `Mass`, or when the step is nothing.
    assert_eq!(next_weight(kg(1000.0), kg(5.0)), None);
    assert_eq!(next_weight(kg(80.0), grams(0)), None);
    // The next weight is always strictly more, even where 5 % rounds down to nothing.
    assert_eq!(next_weight(grams(0), kg(2.5)), Some(kg(2.5)));
    assert_eq!(next_weight(grams(1), kg(1.0)), Some(kg(1.0)));
    // 5 % of 80.953 kg is 85.00065 kg: rounded down it would be a multiple of 5 kg that is under 5 %, so it is rounded up.
    assert_eq!(next_weight(grams(80_953), kg(5.0)), Some(kg(90.0)));
    // A weight that is already a multiple of the step still moves up by at least 5 %.
    assert_eq!(next_weight(kg(100.0), kg(5.0)), Some(kg(105.0)));
    assert_eq!(next_weight(kg(120.0), kg(5.0)), Some(kg(130.0)));
}

#[test]
fn advice_table() {
    let hold = |weight: f64, reps: u16, sessions_at_weight: u32, met_once: bool| Advice::Hold {
        weight: kg(weight),
        reps,
        target: 10,
        sessions_at_weight,
        met_once,
        after_break: None,
    };
    let table: Vec<(&str, Vec<ExerciseSession>, Advice)> = vec![
        (
            "two sessions at the target, 16 days apart",
            vec![planned("2026-09-30", &[(80.0, 10)]), planned("2026-10-16", &[(80.0, 10)])],
            Advice::Add { weight: kg(80.0), to: kg(85.0), target: 10, previous: 0, last: 1 },
        ),
        (
            "the last session misses the target with one set",
            vec![planned("2026-10-01", &[(80.0, 10)]), planned("2026-10-10", &[(80.0, 10), (80.0, 10), (80.0, 9)])],
            hold(80.0, 9, 2, false),
        ),
        ("the target is met after a miss", vec![planned("2026-10-01", &[(80.0, 9)]), planned("2026-10-10", &[(80.0, 10)])], hold(80.0, 10, 2, true)),
        ("the weight went up", vec![planned("2026-10-01", &[(80.0, 10)]), planned("2026-10-10", &[(85.0, 10)])], hold(85.0, 10, 1, true)),
        (
            "exactly 28 days between the sessions is not a break",
            vec![planned("2026-09-02", &[(80.0, 10)]), planned("2026-09-30", &[(80.0, 10)])],
            Advice::Add { weight: kg(80.0), to: kg(85.0), target: 10, previous: 0, last: 1 },
        ),
        (
            "29 days between the sessions is a break before the last one",
            vec![planned("2026-09-01", &[(80.0, 10)]), planned("2026-09-30", &[(80.0, 10)])],
            Advice::Hold {
                weight: kg(80.0),
                reps: 10,
                target: 10,
                sessions_at_weight: 1,
                met_once: true,
                after_break: Some(AfterBreak { days: 29, before: 0 }),
            },
        ),
        ("the last session was 28 days ago", vec![planned("2026-09-29", &[(80.0, 10)])], hold(80.0, 10, 1, true)),
        ("the last session was 29 days ago", vec![planned("2026-09-28", &[(80.0, 10)])], Advice::Break { days: 29, last: 0 }),
        (
            "a break comes before the lack of a weight",
            vec![planned("2026-08-01", &[(75.0, 10)]), planned("2026-09-01", &[(0.0, 10)])],
            Advice::Break { days: 56, last: 1 },
        ),
        (
            "no weight in the last session although there was one before",
            vec![planned("2026-10-01", &[(75.0, 10)]), planned("2026-10-15", &[(0.0, 10)])],
            Advice::Bodyweight { last: 1 },
        ),
        ("no plan anywhere", vec![session("2026-10-01", &[(80.0, 10, None)]), session("2026-10-10", &[(80.0, 10, None)])], Advice::NoTarget { last: 1 }),
        ("a single session at the target", vec![planned("2026-10-10", &[(80.0, 10)])], hold(80.0, 10, 1, true)),
        ("a session three days after today", vec![planned("2026-10-30", &[(80.0, 10)])], hold(80.0, 10, 1, true)),
        ("no heavier weight fits in a mass", vec![planned("2026-10-01", &[(1000.0, 10)]), planned("2026-10-10", &[(1000.0, 10)])], hold(1000.0, 10, 2, true)),
        (
            "three sessions in a row on the same weight count back to the first",
            vec![planned("2026-09-20", &[(80.0, 8)]), planned("2026-09-30", &[(80.0, 9)]), planned("2026-10-10", &[(80.0, 9)])],
            hold(80.0, 9, 3, false),
        ),
        (
            "the run stops at an earlier weight",
            vec![planned("2026-09-20", &[(75.0, 10)]), planned("2026-09-30", &[(80.0, 9)]), planned("2026-10-10", &[(80.0, 9)])],
            hold(80.0, 9, 2, false),
        ),
    ];
    for (name, sessions, expected) in table {
        assert_eq!(advise(&sessions, today()), Some(expected), "{name}");
    }
    assert_eq!(advise(&[], today()), None);
    // A session from the future is no time at all.
    assert_eq!(days_between(at("2026-10-30", 8), today()), 0);
}

#[test]
fn a_gap_between_sessions_stops_the_run_at_the_same_weight() {
    // 80 kg three times, but the first one is more than 28 days before the second: only the last two count.
    let sessions = vec![planned("2026-08-20", &[(80.0, 8)]), planned("2026-09-30", &[(80.0, 9)]), planned("2026-10-10", &[(80.0, 9)])];
    assert_eq!(
        advise(&sessions, today()),
        Some(Advice::Hold { weight: kg(80.0), reps: 9, target: 10, sessions_at_weight: 2, met_once: false, after_break: None })
    );
}

#[test]
fn chart_reps_follow_the_target_and_without_one_the_most_common_working_set_reps() {
    assert_eq!(chart_reps(&[planned("2026-10-01", &[(80.0, 7)])]), 10);
    // No plan: reps of every working set across the history; here 8 occurs twice and 6 once.
    let no_plan = |day: &str, sets: &[(f64, u32)]| {
        let sets: Vec<(f64, u32, Option<u32>)> = sets.iter().map(|&(weight, reps_done)| (weight, reps_done, None)).collect();
        session(day, &sets)
    };
    assert_eq!(chart_reps(&[no_plan("2026-10-01", &[(100.0, 8), (100.0, 8), (100.0, 6)])]), 8);
    // Only working sets count: the light warm-up reps are not in the vote.
    assert_eq!(chart_reps(&[no_plan("2026-10-01", &[(40.0, 15), (40.0, 15), (100.0, 6)])]), 6);
    // A tie goes to the larger number.
    assert_eq!(chart_reps(&[no_plan("2026-10-01", &[(100.0, 6)]), no_plan("2026-10-08", &[(100.0, 10)])]), 10);
    // Sets without reps do not vote; with none at all the answer is 0.
    assert_eq!(chart_reps(&[no_plan("2026-10-01", &[(100.0, 0), (100.0, 0), (100.0, 7)])]), 7);
    assert_eq!(chart_reps(&[no_plan("2026-10-01", &[(100.0, 0)])]), 0);
    assert_eq!(chart_reps(&[]), 0);
}

/// `(month, grams, reps, session)` of each point, for a short comparison.
fn flat(points: &[MonthPoint]) -> Vec<(String, i64, u16, usize)> {
    points.iter().map(|point| (point.month.format("%Y-%m-%d").to_string(), point.weight.grams(), point.reps, point.session)).collect()
}

fn row(month: &str, weight: f64, reps: u16, session: usize) -> (String, i64, u16, usize) {
    (month.to_string(), kg(weight).grams(), reps, session)
}

#[test]
fn series_rules() {
    let sessions = vec![
        // May: a set of 90 x 10 before the heavier 100 x 6.
        planned("2026-05-25", &[(40.0, 10), (80.0, 10), (90.0, 10), (100.0, 6)]),
        // June: no set reaches 10 reps, so only the heaviest set is marked.
        planned("2026-06-20", &[(95.0, 6), (100.0, 4)]),
        // July: a lighter set, and one as heavy as the point, are not "heavier", so neither is marked.
        planned("2026-07-15", &[(90.0, 10), (90.0, 6), (85.0, 6)]),
        // August: two sessions in one month give one point, the heaviest.
        planned("2026-08-03", &[(90.0, 10)]),
        planned("2026-08-20", &[(92.5, 10)]),
    ];
    let series = series(&sessions, at("2026-08-25", 12));
    assert_eq!(series.mode, SeriesMode::Weight { reps: 10 });
    assert_eq!(flat(&series.points), vec![row("2026-05-01", 90.0, 10, 0), row("2026-07-01", 90.0, 10, 2), row("2026-08-01", 92.5, 10, 4)]);
    assert_eq!(flat(&series.heavier), vec![row("2026-05-01", 100.0, 6, 0), row("2026-06-01", 100.0, 4, 1)]);
    assert!(series.gaps.is_empty());

    // The point remembers which set it came from.
    let may = series.points.first().unwrap();
    assert_eq!((may.set.activity, may.set.set), (activity(date("2026-05-25")), SetRef::Recorded(2)));
    let circle = series.heavier.first().unwrap();
    assert_eq!((circle.set.activity, circle.set.set), (activity(date("2026-05-25")), SetRef::Recorded(3)));
}

#[test]
fn series_picks_more_reps_at_the_same_weight() {
    let sessions = vec![planned("2026-05-05", &[(90.0, 10), (90.0, 12)]), planned("2026-06-05", &[(100.0, 5), (100.0, 7)])];
    let series = series(&sessions, at("2026-06-10", 12));
    assert_eq!(flat(&series.points), vec![row("2026-05-01", 90.0, 12, 0)]);
    assert_eq!(flat(&series.heavier), vec![row("2026-06-01", 100.0, 7, 1)]);
}

#[test]
fn series_keeps_the_earlier_of_two_equal_sets() {
    let sessions = vec![planned("2026-05-05", &[(90.0, 10)]), planned("2026-05-20", &[(90.0, 10)])];
    let series = series(&sessions, at("2026-05-25", 12));
    assert_eq!(flat(&series.points), vec![row("2026-05-01", 90.0, 10, 0)]);
}

#[test]
fn series_follows_the_target_set_for_the_whole_history() {
    // The target is 8 (the latest plan), so 90 x 8 of the earlier month is already a point.
    let sessions = vec![session("2026-05-05", &[(90.0, 8, Some(10))]), session("2026-06-05", &[(90.0, 8, Some(8))])];
    let series = series(&sessions, at("2026-06-10", 12));
    assert_eq!(series.mode, SeriesMode::Weight { reps: 8 });
    assert_eq!(flat(&series.points), vec![row("2026-05-01", 90.0, 8, 0), row("2026-06-01", 90.0, 8, 1)]);
    assert!(series.heavier.is_empty());
}

#[test]
fn series_without_a_plan_uses_the_most_common_reps() {
    let sessions = vec![session("2026-05-05", &[(100.0, 8, None), (100.0, 8, None), (100.0, 6, None)])];
    let series = series(&sessions, at("2026-05-10", 12));
    assert_eq!(series.mode, SeriesMode::Weight { reps: 8 });
    assert_eq!(flat(&series.points), vec![row("2026-05-01", 100.0, 8, 0)]);
}

#[test]
fn series_ignores_sets_without_weight_in_weight_mode() {
    let sessions = vec![
        // The set without weight has more reps but is not a point and not a circle.
        planned("2026-05-05", &[(0.0, 15), (60.0, 10)]),
        // A month where only the sets without weight were done has no point at all.
        planned("2026-06-05", &[(0.0, 12)]),
    ];
    let series = series(&sessions, at("2026-06-10", 12));
    assert_eq!(series.mode, SeriesMode::Weight { reps: 10 });
    assert_eq!(flat(&series.points), vec![row("2026-05-01", 60.0, 10, 0)]);
    assert!(series.heavier.is_empty());
}

#[test]
fn series_of_an_exercise_without_weight_shows_the_reps() {
    let sessions = vec![
        // The reps of a session are those of its weakest set: 10, 8 and then 12.
        planned("2026-05-05", &[(0.0, 12), (0.0, 10)]),
        planned("2026-05-12", &[(0.0, 15), (0.0, 8)]),
        planned("2026-05-20", &[(0.0, 14), (0.0, 12)]),
        planned("2026-06-05", &[(0.0, 9)]),
    ];
    let series = series(&sessions, at("2026-06-10", 12));
    assert_eq!(series.mode, SeriesMode::Reps);
    assert_eq!(flat(&series.points), vec![row("2026-05-01", 0.0, 12, 2), row("2026-06-01", 0.0, 9, 3)]);
    assert!(series.heavier.is_empty());
    let may = series.points.first().unwrap();
    assert_eq!((may.set.activity, may.set.set), (activity(date("2026-05-20")), SetRef::Recorded(1)));
}

#[test]
fn one_session_with_a_weight_keeps_the_weight_mode() {
    // The exercise had a weight once: the chart is of weight, and the sessions without it give no point.
    let sessions = vec![planned("2026-05-05", &[(75.0, 10)]), planned("2026-06-05", &[(0.0, 10)])];
    let series = series(&sessions, at("2026-06-10", 12));
    assert_eq!(series.mode, SeriesMode::Weight { reps: 10 });
    assert_eq!(flat(&series.points), vec![row("2026-05-01", 75.0, 10, 0)]);
}

#[test]
fn series_without_sessions_is_empty() {
    let series = series(&[], today());
    assert_eq!(series.mode, SeriesMode::Weight { reps: 0 });
    assert!(series.points.is_empty() && series.heavier.is_empty() && series.gaps.is_empty());
}

#[test]
fn series_marks_the_breaks() {
    let sessions = vec![planned("2026-03-01", &[(80.0, 10)]), planned("2026-05-06", &[(80.0, 10)])];
    // 66 days between the sessions; the second one is 4 days old.
    assert_eq!(series(&sessions, at("2026-05-10", 12)).gaps, vec![Gap { from: 0, to: Some(1), days: 66 }]);
    // The last session was 40 days ago: the running break is a gap that leads to today.
    assert_eq!(series(&sessions, at("2026-06-15", 12)).gaps, vec![Gap { from: 0, to: Some(1), days: 66 }, Gap { from: 1, to: None, days: 40 }]);
    // 28 days is not a break, between sessions or up to today.
    let sessions = vec![planned("2026-05-01", &[(80.0, 10)]), planned("2026-05-29", &[(80.0, 10)])];
    assert!(series(&sessions, at("2026-06-26", 12)).gaps.is_empty());
    assert_eq!(series(&sessions, at("2026-06-27", 12)).gaps, vec![Gap { from: 1, to: None, days: 29 }]);
}

#[test]
fn month_is_the_local_day_month() {
    // 22:30 UTC on 31 October is already 1 November where the workout took place.
    let mut late = planned("2026-11-01", &[(80.0, 10)]);
    late.start = Utc.with_ymd_and_hms(2026, 10, 31, 22, 30, 0).unwrap();
    let series = series(&[late], at("2026-11-05", 12));
    assert_eq!(flat(&series.points), vec![row("2026-11-01", 80.0, 10, 0)]);
}
