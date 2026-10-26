//! `cargo xtask scrub-fit` and `cargo xtask scrub-list`: turning a real Garmin download into a fixture that can be committed.
//!
//! The downloads are the user's personal data (name, device, places, GPS), and the repository is public. Both tools therefore keep
//! only what the app reads and build the output anew rather than blanking fields in the original:
//!
//! - `scrub-fit` decodes the `.FIT` and writes it again through the test writer of `gymgar-garmin`: the workout, its steps, the
//!   exercise titles, the sets (rests included, they carry the rest durations) and the heart rate. No device, no user profile, no
//!   position, no message the app does not read; the file inside the archive gets a neutral name, not the activity id.
//! - `scrub-list` takes a page of the activity list: some activities of different types, without the owner, the profile id, the
//!   activity UUID, the location, any coordinates, the device, the privacy setting, the local start time (it gives away the time
//!   zone) or any link; the name replaced, the id a synthetic one that has nothing to do with the real one. Unlike `scrub-fit` it
//!   is a denylist: a personal field Garmin adds later stays until it is named here.
//!
//! Output is meant to be read by a person before it is committed (`fixtures/garmin/README.md`); nothing here replaces that.

use std::path::{Path, PathBuf};

use gymgar_garmin::fit::testing::{FitWriter, SetSpec, StepSpec, zip_single};
use gymgar_garmin::fit::{self, FitActivity, FitSetKind};
use serde_json::{Map, Value};

/// The `activityId` of the first activity of a scrubbed list; the next ones count up from it in output order. The ids are made up
/// rather than derived from the real ones (a shift by a published offset can be undone), and they stay far below any id Garmin
/// hands out, so a real one is easy to tell apart.
pub const FIRST_SYNTHETIC_ID: i64 = 1_000_001;

/// The name of the one file in the scrubbed archive. The real one is `<activity id>_ACTIVITY.fit`.
const SCRUBBED_ENTRY: &str = "activity.fit";

/// Keys dropped from every object of the list, on top of the prefix and suffix rules in [`is_personal_key`]. `startTimeLocal`
/// goes because next to `startTimeGMT` it gives away where the owner lives; the app reads only `startTimeGMT`.
const PERSONAL_KEYS: &[&str] = &["userProfileId", "activityUUID", "locationName", "deviceId", "privacy", "startTimeLocal"];

// ---- scrub-fit ----

/// `cargo xtask scrub-fit <in.zip> <out.zip>`.
pub fn run_fit(args: &[String]) -> Result<(), String> {
    let (input, output) = fit_arguments(args)?;
    let zip = read(&input)?;
    let scrubbed = scrub_fit(&zip)?;
    write(&output, &scrubbed)?;
    eprintln!("scrub-fit: {} bytes in, {} bytes out ({})", zip.len(), scrubbed.len(), output.display());
    Ok(())
}

/// The scrubbed archive for `zip`, a Personal API download.
pub fn scrub_fit(zip: &[u8]) -> Result<Vec<u8>, String> {
    let fit_bytes = fit::unzip_activity(zip).map_err(|error| error.to_string())?;
    let activity = fit::parse(&fit_bytes).map_err(|error| error.to_string())?;
    rewrite(&activity)
}

