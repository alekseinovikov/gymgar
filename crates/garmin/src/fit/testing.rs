//! A small FIT writer for tests, and a one-file ZIP the way Garmin packs an activity.
//!
//! It writes the messages [`super::parse`] reads, with the FIT profile's message numbers (taken from `fitparser::profile`), field
//! numbers, base types and scales: synthetic files for edge cases, and the scrubbed fixtures, which are rewritten through it so that
//! only these messages survive. Its own tests decode the output with fitparser rather than trusting this encoding.
//!
//! Test code, compiled only for this crate's tests and under the `testing` feature, which apps never enable. A request FIT cannot
//! store (a time before 1990, a weight beyond the field) is a mistake in the test, so it panics instead of returning an error.

use std::collections::HashMap;
use std::io::{Cursor, Write};

use chrono::{DateTime, Utc};
use fitparser::profile::MesgNum;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// FIT counts time in seconds from 1989-12-31 00:00:00 UTC.
const FIT_EPOCH: i64 = 631_065_600;
/// FIT protocol 2.0 and profile 21.00 in the header; fitparser reads neither.
const PROTOCOL_VERSION: u8 = 0x20;
const PROFILE_VERSION: u16 = 2100;
const HEADER_SIZE: u8 = 14;

/// Base types (`fit_base_type`).
const ENUM: u8 = 0x00;
const UINT8: u8 = 0x02;
const STRING: u8 = 0x07;
const UINT16: u8 = 0x84;
const UINT32: u8 = 0x86;

/// Values from the profile's types.
const FILE_ACTIVITY: u8 = 4;
const MANUFACTURER_DEVELOPMENT: u16 = 255;
const DURATION_OPEN: u8 = 5;
const DURATION_REPS: u8 = 29;
const SET_REST: u8 = 0;
const SET_ACTIVE: u8 = 1;
const UNIT_KILOGRAM: u16 = 1;
const UNIT_POUND: u16 = 2;

/// One local message type per kind of message, so a definition is written again only when that kind's set of fields changes.
const LOCAL_FILE_ID: u8 = 0;
const LOCAL_WORKOUT: u8 = 1;
const LOCAL_WORKOUT_STEP: u8 = 2;
const LOCAL_EXERCISE_TITLE: u8 = 3;
const LOCAL_SET: u8 = 4;
const LOCAL_RECORD: u8 = 5;

/// A FIT string field holds at most 255 bytes, its terminating NUL included.
const MAX_STRING_BYTES: usize = 254;

/// One `workout_step` message: a step of the workout the watch followed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepSpec {
    pub message_index: u16,
    /// Planned reps: `duration_type = reps` with this many; `None` writes an open step (`duration_type = open`).
    pub reps: Option<u32>,
    /// Planned weight in kilograms (`exercise_weight`, stored in hundredths).
    pub weight_kg: Option<f64>,
    /// `exercise_category`.
    pub category: Option<u16>,
    /// `exercise_name`: the exercise's number within its category.
    pub name: Option<u16>,
    pub notes: Option<String>,
}

/// One `set` message: an active set or a rest. `None` leaves the field out of the message.
#[derive(Debug, Clone, PartialEq)]
pub struct SetSpec {
    pub message_index: u16,
    /// `set_type`: active, or rest.
    pub active: bool,
    /// `start_time`, whole seconds (FIT has no finer resolution here).
    pub start: DateTime<Utc>,
    /// `duration`, stored in milliseconds.
    pub duration_s: f64,
    pub reps: Option<u16>,
    /// `weight`, always in kilograms (stored in sixteenths), whatever `pounds` says.
    pub weight_kg: Option<f64>,
    /// `category`, written like the watch does: an array of three equal values.
    pub category: Option<u16>,
    /// `category_subtype`, an array of three like `category`.
    pub subtype: Option<u16>,
    /// `wkt_step_index`: the `message_index` of the workout step.
    pub step_index: Option<u16>,
    /// `weight_display_unit`: pounds instead of kilograms. Only how the watch showed the weight.
    pub pounds: bool,
}

/// Builds a FIT activity file message by message, in the order of the calls.
#[derive(Debug, Clone)]
pub struct FitWriter {
    /// Definition and data messages, without the file header and the file CRC.
    records: Vec<u8>,
    /// The definition message last written for each local message type.
    definitions: HashMap<u8, Vec<u8>>,
    /// `message_index` of the next `exercise_title`.
    next_title: u16,
}

impl FitWriter {
    /// A file that starts with `file_id`: an activity, made by "development", created at `start`. No serial number.
    pub fn new(start: DateTime<Utc>) -> Self {
        let mut writer = Self { records: Vec::new(), definitions: HashMap::new(), next_title: 0 };
        writer.message(
            LOCAL_FILE_ID,
            MesgNum::FileId,
            &[Field::new(0, ENUM, vec![FILE_ACTIVITY]), Field::uint16(1, MANUFACTURER_DEVELOPMENT), Field::uint32(4, fit_time(start))],
        );
        writer
    }

