//! Decoding a `.FIT` file into what a strength workout needs: sets, exercise titles, workout steps, the workout's name and heart
//! rate. The DTOs keep FIT's vocabulary (category and exercise numbers, message indexes, kilograms as the file stores them); turning
//! them into domain types is `gymgar-sync`'s job.
//!
//! The file is hostile input. fitparser checks the structure and both CRCs, but it also slices with sizes read from the file and
//! can panic on a crafted one, so decoding runs under `catch_unwind` and a panic becomes [`FitError::Fit`]. Messages are decoded one
//! at a time and only the ones listed above are kept, each kind up to a count far above any real workout ([`LIMITS`]); a file past
//! one is [`FitError::TooLarge`]. A kept message takes more memory than its bytes in the file, so the cap on the file's size alone
//! would not bound what decoding holds; the counts do.

use chrono::{DateTime, Utc};
use fitparser::de::{FitObject, FitStreamProcessor};
use fitparser::profile::MesgNum;
use fitparser::profile::field_types::{ExerciseCategory, MessageIndex};
use fitparser::{ErrorKind, FitDataField, FitDataRecord, Value};

use super::FitError;

/// The messages of an activity file that matter for strength training, each list in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FitActivity {
    /// `workout.wkt_name`: the workout the watch followed, if it followed one.
    pub workout_name: Option<String>,
    /// Every `set` message that has a `message_index`: active sets, rests and anything else.
    pub sets: Vec<FitSet>,
    /// `exercise_title`: what the watch called each exercise of the workout, in the watch's language.
    pub titles: Vec<FitTitle>,
    /// `workout_step`: the plan.
    pub steps: Vec<FitStep>,
    /// `record` messages that carry both a time and a heart rate (beats per minute).
    pub heart_rate: Vec<(DateTime<Utc>, u8)>,
}

/// One `set` message. A field the file leaves out (or marks invalid) is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct FitSet {
    /// The set's key within the file: it counts active sets and rests together (0, 1, 2...).
    pub message_index: u16,
    pub kind: FitSetKind,
    pub start_time: Option<DateTime<Utc>>,
    pub duration_s: Option<f64>,
    pub repetitions: Option<u16>,
    /// Kilograms, whatever unit the watch displayed. 0 is a set without added weight, not a missing value.
    pub weight_kg: Option<f64>,
    /// `exercise_category` number; 65534 is FIT's "unknown".
    pub category: Option<u16>,
    /// The exercise's number within its category.
    pub category_subtype: Option<u16>,
    /// `message_index` of the workout step this set belongs to.
    pub wkt_step_index: Option<u16>,
}

/// `set_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitSetKind {
    Active,
    Rest,
    /// Missing, or a value the profile does not name.
    Other,
}

/// One `exercise_title` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitTitle {
    pub category: Option<u16>,
    pub name: Option<u16>,
    /// `wkt_step_name`; empty if the message has none.
    pub title: String,
}

/// One `workout_step` message that has a `message_index`.
#[derive(Debug, Clone, PartialEq)]
pub struct FitStep {
    pub message_index: u16,
    /// Planned reps, present only on steps measured in reps.
    pub duration_reps: Option<u32>,
    /// Planned weight in kilograms.
    pub exercise_weight_kg: Option<f64>,
    pub category: Option<u16>,
    pub name: Option<u16>,
    pub notes: Option<String>,
}

/// How many messages of each kept kind one file may hold.
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// `set` messages, rests included. A long workout has a few hundred.
    sets: usize,
    /// Heart-rate readings. A day-long activity recorded every second has 86 400.
    heart_rate: usize,
    /// `workout_step` messages, and `exercise_title` messages, each. A workout on the watch has at most a few hundred steps.
    plan: usize,
}

/// The limits of [`parse`].
const LIMITS: Limits = Limits { sets: 2_000, heart_rate: 200_000, plan: 2_000 };