/// `activity` written as a new FIT file, in a ZIP. A set the file gives no start gets the activity's start (the earliest time in the
/// file) and one without a duration gets 0 s: the writer needs both, and a fixture is not the place to guess better.
///
/// The writer panics on values FIT cannot store, but every value here was just decoded from a FIT field of the same width and scale,
/// so it fits.
fn rewrite(activity: &FitActivity) -> Result<Vec<u8>, String> {
    let start = activity
        .sets
        .iter()
        .filter_map(|set| set.start_time)
        .chain(activity.heart_rate.iter().map(|(at, _)| *at))
        .min()
        .ok_or("the activity has no set or heart rate time to date the file by")?;
    let mut writer = FitWriter::new(start);
    if let Some(name) = &activity.workout_name {
        writer.workout(name);
    }
    for step in &activity.steps {
        writer.step(StepSpec {
            message_index: step.message_index,
            reps: step.duration_reps,
            weight_kg: step.exercise_weight_kg,
            category: step.category,
            name: step.name,
            notes: step.notes.clone(),
        });
    }
    for title in &activity.titles {
        writer.title(title.category, title.name, &title.title);
    }
    for set in &activity.sets {
        let active = match set.kind {
            FitSetKind::Active => true,
            FitSetKind::Rest => false,
            FitSetKind::Other => return Err(format!("set {} has a set_type the writer cannot say (neither active nor rest)", set.message_index)),
        };
        writer.set(SetSpec {
            message_index: set.message_index,
            active,
            start: set.start_time.unwrap_or(start),
            duration_s: set.duration_s.unwrap_or(0.0),
            reps: set.repetitions,
            weight_kg: set.weight_kg,
            category: set.category,
            subtype: set.category_subtype,
            step_index: set.wkt_step_index,
            // Only how the watch displayed the weight; the file stores kilograms either way.
            pounds: false,
        });
    }
    for (at, bpm) in &activity.heart_rate {
        writer.heart_rate(*at, *bpm);
    }
    Ok(zip_single(SCRUBBED_ENTRY, &writer.finish()))
}

fn fit_arguments(args: &[String]) -> Result<(PathBuf, PathBuf), String> {
    match args {
        [input, output] => Ok((PathBuf::from(input), PathBuf::from(output))),
        _ => Err("usage: cargo xtask scrub-fit <in.zip> <out.zip>".to_owned()),
    }
}

// ---- scrub-list ----

/// `cargo xtask scrub-list <in.json> <out.json> --take <n>`.
pub fn run_list(args: &[String]) -> Result<(), String> {
    let (input, output, take) = list_arguments(args)?;
    let list: Value = serde_json::from_slice(&read(&input)?).map_err(|error| format!("{} is not JSON: {error}", input.display()))?;
    let scrubbed = scrub_list(list, take)?;
    let mut text = serde_json::to_string_pretty(&scrubbed).map_err(|error| error.to_string())?;
    text.push('\n');
    write(&output, text.as_bytes())?;
    eprintln!("scrub-list: {} activities written to {}", scrubbed.as_array().map_or(0, Vec::len), output.display());
    Ok(())
}

/// Up to `take` activities of `list` (a JSON array as the Personal API sends it), scrubbed. The pick is one of each type in the
/// order the types first appear, then the second of each, and so on, so a small page still shows every type; the activities keep
/// the list's order.
pub fn scrub_list(list: Value, take: usize) -> Result<Value, String> {
    if take == 0 {
        return Err("--take must be at least 1".to_owned());
    }
    let Value::Array(items) = list else {
        return Err("the activity list must be a JSON array".to_owned());
    };
    let mut picked = pick_varied(&items, take);
    picked.sort_unstable();
    let mut scrubbed = Vec::with_capacity(picked.len());
    for (position, index) in picked.into_iter().enumerate() {
        let mut item = items.get(index).cloned().ok_or("an activity vanished from the list")?;
        let id = i64::try_from(position).ok().and_then(|position| FIRST_SYNTHETIC_ID.checked_add(position)).ok_or("the list is too long to number")?;
        scrub_item(&mut item, position.saturating_add(1), id)?;
        scrubbed.push(item);
    }
    Ok(Value::Array(scrubbed))
}

fn type_key(item: &Value) -> &str {
    item.pointer("/activityType/typeKey").and_then(Value::as_str).unwrap_or("activity")
}

/// Indexes into `items`, at most `take`, round-robin over the activity types.
fn pick_varied(items: &[Value], take: usize) -> Vec<usize> {
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let key = type_key(item);
        match groups.iter_mut().find(|(group, _)| *group == key) {
            Some((_, indexes)) => indexes.push(index),
            None => groups.push((key, vec![index])),
        }
    }
    let mut picked = Vec::new();
    let mut round = 0;
    loop {
        let before = picked.len();
        for (_, indexes) in &groups {
            if let Some(&index) = indexes.get(round) {
                if picked.len() >= take {
                    return picked;
                }
                picked.push(index);
            }
        }
        if picked.len() == before {
            return picked;
        }
        round = round.saturating_add(1);
    }
}