    /// A `workout` message with `wkt_name`.
    pub fn workout(&mut self, name: &str) {
        self.message(LOCAL_WORKOUT, MesgNum::Workout, &[Field::string(8, name)]);
    }

    /// A `workout_step` message.
    pub fn step(&mut self, s: StepSpec) {
        let mut fields = vec![Field::uint16(254, s.message_index)];
        match s.reps {
            Some(reps) => {
                fields.push(Field::new(1, ENUM, vec![DURATION_REPS]));
                // `duration_value`, which the profile reads as `duration_reps` because of the duration type.
                fields.push(Field::uint32(2, reps));
            }
            None => fields.push(Field::new(1, ENUM, vec![DURATION_OPEN])),
        }
        if let Some(notes) = &s.notes {
            fields.push(Field::string(8, notes));
        }
        if let Some(category) = s.category {
            fields.push(Field::uint16(10, category));
        }
        if let Some(name) = s.name {
            fields.push(Field::uint16(11, name));
        }
        if let Some(weight) = s.weight_kg {
            fields.push(Field::uint16(12, scaled(weight, 100.0)));
        }
        self.message(LOCAL_WORKOUT_STEP, MesgNum::WorkoutStep, &fields);
    }

    /// An `exercise_title` message; `message_index` counts the titles from 0.
    pub fn title(&mut self, category: Option<u16>, name: Option<u16>, title: &str) {
        let mut fields = vec![Field::uint16(254, self.next_title)];
        if let Some(category) = category {
            fields.push(Field::uint16(0, category));
        }
        if let Some(name) = name {
            fields.push(Field::uint16(1, name));
        }
        fields.push(Field::string(2, title));
        self.message(LOCAL_EXERCISE_TITLE, MesgNum::ExerciseTitle, &fields);
        self.next_title = self.next_title.wrapping_add(1);
    }

    /// A `set` message.
    pub fn set(&mut self, s: SetSpec) {
        let mut fields = vec![Field::uint32(6, fit_time(s.start)), Field::uint32(0, scaled(s.duration_s, 1000.0))];
        if let Some(reps) = s.reps {
            fields.push(Field::uint16(3, reps));
        }
        if let Some(weight) = s.weight_kg {
            fields.push(Field::uint16(4, scaled(weight, 16.0)));
        }
        fields.push(Field::new(5, UINT8, vec![if s.active { SET_ACTIVE } else { SET_REST }]));
        if let Some(category) = s.category {
            fields.push(Field::uint16_triple(7, category));
        }
        if let Some(subtype) = s.subtype {
            fields.push(Field::uint16_triple(8, subtype));
        }
        fields.push(Field::uint16(9, if s.pounds { UNIT_POUND } else { UNIT_KILOGRAM }));
        fields.push(Field::uint16(10, s.message_index));
        if let Some(step) = s.step_index {
            fields.push(Field::uint16(11, step));
        }
        self.message(LOCAL_SET, MesgNum::Set, &fields);
    }

    /// A `record` message with a time and a heart rate.
    pub fn heart_rate(&mut self, at: DateTime<Utc>, bpm: u8) {
        self.message(LOCAL_RECORD, MesgNum::Record, &[Field::uint32(253, fit_time(at)), Field::new(3, UINT8, vec![bpm])]);
    }

    /// The file: a 14-byte header with its CRC, the messages, the file CRC.
    #[allow(clippy::expect_used, reason = "test helper: a test file never reaches 4 GiB")]
    pub fn finish(self) -> Vec<u8> {
        let data_size = u32::try_from(self.records.len()).expect("a test file stays far below 4 GiB");
        let mut file = Vec::with_capacity(usize::from(HEADER_SIZE) + self.records.len() + 2);
        file.push(HEADER_SIZE);
        file.push(PROTOCOL_VERSION);
        file.extend_from_slice(&PROFILE_VERSION.to_le_bytes());
        file.extend_from_slice(&data_size.to_le_bytes());
        file.extend_from_slice(b".FIT");
        let header_crc = crc(&file);
        file.extend_from_slice(&header_crc.to_le_bytes());
        file.extend_from_slice(&self.records);
        // The SDK computes the file CRC over the header too. A header that ends with its own CRC adds nothing to a CRC-16, so this
        // is also the CRC of the messages alone, which is what fitparser checks.
        let file_crc = crc(&file);
        file.extend_from_slice(&file_crc.to_le_bytes());
        file
    }