/// Decodes `fit`. A file without `set` messages is fine (a manual activity, another app's recording); a truncated, damaged or
/// foreign one is [`FitError::Fit`], one with more sets, heart-rate readings or workout steps than [`LIMITS`] allows is
/// [`FitError::TooLarge`].
pub fn parse(fit: &[u8]) -> Result<FitActivity, FitError> {
    parse_within(fit, LIMITS)
}

fn parse_within(fit: &[u8], limits: Limits) -> Result<FitActivity, FitError> {
    if fit.is_empty() {
        return Err(FitError::Fit("the file is empty".to_owned()));
    }
    // fitparser panics on some crafted files (it slices the header with the size the header claims); the rest of the app must not.
    std::panic::catch_unwind(|| decode(fit, limits)).unwrap_or_else(|_| Err(FitError::Fit("the FIT decoder failed on malformed data".to_owned())))
}

fn decode(mut input: &[u8], limits: Limits) -> Result<FitActivity, FitError> {
    let mut processor = FitStreamProcessor::new();
    let mut activity = FitActivity::default();
    // A file is complete once the CRC that closes it has been read and checked; input that stops anywhere else is truncated, even
    // right after a whole message (fitparser's own `from_bytes` accepts that).
    let mut complete = false;
    while !input.is_empty() {
        let (rest, object) = processor.deserialize_next(input).map_err(|error| decoder_error(&error))?;
        if rest.len() >= input.len() {
            return Err(FitError::Fit("the decoder stopped making progress".to_owned()));
        }
        input = rest;
        complete = matches!(object, FitObject::Crc(_));
        match object {
            // Chained files follow each other; each starts over with its own header and definitions.
            FitObject::Crc(_) => processor.reset(),
            // Every data message is decoded, kept or not: a compressed timestamp counts from the last full one.
            FitObject::DataMessage(message) => activity.take(processor.decode_message(message).map_err(|error| decoder_error(&error))?, limits)?,
            FitObject::Header(_) | FitObject::DefinitionMessage(_) => {}
        }
    }
    if complete { Ok(activity) } else { Err(FitError::Fit("the file ends before its last CRC".to_owned())) }
}

/// fitparser's error as text. A value error may quote a field's value, which can be the user's text: it is left out.
fn decoder_error(error: &ErrorKind) -> FitError {
    match error {
        ErrorKind::ValueError(_) => FitError::Fit("a field value cannot be decoded".to_owned()),
        other => FitError::Fit(other.to_string()),
    }
}

