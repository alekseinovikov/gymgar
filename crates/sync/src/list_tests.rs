use chrono::{TimeZone, Utc};
use serde_json::{Value, json};

use super::*;

const PAGE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/garmin/activities-page.json"));

fn parse(value: &Value) -> Result<Vec<ListedActivity>, SyncError> {
    parse_page(value.to_string().as_bytes()).map(|page| page.items)
}

/// A list element with only what the mapping needs; tests overwrite what they probe.
fn element(id: i64) -> Value {
    json!({
        "activityId": id,
        "activityName": "Morning run",
        "activityType": { "typeId": 1, "typeKey": "running" },
        "startTimeGMT": "2026-10-20 07:30:00",
        "duration": 1800.0,
        "distance": 5000.0,
        "averageHR": 150.0,
        "calories": 400.0,
        "manualActivity": false
    })
}

fn with(mut element: Value, key: &str, value: Value) -> Value {
    if let Some(object) = element.as_object_mut() {
        object.insert(key.to_owned(), value);
    }
    element
}

fn without(mut element: Value, key: &str) -> Value {
    if let Some(object) = element.as_object_mut() {
        object.remove(key);
    }
    element
}

#[test]
fn page_fixture() {
    let ListPage { items: page, raw_count } = parse_page(PAGE).unwrap();

    assert_eq!((page.len(), raw_count), (8, 8));
    let source: Vec<Value> = serde_json::from_slice(PAGE).unwrap();
    for (listed, element) in page.iter().zip(&source) {
        // The raw JSON is the element itself, with the same id the summary carries.
        let raw: Value = serde_json::from_str(&listed.raw_json).unwrap();
        assert_eq!(&raw, element);
        assert_eq!(raw.get("activityId").and_then(Value::as_i64), Some(listed.summary.id.0));
    }

    let strength = page.iter().find(|listed| listed.summary.is_strength()).unwrap();
    let summary = &strength.summary;
    assert_eq!(summary.id.0, 1_000_001);
    assert_eq!(summary.type_key, "strength_training");
    assert_eq!(summary.name.as_deref(), Some("strength_training 1"));
    assert_eq!(summary.start, Utc.with_ymd_and_hms(2026, 10, 22, 13, 47, 1).unwrap());
    // 5940.52783203125 s, rounded to the millisecond.
    assert_eq!(summary.duration_ms, Some(5_940_528));
    assert_eq!(summary.avg_hr, Some(136));
    assert_eq!(summary.calories, Some(700));
    assert!(!summary.manual);
    assert_eq!(page.iter().filter(|listed| listed.summary.is_strength()).count(), 2);

    let cycling = page.iter().find(|listed| listed.summary.type_key == "cycling").unwrap();
    assert!(cycling.summary.distance_m.is_some_and(|meters| meters > 0.0));
    assert!(!cycling.summary.is_strength());
    assert_eq!(cycling.summary.calories, Some(677));

    // The page keeps Garmin's order (newest first).
    assert!(page.windows(2).all(|pair| pair[0].summary.start > pair[1].summary.start));
}

#[test]
fn page_odd_values() {
    let odd = json!({
        "activityId": 1,
        "activityType": { "typeKey": "running" },
        "startTimeGMT": "2026-10-20 07:30:00",
        "activityName": "   ",
        "duration": -1,
        "distance": -5.0,
        "averageHR": 1e9,
        "calories": -3
    });
    let no_time = without(element(2), "startTimeGMT");
    let bad_time = with(element(3), "startTimeGMT", json!("yesterday"));
    let null_time = with(element(4), "startTimeGMT", Value::Null);
    let fine = element(5);

    let page = parse(&json!([odd, no_time, bad_time, null_time, fine])).unwrap();

    // The elements without a readable start are skipped, the rest of the page is kept.
    let ids: Vec<i64> = page.iter().map(|listed| listed.summary.id.0).collect();
    assert_eq!(ids, [1, 5]);
    let summary = &page.first().unwrap().summary;
    assert_eq!(summary.name, None);
    assert_eq!(summary.duration_ms, None);
    assert_eq!(summary.distance_m, None);
    assert_eq!(summary.avg_hr, None);
    assert_eq!(summary.calories, None);
    assert!(!summary.manual);
    assert_eq!(page.get(1).unwrap().summary.name.as_deref(), Some("Morning run"));
}

#[test]
fn page_numbers_outside_their_range_are_absent() {
    let cases = [
        ("duration", json!(1e30)),
        ("duration", json!(-0.5)),
        ("distance", json!(-1)),
        ("averageHR", json!(65_536)),
        ("averageHR", json!(-1)),
        ("calories", json!(5_000_000_000_i64)),
        ("calories", json!(-1)),
    ];
    for (key, value) in cases {
        let page = parse(&json!([with(element(1), key, value.clone())])).unwrap();
        let summary = &page.first().unwrap().summary;
        let present = match key {
            "duration" => summary.duration_ms.is_some(),
            "distance" => summary.distance_m.is_some(),
            "averageHR" => summary.avg_hr.is_some(),
            _ => summary.calories.is_some(),
        };
        assert!(!present, "{key} = {value}");
    }
    // Zero and the edges of the ranges are values.
    let edge = with(with(with(element(1), "averageHR", json!(65_535)), "calories", json!(4_294_967_295_u32)), "duration", json!(0));
    let summary = parse(&json!([edge])).unwrap().remove(0).summary;
    assert_eq!((summary.avg_hr, summary.calories, summary.duration_ms), (Some(65_535), Some(4_294_967_295), Some(0)));
    // Garmin sends fractions; they round.
    let fractions = with(with(element(1), "averageHR", json!(136.5)), "calories", json!(699.4));
    let summary = parse(&json!([fractions])).unwrap().remove(0).summary;
    assert_eq!((summary.avg_hr, summary.calories), (Some(137), Some(699)));
}