/// One activity of the page, number `number` (from 1) in the output, which gets the made-up id `id`.
fn scrub_item(item: &mut Value, number: usize, id: i64) -> Result<(), String> {
    let name = format!("{} {number}", type_key(item));
    let Value::Object(object) = item else {
        return Err("an activity in the list is not a JSON object".to_owned());
    };
    // Only to refuse what is not an activity; the real id itself goes nowhere.
    object.get("activityId").and_then(Value::as_i64).ok_or("an activity has no integer activityId")?;
    scrub_object(object);
    if let Some(slot) = object.get_mut("activityName") {
        *slot = Value::String(name);
    }
    object.insert("activityId".to_owned(), Value::from(id));
    Ok(())
}

/// Drops the personal keys and every link, at any depth.
fn scrub_object(object: &mut Map<String, Value>) {
    object.retain(|key, value| !is_personal_key(key) && !is_link(value));
    for value in object.values_mut() {
        scrub_value(value);
    }
}

fn scrub_value(value: &mut Value) {
    match value {
        Value::Object(object) => scrub_object(object),
        Value::Array(values) => {
            values.retain(|value| !is_link(value));
            values.iter_mut().for_each(scrub_value);
        }
        _ => {}
    }
}

/// `owner*` (the owner's id, name, display name, profile pictures), the names in [`PERSONAL_KEYS`] (at any depth), and anything
/// that is a coordinate (`startLatitude`, `endLongitude`, ...).
fn is_personal_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.starts_with("owner") || PERSONAL_KEYS.contains(&key) || lower.ends_with("latitude") || lower.ends_with("longitude")
}

/// A string that is a web address, whatever it is called.
fn is_link(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        let lower = text.trim_start().to_ascii_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    })
}

fn list_arguments(args: &[String]) -> Result<(PathBuf, PathBuf, usize), String> {
    const USAGE: &str = "usage: cargo xtask scrub-list <in.json> <out.json> --take <n>";
    let mut paths = Vec::new();
    let mut take = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--take" {
            take = Some(rest.next().and_then(|n| n.parse::<usize>().ok()).filter(|&n| n > 0).ok_or(USAGE)?);
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    match (paths.as_slice(), take) {
        ([input, output], Some(take)) => Ok((input.clone(), output.clone(), take)),
        _ => Err(USAGE.to_owned()),
    }
}