impl FitActivity {
    /// Keeps `record` if it is one of the messages this module reads; [`FitError::TooLarge`] when that would take a list past
    /// its limit.
    fn take(&mut self, record: FitDataRecord, limits: Limits) -> Result<(), FitError> {
        match record.kind() {
            MesgNum::Set => match set(&record) {
                Some(set) => push_within(&mut self.sets, set, limits.sets)?,
                None => log::debug!("skipping a FIT set message without message_index"),
            },
            MesgNum::ExerciseTitle => {
                let title = FitTitle {
                    category: field(&record, "exercise_category").and_then(category),
                    name: field(&record, "exercise_name").and_then(uint16),
                    title: field(&record, "wkt_step_name").and_then(text).unwrap_or_default(),
                };
                push_within(&mut self.titles, title, limits.plan)?;
            }
            MesgNum::WorkoutStep => match step(&record) {
                Some(step) => push_within(&mut self.steps, step, limits.plan)?,
                None => log::debug!("skipping a FIT workout step without message_index"),
            },
            MesgNum::Workout if self.workout_name.is_none() => {
                self.workout_name = field(&record, "wkt_name").and_then(text).filter(|name| !name.is_empty());
            }
            MesgNum::Record => {
                if let (Some(at), Some(bpm)) = (field(&record, "timestamp").and_then(time), field(&record, "heart_rate").and_then(uint8)) {
                    push_within(&mut self.heart_rate, (at, bpm), limits.heart_rate)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Adds `item` to `list` unless the list already holds `limit` items.
fn push_within<T>(list: &mut Vec<T>, item: T, limit: usize) -> Result<(), FitError> {
    if list.len() >= limit {
        return Err(FitError::TooLarge);
    }
    list.push(item);
    Ok(())
}

fn set(record: &FitDataRecord) -> Option<FitSet> {
    Some(FitSet {
        message_index: field(record, "message_index").and_then(message_index)?,
        kind: match field(record, "set_type") {
            Some(Value::String(kind)) if kind == "active" => FitSetKind::Active,
            Some(Value::String(kind)) if kind == "rest" => FitSetKind::Rest,
            _ => FitSetKind::Other,
        },
        start_time: field(record, "start_time").and_then(time),
        duration_s: field(record, "duration").and_then(float),
        repetitions: field(record, "repetitions").and_then(uint16),
        weight_kg: field(record, "weight").and_then(float),
        category: field(record, "category").and_then(category),
        category_subtype: field(record, "category_subtype").and_then(uint16),
        wkt_step_index: field(record, "wkt_step_index").and_then(message_index),
    })
}

fn step(record: &FitDataRecord) -> Option<FitStep> {
    Some(FitStep {
        message_index: field(record, "message_index").and_then(message_index)?,
        duration_reps: field(record, "duration_reps").and_then(uint32),
        exercise_weight_kg: field(record, "exercise_weight").and_then(float),
        category: field(record, "exercise_category").and_then(category),
        name: field(record, "exercise_name").and_then(uint16),
        notes: field(record, "notes").and_then(text),
    })
}

/// The profile field called `name`. fitparser puts the profile's fields before developer fields, so a developer field with the
/// same name cannot shadow it.
fn field<'a>(record: &'a FitDataRecord, name: &str) -> Option<&'a Value> {
    record.fields().iter().find(|field| field.name() == name).map(FitDataField::value)
}

/// An integer as fitparser hands it over: any integer variant, or the first element of an array (watches write `set.category` as
/// three equal values). Floats, strings, times and FIT's "invalid" are `None`.
fn integer(value: &Value) -> Option<i64> {
    match value {
        Value::Byte(n) | Value::Enum(n) | Value::UInt8(n) | Value::UInt8z(n) => Some(i64::from(*n)),
        Value::SInt8(n) => Some(i64::from(*n)),
        Value::SInt16(n) => Some(i64::from(*n)),
        Value::UInt16(n) | Value::UInt16z(n) => Some(i64::from(*n)),
        Value::SInt32(n) => Some(i64::from(*n)),
        Value::UInt32(n) | Value::UInt32z(n) => Some(i64::from(*n)),
        Value::SInt64(n) => Some(*n),
        Value::UInt64(n) | Value::UInt64z(n) => i64::try_from(*n).ok(),
        Value::Array(values) => values.first().and_then(integer),
        Value::Timestamp(_) | Value::String(_) | Value::Float32(_) | Value::Float64(_) | Value::Invalid => None,
    }
}

/// FIT marks a missing value with the type's largest number (`0xFF`, `0xFFFF`, ...): that is `None`, not a value.
fn uint8(value: &Value) -> Option<u8> {
    integer(value).and_then(|n| u8::try_from(n).ok()).filter(|&n| n != u8::MAX)
}

fn uint16(value: &Value) -> Option<u16> {
    integer(value).and_then(|n| u16::try_from(n).ok()).filter(|&n| n != u16::MAX)
}

fn uint32(value: &Value) -> Option<u32> {
    integer(value).and_then(|n| u32::try_from(n).ok()).filter(|&n| n != u32::MAX)
}

/// A `u16` enum field: fitparser turns the values its profile names into those names ("deadlift", "unknown") and passes the rest
/// as numbers. `from_name` maps a name back, `None` for one it does not know.
fn named_uint16(value: &Value, from_name: fn(&str) -> Option<u16>) -> Option<u16> {
    match value {
        Value::String(name) => from_name(name).filter(|&n| n != u16::MAX),
        Value::Array(values) => values.first().and_then(|first| named_uint16(first, from_name)),
        other => uint16(other),
    }
}

fn category(value: &Value) -> Option<u16> {
    named_uint16(value, |name| match ExerciseCategory::from(name) {
        // fitparser maps a name it does not know to `Value(0)`, which is a real category (bench press).
        ExerciseCategory::Value(_) => None,
        known => Some(known.as_u16()),
    })
}

fn message_index(value: &Value) -> Option<u16> {
    named_uint16(value, |name| match MessageIndex::from(name) {
        MessageIndex::Value(_) => None,
        known => Some(known.as_u16()),
    })
}

/// A scaled field (fitparser divides by the scale and hands over a float). Only finite values.
fn float(value: &Value) -> Option<f64> {
    match value {
        Value::Float64(n) => Some(*n),
        Value::Float32(n) => Some(f64::from(*n)),
        Value::Array(values) => values.first().and_then(float),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// fitparser reads FIT time as the machine's local time; the instant is what matters, kept in UTC.
fn time(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::Timestamp(at) => Some(at.with_timezone(&Utc)),
        _ => None,
    }
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::fit::testing::{FitWriter, SetSpec, StepSpec};

    fn start() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 17, 5, 9).unwrap()
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        start() + Duration::seconds(seconds)
    }

    /// An active set with no exercise, reps or weight; tests fill in what they need.
    fn bare_set(message_index: u16, seconds: i64) -> SetSpec {
        SetSpec {
            message_index,
            active: true,
            start: at(seconds),
            duration_s: 30.0,
            reps: None,
            weight_kg: None,
            category: None,
            subtype: None,
            step_index: None,
            pounds: false,
        }
    }

    fn fit_error(result: Result<FitActivity, FitError>) -> String {
        match result {
            Err(FitError::Fit(reason)) => reason,
            other => panic!("expected FitError::Fit, got {other:?}"),
        }
    }

    #[test]
    fn parse_round_trip() {
        let mut writer = FitWriter::new(start());
        writer.workout("FB2");
        writer.step(StepSpec {
            message_index: 0,
            reps: Some(10),
            weight_kg: Some(62.5),
            category: Some(8),
            name: Some(23),
            notes: Some("Спина прямая".into()),
        });
        writer.step(StepSpec { message_index: 1, category: Some(31), ..StepSpec::default() });
        writer.title(Some(8), Some(23), "Румынская становая тяга");
        writer.title(Some(31), None, "Разминка");
        // A category the profile does not name: fitparser passes it as a number, not a name.
        writer.title(Some(900), Some(3), "Future exercise");
        writer.set(SetSpec { reps: Some(10), weight_kg: Some(25.0), category: Some(8), subtype: Some(23), step_index: Some(0), ..bare_set(0, 0) });
        writer.set(SetSpec { active: false, duration_s: 31.765, ..bare_set(1, 30) });
        writer.set(SetSpec { duration_s: 300.0, category: Some(31), step_index: Some(1), ..bare_set(2, 62) });
        writer.set(SetSpec { reps: Some(8), weight_kg: Some(0.0), category: Some(900), subtype: Some(3), ..bare_set(3, 400) });
        writer.heart_rate(at(0), 95);
        writer.heart_rate(at(2), 101);
        writer.heart_rate(at(4), 110);

        let parsed = parse(&writer.finish()).unwrap();

        let set = |message_index, kind, seconds, duration_s| FitSet {
            message_index,
            kind,
            start_time: Some(at(seconds)),
            duration_s: Some(duration_s),
            repetitions: None,
            weight_kg: None,
            category: None,
            category_subtype: None,
            wkt_step_index: None,
        };
        let expected = FitActivity {
            workout_name: Some("FB2".into()),
            sets: vec![
                FitSet {
                    repetitions: Some(10),
                    weight_kg: Some(25.0),
                    category: Some(8),
                    category_subtype: Some(23),
                    wkt_step_index: Some(0),
                    ..set(0, FitSetKind::Active, 0, 30.0)
                },
                set(1, FitSetKind::Rest, 30, 31.765),
                FitSet { category: Some(31), wkt_step_index: Some(1), ..set(2, FitSetKind::Active, 62, 300.0) },
                FitSet { repetitions: Some(8), weight_kg: Some(0.0), category: Some(900), category_subtype: Some(3), ..set(3, FitSetKind::Active, 400, 30.0) },
            ],
            titles: vec![
                FitTitle { category: Some(8), name: Some(23), title: "Румынская становая тяга".into() },
                FitTitle { category: Some(31), name: None, title: "Разминка".into() },
                FitTitle { category: Some(900), name: Some(3), title: "Future exercise".into() },
            ],
            steps: vec![
                FitStep {
                    message_index: 0,
                    duration_reps: Some(10),
                    exercise_weight_kg: Some(62.5),
                    category: Some(8),
                    name: Some(23),
                    notes: Some("Спина прямая".into()),
                },
                FitStep { message_index: 1, duration_reps: None, exercise_weight_kg: None, category: Some(31), name: None, notes: None },
            ],
            heart_rate: vec![(at(0), 95), (at(2), 101), (at(4), 110)],
        };
        assert_eq!(parsed, expected);
    }

    #[test]
    fn parse_no_exercise_encodings() {
        let mut writer = FitWriter::new(start());
        writer.set(bare_set(0, 0));
        writer.set(SetSpec { category: Some(65534), ..bare_set(1, 40) });
        writer.set(SetSpec { category: Some(0xFFFF), subtype: Some(0xFFFF), ..bare_set(2, 80) });
        // In a title the profile names 65534 ("unknown"): the name maps back to the number.
        writer.title(Some(65534), None, "Без упражнения");
        writer.title(Some(0xFFFF), Some(0xFFFF), "Invalid");

        let parsed = parse(&writer.finish()).unwrap();

        let categories: Vec<_> = parsed.sets.iter().map(|set| (set.category, set.category_subtype)).collect();
        assert_eq!(categories, [(None, None), (Some(65534), None), (None, None)]);
        let titles: Vec<_> = parsed.titles.iter().map(|title| (title.category, title.name)).collect();
        assert_eq!(titles, [(Some(65534), None), (None, None)]);
    }

    #[test]
    fn parse_pound_display_keeps_kg() {
        let mut writer = FitWriter::new(start());
        writer.set(SetSpec { reps: Some(5), weight_kg: Some(20.0), pounds: true, ..bare_set(0, 0) });
        let file = writer.finish();

        // The file really says "pound"...
        let records = fitparser::from_bytes(&file).unwrap();
        let set = records.iter().find(|record| record.kind() == MesgNum::Set).unwrap();
        let unit = set.fields().iter().find(|field| field.name() == "weight_display_unit").map(|field| field.value());
        assert_eq!(unit, Some(&Value::String("pound".into())));

        // ...and the weight stays in kilograms: FIT stores kilograms whatever the watch shows.
        assert_eq!(parse(&file).unwrap().sets[0].weight_kg, Some(20.0));
    }

    #[test]
    fn parse_no_sets_is_ok() {
        let mut writer = FitWriter::new(start());
        writer.workout("Manual");
        writer.heart_rate(at(0), 90);

        let parsed = parse(&writer.finish()).unwrap();

        assert!(parsed.sets.is_empty());
        assert_eq!(parsed.workout_name.as_deref(), Some("Manual"));
        assert_eq!(parsed.heart_rate, [(at(0), 90)]);
    }

    #[test]
    fn parse_skips_a_set_without_message_index() {
        let mut writer = FitWriter::new(start());
        // FIT's invalid value: fitparser drops the field, so the set arrives without an index.
        writer.set(SetSpec { message_index: 0xFFFF, ..bare_set(0, 0) });
        writer.set(bare_set(1, 40));

        let indexes: Vec<_> = parse(&writer.finish()).unwrap().sets.iter().map(|set| set.message_index).collect();
        assert_eq!(indexes, [1]);
    }

    #[test]
    fn parse_truncated_or_garbage_is_error() {
        let mut writer = FitWriter::new(start());
        writer.workout("FB2");
        writer.set(SetSpec { reps: Some(10), weight_kg: Some(25.0), category: Some(8), subtype: Some(23), ..bare_set(0, 0) });
        writer.set(SetSpec { active: false, ..bare_set(1, 30) });
        writer.heart_rate(at(0), 95);
        let file = writer.finish();
        assert!(parse(&file).is_ok());

        // Every prefix, half of the file among them; a cut right after a whole message must not pass for a complete file.
        for len in 0..file.len() {
            fit_error(parse(&file[..len]));
        }

        // 1 KiB of noise (a fixed LCG, so the test is reproducible).
        let mut state = 0x2545_f491_u32;
        let garbage: Vec<u8> = (0..1024)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        fit_error(parse(&garbage));
    }

    /// A file of `count` sets, every other one a rest.
    fn file_with_sets(count: usize) -> Vec<u8> {
        let mut writer = FitWriter::new(start());
        for index in 0..count {
            let seconds = i64::try_from(index).unwrap() * 40;
            writer.set(SetSpec { active: index % 2 == 0, ..bare_set(u16::try_from(index).unwrap(), seconds) });
        }
        writer.finish()
    }

    /// A file of `count` heart-rate readings, one a second.
    fn file_with_heart_rate(count: usize) -> Vec<u8> {
        let mut writer = FitWriter::new(start());
        for second in 0..count {
            writer.heart_rate(at(i64::try_from(second).unwrap()), 120);
        }
        writer.finish()
    }

    #[test]
    fn parse_refuses_more_sets_than_a_workout_has() {
        // Rests count too: each is a set message kept in memory.
        assert_eq!(parse(&file_with_sets(2_000)).map(|activity| activity.sets.len()), Ok(2_000));
        assert_eq!(parse(&file_with_sets(2_001)).map(|activity| activity.sets.len()), Err(FitError::TooLarge));
    }

    #[test]
    fn parse_refuses_more_heart_rate_than_a_day_has() {
        // A day-long activity recorded every second fits with room to spare.
        assert_eq!(parse(&file_with_heart_rate(200_000)).map(|activity| activity.heart_rate.len()), Ok(200_000));
        assert_eq!(parse(&file_with_heart_rate(200_001)).map(|activity| activity.heart_rate.len()), Err(FitError::TooLarge));
    }

    #[test]
    fn parse_refuses_more_steps_or_titles_than_a_workout_has() {
        let limits = Limits { plan: 2, ..LIMITS };
        let steps = |count: u16| {
            let mut writer = FitWriter::new(start());
            for index in 0..count {
                writer.step(StepSpec { message_index: index, ..StepSpec::default() });
            }
            writer.finish()
        };
        let titles = |count: u16| {
            let mut writer = FitWriter::new(start());
            for index in 0..count {
                writer.title(Some(8), Some(index), "Title");
            }
            writer.finish()
        };
        assert_eq!(parse_within(&steps(2), limits).map(|activity| activity.steps.len()), Ok(2));
        assert_eq!(parse_within(&steps(3), limits).map(|activity| activity.steps.len()), Err(FitError::TooLarge));
        assert_eq!(parse_within(&titles(2), limits).map(|activity| activity.titles.len()), Ok(2));
        assert_eq!(parse_within(&titles(3), limits).map(|activity| activity.titles.len()), Err(FitError::TooLarge));
        assert_eq!(LIMITS.plan, 2_000);
    }

    #[test]
    fn parse_hostile_header_is_error_not_panic() {
        // A header that claims 255 bytes and carries a non-zero CRC: fitparser slices 253 bytes out of 20 and panics.
        let mut file = vec![255, 0x20, 0, 0, 0, 0, 0, 0, b'.', b'F', b'I', b'T', 0x01, 0x00];
        file.resize(20, 0);
        fit_error(parse(&file));
    }
}