#[test]
fn page_skips_an_element_that_is_not_an_activity() {
    let body = json!([
        element(1),
        // No type, a text id, not an object at all, a null: each is skipped, none fails the page.
        without(element(2), "activityType"),
        with(element(3), "activityId", json!("3")),
        json!("text"),
        Value::Null,
        json!({ "activityName": "no id" }),
        element(6)
    ]);
    let page = parse_page(body.to_string().as_bytes()).unwrap();

    let ids: Vec<i64> = page.items.iter().map(|listed| listed.summary.id.0).collect();
    assert_eq!(ids, [1, 6]);
    assert_eq!(page.raw_count, 7, "skipped elements still count: the next page starts after them");
}

#[test]
fn only_an_empty_array_has_no_elements() {
    let empty = parse_page(b"[]").unwrap();
    assert_eq!((empty.items.len(), empty.raw_count), (0, 0));
    // A page of elements none of which can be read is not the end of the list.
    let unreadable = parse_page(json!([Value::Null, json!("text"), json!({ "activityId": 5 })]).to_string().as_bytes()).unwrap();
    assert_eq!((unreadable.items.len(), unreadable.raw_count), (0, 3));
}

#[test]
fn page_skips_a_start_the_database_cannot_hold() {
    // Chrono reads these years; a stored time has four digits, so the store would refuse the whole page for one of them.
    let body = json!([
        with(element(1), "startTimeGMT", json!("+10000-01-01 00:00:00")),
        with(element(2), "startTimeGMT", json!("-0001-12-31 23:59:59")),
        with(element(3), "startTimeGMT", json!("0000-01-01 00:00:00")),
        with(element(4), "startTimeGMT", json!("9999-12-31 23:59:59")),
        element(5)
    ]);

    let page = parse_page(body.to_string().as_bytes()).unwrap();

    let ids: Vec<i64> = page.items.iter().map(|listed| listed.summary.id.0).collect();
    assert_eq!(ids, [3, 4, 5]);
    assert_eq!(page.raw_count, 5);
}

#[test]
fn page_manual_flag_reads_either_field() {
    let by_manual_activity = with(element(1), "manualActivity", json!(true));
    let by_is_manual = with(without(element(2), "manualActivity"), "isManualActivity", json!(true));
    let neither = without(element(3), "manualActivity");
    let both_false = with(with(element(4), "manualActivity", json!(false)), "isManualActivity", json!(false));

    let page = parse(&json!([by_manual_activity, by_is_manual, neither, both_false])).unwrap();

    let flags: Vec<bool> = page.iter().map(|listed| listed.summary.manual).collect();
    assert_eq!(flags, [true, true, false, false]);
}

#[test]
fn page_raw_json_is_the_element_itself() {
    // `null` fields, an unknown field and a whole-number float are what a typed round trip would change.
    let source = with(with(with(element(1), "distance", Value::Null), "newField", json!({ "a": [1, 2, { "b": null }] })), "maxHR", json!(172.0));

    let page = parse(&json!([source.clone()])).unwrap();

    let listed = page.first().unwrap();
    assert_eq!(serde_json::from_str::<Value>(&listed.raw_json).unwrap(), source);
    // A null distance is no distance.
    assert_eq!(listed.summary.distance_m, None);
}

#[test]
fn page_blank_name_is_no_name() {
    let names = [(json!(""), None), (json!(" \t\n"), None), (Value::Null, None), (json!("Leg day"), Some("Leg day"))];
    for (name, expected) in names {
        let page = parse(&json!([with(element(1), "activityName", name.clone())])).unwrap();
        assert_eq!(page.first().unwrap().summary.name.as_deref(), expected, "{name}");
    }
    let page = parse(&json!([without(element(1), "activityName")])).unwrap();
    assert_eq!(page.first().unwrap().summary.name, None);
}

#[test]
fn page_that_is_not_a_list_is_an_error() {
    let bodies: [&[u8]; 5] = [b"", b"not json", br#"{"activityId": 1}"#, b"\"text\"", br#"[{"activityId": 1"#];
    for body in bodies {
        assert!(matches!(parse_page(body), Err(SyncError::Garmin(_))), "{}", String::from_utf8_lossy(body));
    }
}

#[test]
fn errors_do_not_quote_the_page() {
    let body = br#"{"activityName": "Secret name"}"#;
    let message = parse_page(body).unwrap_err().to_string();
    assert!(!message.contains("Secret"), "{message}");
    let message = parse_page(br#"["Secret name", {"activityName": "Secret name"}"#).unwrap_err().to_string();
    assert!(!message.contains("Secret"), "{message}");
}