    /// Writes a definition for `local` if its fields differ from the last one, then the data message.
    #[allow(clippy::expect_used, reason = "test helper: every message here has a handful of fields of at most 255 bytes")]
    fn message(&mut self, local: u8, kind: MesgNum, fields: &[Field]) {
        // Header byte: definition flag plus the local type; reserved byte; architecture 0 = little-endian.
        let mut definition = vec![0x40 | local, 0, 0];
        definition.extend_from_slice(&kind.as_u16().to_le_bytes());
        definition.push(u8::try_from(fields.len()).expect("fewer than 256 fields"));
        for field in fields {
            definition.extend_from_slice(&[field.number, u8::try_from(field.value.len()).expect("fields are at most 255 bytes"), field.base_type]);
        }
        if self.definitions.get(&local) != Some(&definition) {
            self.records.extend_from_slice(&definition);
            self.definitions.insert(local, definition);
        }
        self.records.push(local);
        for field in fields {
            self.records.extend_from_slice(&field.value);
        }
    }
}

/// One field of a message: its number in the profile, its base type and its little-endian bytes.
struct Field {
    number: u8,
    base_type: u8,
    value: Vec<u8>,
}

impl Field {
    fn new(number: u8, base_type: u8, value: Vec<u8>) -> Self {
        Self { number, base_type, value }
    }

    fn uint16(number: u8, value: u16) -> Self {
        Self::new(number, UINT16, value.to_le_bytes().to_vec())
    }

    fn uint32(number: u8, value: u32) -> Self {
        Self::new(number, UINT32, value.to_le_bytes().to_vec())
    }

    /// Three equal `u16`, the way watches write `set.category` and `set.category_subtype`.
    fn uint16_triple(number: u8, value: u16) -> Self {
        Self::new(number, UINT16, value.to_le_bytes().repeat(3))
    }

    /// NUL-terminated UTF-8, cut at a character boundary to fit the field.
    fn string(number: u8, text: &str) -> Self {
        let mut bytes = Vec::new();
        for ch in text.chars() {
            if bytes.len() + ch.len_utf8() > MAX_STRING_BYTES {
                break;
            }
            bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
        }
        bytes.push(0);
        Self::new(number, STRING, bytes)
    }
}

/// `value × scale`, rounded: how FIT stores a scaled field.
#[allow(clippy::expect_used, reason = "test helper: a value the field cannot hold is a mistake in the test")]
fn scaled<T: TryFrom<i64>>(value: f64, scale: f64) -> T {
    let raw = (value * scale).round();
    // `as` turns NaN into 0 and infinities into the bounds: checking finiteness first keeps a NaN from being written as 0.
    raw.is_finite().then(|| T::try_from(raw as i64).ok()).flatten().expect("the value fits the FIT field")
}

/// Seconds since the FIT epoch.
#[allow(clippy::expect_used, reason = "test helper: tests use dates FIT can store")]
fn fit_time(at: DateTime<Utc>) -> u32 {
    u32::try_from(at.timestamp() - FIT_EPOCH).expect("FIT stores times from 1990 to 2126")
}

/// FIT's CRC-16 (reflected polynomial 0xA001, initial value 0), bit by bit: the same values as the SDK's nibble table.
fn crc(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0, |crc, &byte| {
        let mut crc = crc ^ u16::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
        crc
    })
}