// ---- files ----

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use fitparser::profile::MesgNum;
    use gymgar_garmin::fit::testing::{FitWriter, SetSpec, StepSpec, zip_single};
    use gymgar_garmin::fit::{self, FitActivity, FitSet, FitSetKind};
    use serde_json::json;

    use super::*;

    fn start() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 17, 5, 9).unwrap()
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        start() + Duration::seconds(seconds)
    }

    // ---- scrub-list ----

    fn item(id: i64, type_key: &str, name: &str) -> Value {
        json!({
            "activityId": id,
            "activityName": name,
            "startTimeGMT": "2026-10-22 13:47:01",
            "activityType": {"typeId": 13, "typeKey": type_key, "parentTypeId": 29},
            "ownerId": 1234567,
            "ownerDisplayName": "11111111-2222-4333-8444-555555555555",
            "ownerFullName": "Test Person",
            "ownerProfileImageUrlSmall": "https://images.example.com/profile/x-prth.png",
            "userProfileId": 7,
            "activityUUID": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "locationName": "Testville",
            "startLatitude": 10.0,
            "startLongitude": 20.0,
            "endLatitude": 10.5,
            "endLongitude": 20.5,
            "deviceId": 3_900_000_001_u64,
            "privacy": {"typeId": 4, "typeKey": "groups"},
            "startTimeLocal": "2026-10-22 16:47:01",
            "averageHR": 136.0,
            "summarizedExerciseSets": [{"category": "DEADLIFT", "subCategory": "ROMANIAN_DEADLIFT", "reps": 40, "sets": 4}],
            "links": ["https://example.com/a", "kept"],
            "nested": {"homepage": "HTTP://example.com/b", "note": "kept"},
        })
    }

    fn list() -> Value {
        json!([
            item(100, "strength_training", "FB2"),
            item(99, "strength_training", "Testville Велосипед"),
            item(98, "strength_training", "FB1"),
            item(97, "cycling", "Test Person's ride"),
            item(96, "treadmill_running", "Run"),
            item(95, "strength_training", "FB3"),
        ])
    }

    #[test]
    fn scrub_list_removes_personal_fields() {
        let scrubbed = scrub_list(list(), 4).unwrap();
        let items = scrubbed.as_array().unwrap();

        // One of each type first (in the order the types first appear), then the next of the biggest group; the list's order stays.
        let types: Vec<_> = items.iter().map(|item| item.pointer("/activityType/typeKey").and_then(Value::as_str).unwrap()).collect();
        assert_eq!(types, ["strength_training", "strength_training", "cycling", "treadmill_running"]);
        // Made up and counted in output order: nothing of the real ids (100, 99, 97, 96) is left to recover.
        let ids: Vec<_> = items.iter().map(|item| item["activityId"].as_i64().unwrap()).collect();
        assert_eq!(ids, [1_000_001, 1_000_002, 1_000_003, 1_000_004]);

        let names: Vec<_> = items.iter().map(|item| item["activityName"].as_str().unwrap()).collect();
        assert_eq!(names, ["strength_training 1", "strength_training 2", "cycling 3", "treadmill_running 4"]);

        let text = serde_json::to_string(&scrubbed).unwrap();
        for personal in [
            "Test Person",
            "owner",
            "images.example.com",
            "Testville",
            "11111111",
            "aaaaaaaa",
            "http",
            "HTTP",
            "atitude",
            "ongitude",
            "deviceId",
            "userProfile",
            "privacy",
            "groups",
            "startTimeLocal",
            "16:47:01",
        ] {
            assert!(!text.contains(personal), "{personal} is still in {text}");
        }

        // What the app reads stays, nested values too; only the URLs and the personal keys go.
        let first = items.first().unwrap();
        assert_eq!(first["averageHR"], json!(136.0));
        assert_eq!(first["startTimeGMT"], json!("2026-10-22 13:47:01"));
        assert_eq!(first["activityType"], json!({"typeId": 13, "typeKey": "strength_training", "parentTypeId": 29}));
        assert_eq!(first["summarizedExerciseSets"], json!([{"category": "DEADLIFT", "subCategory": "ROMANIAN_DEADLIFT", "reps": 40, "sets": 4}]));
        assert_eq!(first["links"], json!(["kept"]));
        assert_eq!(first["nested"], json!({"note": "kept"}));
    }

    #[test]
    fn scrub_list_takes_up_to_n_and_rejects_bad_input() {
        // Fewer items than asked for: all of them.
        assert_eq!(scrub_list(list(), 50).unwrap().as_array().unwrap().len(), 6);
        assert_eq!(scrub_list(json!([]), 3).unwrap(), json!([]));
        scrub_list(list(), 0).unwrap_err();
        scrub_list(json!({"activities": []}), 3).unwrap_err();
        scrub_list(json!([1, 2]), 2).unwrap_err();
        // Something without an integer id is not an activity.
        scrub_list(json!([{"activityId": "123456789", "activityType": {"typeKey": "x"}}]), 1).unwrap_err();
        scrub_list(json!([{"activityName": "x"}]), 1).unwrap_err();
        // No type: still named, still given a made-up id; the real one, however large, does not matter.
        let untyped = scrub_list(json!([{"activityId": i64::MAX, "activityName": "Secret"}]), 1).unwrap();
        assert_eq!(untyped, json!([{"activityId": FIRST_SYNTHETIC_ID, "activityName": "activity 1"}]));
    }

    // ---- the committed fixture ----

    /// The page of the list in the repository, as `scrub-list` wrote it.
    const COMMITTED_PAGE: &str = include_str!("../../fixtures/garmin/activities-page.json");

    /// Every object key and every string of `value`, at any depth.
    fn keys_and_strings<'a>(value: &'a Value, keys: &mut Vec<&'a str>, strings: &mut Vec<&'a str>) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    keys.push(key);
                    keys_and_strings(value, keys, strings);
                }
            }
            Value::Array(values) => values.iter().for_each(|value| keys_and_strings(value, keys, strings)),
            Value::String(text) => strings.push(text),
            _ => {}
        }
    }

    /// The repository is public: whatever the scrub does today, the committed page must not hold what leads back to its owner.
    #[test]
    fn committed_activity_page_holds_nothing_personal() {
        let page: Value = serde_json::from_str(COMMITTED_PAGE).unwrap();
        let items = page.as_array().unwrap();
        assert!(!items.is_empty());
        let (mut keys, mut strings) = (Vec::new(), Vec::new());
        keys_and_strings(&page, &mut keys, &mut strings);

        let banned = ["privacy", "starttimelocal", "activityuuid", "userprofileid", "deviceid", "locationname"];
        for key in keys {
            let lower = key.to_ascii_lowercase();
            assert!(!lower.starts_with("owner"), "{key}");
            assert!(!lower.ends_with("latitude") && !lower.ends_with("longitude"), "{key}");
            assert!(!banned.contains(&lower.as_str()), "{key}");
        }
        for text in strings {
            assert!(!text.trim_start().to_ascii_lowercase().starts_with("http"), "a link is left: {text}");
        }
        // Made-up ids only: Garmin's own are in the billions.
        for item in items {
            let id = item["activityId"].as_i64().unwrap();
            assert!((1..10_000_000).contains(&id), "activity id {id} is not a made-up one");
        }
    }

    // ---- scrub-fit ----

    /// FIT's CRC-16, the way the writer computes it.
    fn crc16(bytes: &[u8]) -> u16 {
        bytes.iter().fold(0, |crc, &byte| {
            let mut crc = crc ^ u16::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
            }
            crc
        })
    }

    /// `base` (a finished FIT file) with raw messages appended to its data, headers and CRCs recomputed.
    fn with_extra_messages(base: &[u8], extra: &[u8]) -> Vec<u8> {
        let mut data = base[14..base.len() - 2].to_vec();
        data.extend_from_slice(extra);
        let mut file = vec![14, 0x20];
        file.extend_from_slice(&2100u16.to_le_bytes());
        file.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        file.extend_from_slice(b".FIT");
        file.extend_from_slice(&crc16(&file).to_le_bytes());
        file.extend_from_slice(&data);
        file.extend_from_slice(&crc16(&file).to_le_bytes());
        file
    }

    /// A definition message (local type, message number, `(field number, size, base type)`) and one data message for it.
    fn raw_message(local: u8, mesg_num: u16, fields: &[(u8, u8, u8, &[u8])]) -> Vec<u8> {
        let mut bytes = vec![0x40 | local, 0, 0];
        bytes.extend_from_slice(&mesg_num.to_le_bytes());
        bytes.push(u8::try_from(fields.len()).unwrap());
        for (number, size, base_type, _) in fields {
            bytes.extend_from_slice(&[*number, *size, *base_type]);
        }
        bytes.push(local);
        for (_, _, _, value) in fields {
            bytes.extend_from_slice(value);
        }
        bytes
    }

    /// A strength activity as a watch writes it: the kept messages plus a device, a user profile, a GPS record and a message the
    /// profile does not know.
    fn real_looking_zip() -> Vec<u8> {
        let mut writer = FitWriter::new(start());
        writer.workout("FB2");
        writer.step(StepSpec {
            message_index: 0,
            reps: Some(10),
            weight_kg: Some(25.0),
            category: Some(8),
            name: Some(23),
            notes: Some("Спина прямая".into()),
        });
        writer.step(StepSpec { message_index: 1, category: Some(31), ..StepSpec::default() });
        writer.title(Some(8), Some(23), "Румынская становая тяга");
        writer.title(Some(31), None, "Разминка");
        writer.set(SetSpec {
            message_index: 0,
            active: true,
            start: at(0),
            duration_s: 45.5,
            reps: Some(10),
            weight_kg: Some(25.0),
            category: Some(8),
            subtype: Some(23),
            step_index: Some(0),
            pounds: false,
        });
        writer.set(SetSpec {
            message_index: 1,
            active: false,
            start: at(46),
            duration_s: 31.765,
            reps: None,
            weight_kg: None,
            category: None,
            subtype: None,
            step_index: None,
            pounds: false,
        });
        writer.heart_rate(at(0), 95);
        writer.heart_rate(at(2), 101);
        let base = writer.finish();

        let mut extra = Vec::new();
        // device_info: serial_number (uint32z).
        extra.extend(raw_message(10, 23, &[(3, 4, 0x8C, &3_900_000_001_u32.to_le_bytes())]));
        // user_profile: friendly_name (string).
        extra.extend(raw_message(11, 3, &[(0, 12, 0x07, b"Test Person\0")]));
        // record with a position (semicircles) next to the heart rate.
        let when = u32::try_from(at(4).timestamp() - 631_065_600).unwrap().to_le_bytes();
        extra.extend(raw_message(
            12,
            20,
            &[(253, 4, 0x86, &when), (0, 4, 0x85, &1_000_i32.to_le_bytes()), (1, 4, 0x85, &2_000_i32.to_le_bytes()), (3, 1, 0x02, &[110])],
        ));
        // A message number nothing names (manufacturer-specific range).
        extra.extend(raw_message(13, 0xFF00, &[(0, 1, 0x02, &[7])]));
        zip_single("123456789_ACTIVITY.fit", &with_extra_messages(&base, &extra))
    }

    fn kinds_and_fields(fit_bytes: &[u8]) -> Vec<(MesgNum, Vec<String>)> {
        fitparser::from_bytes(fit_bytes).unwrap().iter().map(|record| (record.kind(), record.fields().iter().map(|f| f.name().to_owned()).collect())).collect()
    }

    #[test]
    fn scrub_fit_drops_device_and_profile() {
        let input = real_looking_zip();
        let input_fit = fit::unzip_activity(&input).unwrap();
        // The input really carries what must go (otherwise this test proves nothing).
        let before = kinds_and_fields(&input_fit);
        for personal in [MesgNum::DeviceInfo, MesgNum::UserProfile] {
            assert!(before.iter().any(|(kind, _)| *kind == personal), "{personal:?}");
        }
        assert!(before.iter().any(|(_, fields)| fields.iter().any(|f| f == "position_lat")));
        assert!(before.iter().any(|(_, fields)| fields.iter().any(|f| f == "serial_number")));
        assert!(before.iter().any(|(kind, _)| *kind == MesgNum::MfgRangeMin), "{before:?}");

        let output = scrub_fit(&input).unwrap();
        let output_fit = fit::unzip_activity(&output).unwrap();

        // Only the messages the app reads, and heart rate records carry nothing but time and heart rate.
        let after = kinds_and_fields(&output_fit);
        let kept = [MesgNum::FileId, MesgNum::Workout, MesgNum::WorkoutStep, MesgNum::ExerciseTitle, MesgNum::Set, MesgNum::Record];
        for (kind, fields) in &after {
            assert!(kept.contains(kind), "{kind:?}");
            if *kind == MesgNum::Record {
                assert!(fields.iter().all(|f| f == "timestamp" || f == "heart_rate"), "{fields:?}");
            }
            // No serial number, no file number: nothing that could lead back to the device or the activity.
            if *kind == MesgNum::FileId {
                assert!(fields.iter().all(|f| f == "type" || f == "manufacturer" || f == "time_created"), "{fields:?}");
            }
            for personal in ["serial_number", "friendly_name", "position_lat", "position_long", "product_name"] {
                assert!(fields.iter().all(|f| f != personal), "{personal}");
            }
        }
        assert!(!output_fit.windows(11).any(|window| window == b"Test Person"));

        // And nothing the app reads was lost: both the active set and the rest, the plan, the titles, the heart rate (the GPS
        // record's heart rate too).
        let parsed = fit::parse(&output_fit).unwrap();
        assert_eq!(parsed, fit::parse(&input_fit).unwrap());
        assert_eq!(parsed.sets.len(), 2);
        assert_eq!(parsed.sets.get(1).map(|set| set.kind), Some(FitSetKind::Rest));
        assert_eq!(parsed.heart_rate.len(), 3);
        assert_eq!(parsed.workout_name.as_deref(), Some("FB2"));

        // The archive holds one anonymous entry, and neither it nor the file carries the activity id of the original's name.
        let entry_names = String::from_utf8_lossy(&output);
        assert!(!entry_names.contains("123456789") && entry_names.contains("activity.fit"));
        assert!(!output_fit.windows(9).any(|window| window == b"123456789"));
        assert!(!output_fit.windows(4).any(|window| window == 123_456_789_u32.to_le_bytes()));
    }

    #[test]
    fn scrub_fit_rejects_what_it_cannot_rewrite() {
        scrub_fit(b"not a zip").unwrap_err();
        scrub_fit(&zip_single("1_ACTIVITY.fit", b"not a fit file")).unwrap_err();
        // A file with nothing to date it by.
        let empty = FitWriter::new(start()).finish();
        scrub_fit(&zip_single("1_ACTIVITY.fit", &empty)).unwrap_err();
    }

    #[test]
    fn rewrite_fills_what_a_set_leaves_out() {
        let set = |message_index, kind, start_time, duration_s| FitSet {
            message_index,
            kind,
            start_time,
            duration_s,
            repetitions: None,
            weight_kg: None,
            category: None,
            category_subtype: None,
            wkt_step_index: None,
        };
        let activity = FitActivity {
            sets: vec![set(0, FitSetKind::Active, Some(at(30)), Some(12.5)), set(1, FitSetKind::Rest, None, None)],
            heart_rate: vec![(at(10), 90)],
            ..FitActivity::default()
        };

        let rewritten = fit::parse(&fit::unzip_activity(&rewrite(&activity).unwrap()).unwrap()).unwrap();

        // A set without a start gets the activity's start (the earliest time in the file), without a duration 0 s.
        assert_eq!(rewritten.sets.get(1).map(|set| (set.start_time, set.duration_s)), Some((Some(at(10)), Some(0.0))));
        assert_eq!(rewritten.sets.first().map(|set| (set.start_time, set.duration_s)), Some((Some(at(30)), Some(12.5))));

        // A set type the writer cannot say is an error, not a set quietly turned into something else.
        let odd = FitActivity { sets: vec![set(0, FitSetKind::Other, Some(at(0)), Some(1.0))], ..FitActivity::default() };
        assert!(rewrite(&odd).unwrap_err().contains("set_type"));
    }

    // ---- command line ----

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn list_command_arguments() {
        let parsed = list_arguments(&args(&["in.json", "out.json", "--take", "8"])).unwrap();
        assert_eq!((parsed.0.to_str(), parsed.1.to_str(), parsed.2), (Some("in.json"), Some("out.json"), 8));
        assert_eq!(list_arguments(&args(&["--take", "3", "a", "b"])).unwrap().2, 3);
        for bad in [
            &["a", "b"][..],
            &["a", "--take", "3"],
            &["a", "b", "c", "--take", "3"],
            &["a", "b", "--take"],
            &["a", "b", "--take", "x"],
            &["a", "b", "--take", "0"],
            &["a", "b", "--take", "-1"],
        ] {
            list_arguments(&args(bad)).unwrap_err();
        }
    }

    #[test]
    fn fit_command_arguments() {
        let parsed = fit_arguments(&args(&["in.zip", "out.zip"])).unwrap();
        assert_eq!((parsed.0.to_str(), parsed.1.to_str()), (Some("in.zip"), Some("out.zip")));
        for bad in [&[][..], &["a"], &["a", "b", "c"]] {
            fit_arguments(&args(bad)).unwrap_err();
        }
    }
}