/// A ZIP with one deflated entry, like the archive the Personal API sends (`<id>_ACTIVITY.fit`).
#[allow(clippy::expect_used, reason = "test helper: writing a ZIP into memory does not fail")]
pub fn zip_single(file_name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(file_name, SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)).expect("starting an entry in memory does not fail");
    zip.write_all(bytes).expect("writing to memory does not fail");
    zip.finish().expect("finishing a ZIP in memory does not fail").into_inner()
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Local, TimeZone};
    use fitparser::{ErrorKind, FitDataRecord, Value};

    use super::*;

    fn start() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 17, 5, 9).unwrap()
    }

    fn value<'a>(record: &'a FitDataRecord, name: &str) -> Option<&'a Value> {
        record.fields().iter().find(|field| field.name() == name).map(|field| field.value())
    }

    fn of_kind(records: &[FitDataRecord], kind: MesgNum) -> Vec<&FitDataRecord> {
        records.iter().filter(|record| record.kind() == kind).collect()
    }

    /// One active set (10 × 25 kg, deadlift / romanian deadlift), its rest, three heart rate points.
    fn sample() -> Vec<u8> {
        let start = start();
        let mut writer = FitWriter::new(start);
        writer.set(SetSpec {
            message_index: 4,
            active: true,
            start,
            duration_s: 45.5,
            reps: Some(10),
            weight_kg: Some(25.0),
            category: Some(8),
            subtype: Some(23),
            step_index: Some(4),
            pounds: false,
        });
        writer.set(SetSpec {
            message_index: 5,
            active: false,
            start: start + Duration::seconds(46),
            duration_s: 31.765,
            reps: None,
            weight_kg: None,
            category: None,
            subtype: None,
            step_index: None,
            pounds: false,
        });
        for (i, bpm) in [101u8, 102, 103].into_iter().enumerate() {
            writer.heart_rate(start + Duration::seconds(2 * i as i64), bpm);
        }
        writer.finish()
    }

    #[test]
    fn writer_output_parses_with_fitparser() {
        let file = sample();
        // fitparser checks the header CRC and the file CRC by default.
        let records = fitparser::from_bytes(&file).expect("the writer's output is a valid FIT file");

        let sets = of_kind(&records, MesgNum::Set);
        assert_eq!(sets.len(), 2);
        let active = sets[0];
        assert_eq!(value(active, "set_type"), Some(&Value::String("active".into())));
        assert_eq!(value(active, "repetitions"), Some(&Value::UInt16(10)));
        assert_eq!(value(active, "weight"), Some(&Value::Float64(25.0)));
        assert_eq!(value(active, "duration"), Some(&Value::Float64(45.5)));
        assert_eq!(value(active, "category"), Some(&Value::Array(vec![Value::UInt16(8); 3])));
        assert_eq!(value(active, "category_subtype"), Some(&Value::Array(vec![Value::UInt16(23); 3])));
        assert_eq!(value(active, "start_time"), Some(&Value::Timestamp(start().with_timezone(&Local))));
        assert_eq!(value(active, "weight_display_unit"), Some(&Value::String("kilogram".into())));
        // `message_index` is an enum type in the profile: fitparser hands unnamed values over as `SInt64`.
        assert_eq!(value(active, "message_index"), Some(&Value::SInt64(4)));
        assert_eq!(value(active, "wkt_step_index"), Some(&Value::SInt64(4)));

        let rest = sets[1];
        assert_eq!(value(rest, "set_type"), Some(&Value::String("rest".into())));
        assert_eq!(value(rest, "duration"), Some(&Value::Float64(31.765)));
        assert_eq!(value(rest, "message_index"), Some(&Value::SInt64(5)));
        for absent in ["repetitions", "weight", "category", "category_subtype", "wkt_step_index"] {
            assert_eq!(value(rest, absent), None, "{absent}");
        }

        let heart_rate: Vec<_> =
            of_kind(&records, MesgNum::Record).into_iter().map(|record| (value(record, "timestamp").cloned(), value(record, "heart_rate").cloned())).collect();
        let expected: Vec<_> = (0..3)
            .map(|i| {
                let at = (start() + Duration::seconds(2 * i)).with_timezone(&Local);
                (Some(Value::Timestamp(at)), Some(Value::UInt8(101 + i as u8)))
            })
            .collect();
        assert_eq!(heart_rate, expected);
    }

    #[test]
    fn writer_crcs_are_checked() {
        let file = sample();
        // A non-zero header CRC: fitparser skips the check for 0, so this proves it really checked it.
        assert_ne!(&file[12..14], &[0, 0]);

        let mut header = file.clone();
        header[13] ^= 0x01;
        let error = fitparser::from_bytes(&header).unwrap_err();
        assert!(matches!(*error, ErrorKind::InvalidCrc(..)), "{error}");

        // The last record byte is a heart rate: the structure stays valid, only the file CRC no longer matches.
        let mut data = file.clone();
        let last = data.len() - 3;
        data[last] ^= 0x01;
        let error = fitparser::from_bytes(&data).unwrap_err();
        assert!(matches!(*error, ErrorKind::InvalidCrc(..)), "{error}");
    }

    #[test]
    fn writer_writes_no_serial_number() {
        let records = fitparser::from_bytes(&sample()).unwrap();
        let file_ids = of_kind(&records, MesgNum::FileId);
        assert_eq!(file_ids.len(), 1);
        let file_id = file_ids[0];
        assert_eq!(value(file_id, "type"), Some(&Value::String("activity".into())));
        assert_eq!(value(file_id, "time_created"), Some(&Value::Timestamp(start().with_timezone(&Local))));
        assert!(file_id.fields().iter().all(|field| field.name() != "serial_number" && field.number() != 3), "{file_id:?}");
    }

    #[test]
    fn writer_cuts_long_strings_at_a_character_boundary() {
        let mut writer = FitWriter::new(start());
        // 200 two-byte characters: 400 bytes, more than a FIT string field holds.
        writer.workout(&"ж".repeat(200));
        let records = fitparser::from_bytes(&writer.finish()).unwrap();
        let workout = of_kind(&records, MesgNum::Workout)[0];
        assert_eq!(value(workout, "wkt_name"), Some(&Value::String("ж".repeat(127))));
    }
}
