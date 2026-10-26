//! One element of Garmin's activity list, and reading a page of that list.
//!
//! The DTOs mirror Garmin's JSON and keep every field they do not know (`extra`), so the element serializes back to what Garmin
//! sent and a field added later is not lost. They carry Garmin's own vocabulary: turning it into `gymgar-domain` types is the job of
//! `gymgar-sync`, not of this crate. The list is hostile input like any other body: a page that is not what we expect is an error,
//! never a panic, and the error text never quotes the page (activity names are the user's data).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::GarminError;

/// What Garmin calls the kind of an activity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTypeDto {
    /// Stable machine name, such as `strength_training` or `running`.
    pub type_key: String,
    /// Everything else Garmin sends about the type (`typeId`, `parentTypeId`, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One element of the activity list (`activities/search/activities`).
///
/// Only what the import needs is typed; `None` stands for a field that is absent or `null` (Garmin sends both), and an absent
/// field stays absent when the element is serialized. `manualActivity` and `isManualActivity` both appear in real responses; the first
/// is typed, the second stays in `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityListItem {
    pub activity_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_name: Option<String>,
    pub activity_type: ActivityTypeDto,
    /// Start in UTC as Garmin writes it ("2026-10-20 17:05:09"); turning it into a point in time is `gymgar-sync`'s job.
    #[serde(default, rename = "startTimeGMT", skip_serializing_if = "Option::is_none")]
    pub start_time_gmt: Option<String>,
    /// Seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Meters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    #[serde(default, rename = "averageHR", skip_serializing_if = "Option::is_none")]
    pub average_hr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calories: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_activity: Option<bool>,
    /// Every field not typed above, verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Reads the body of one list request: a JSON array of activities. Anything else (an object, a truncated or non-JSON body, an
/// element without an id or a type) is [`GarminError::UnexpectedResponse`].
pub fn parse_activity_page(body: &[u8]) -> Result<Vec<ActivityListItem>, GarminError> {
    serde_json::from_slice(body).map_err(|error| {
        // Not `error.to_string()`: serde_json quotes the offending value, and that is the user's data.
        let reason = if error.is_data() {
            format!("the activity list has an unexpected shape (line {}, column {})", error.line(), error.column())
        } else {
            "the activity list is not valid JSON".to_owned()
        };
        GarminError::UnexpectedResponse(reason)
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn parse(value: &Value) -> Result<Vec<ActivityListItem>, GarminError> {
        parse_activity_page(value.to_string().as_bytes())
    }

    fn unexpected(result: Result<Vec<ActivityListItem>, GarminError>) -> String {
        match result {
            Err(GarminError::UnexpectedResponse(reason)) => reason,
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
    }

    /// A list element with every field the client reads, plus ones it does not know. Non-integral numbers, so they survive a round trip unchanged.
    fn full_item() -> Value {
        json!({
            "activityId": 9_000_000_001_i64,
            "activityName": "Evening strength",
            "activityType": { "typeId": 13, "typeKey": "strength_training", "parentTypeId": 17, "sortOrder": 6 },
            "startTimeGMT": "2026-10-20 17:05:09",
            "startTimeLocal": "2026-10-20 19:05:09",
            "duration": 3412.5,
            "distance": 0.5,
            "averageHR": 101.5,
            "calories": 412.5,
            "manualActivity": false,
            "isManualActivity": false,
            "summarizedExerciseSets": [{ "category": "BENCH_PRESS", "sets": 3, "volume": 1800.5 }],
            "newField": 1
        })
    }

    #[test]
    fn list_item_reads_the_known_fields() {
        let items = parse(&json!([full_item()])).unwrap();

        let item = items.first().unwrap();
        assert_eq!(item.activity_id, 9_000_000_001);
        assert_eq!(item.activity_name.as_deref(), Some("Evening strength"));
        assert_eq!(item.activity_type.type_key, "strength_training");
        assert_eq!(item.start_time_gmt.as_deref(), Some("2026-10-20 17:05:09"));
        assert_eq!(item.duration, Some(3412.5));
        assert_eq!(item.distance, Some(0.5));
        assert_eq!(item.average_hr, Some(101.5));
        assert_eq!(item.calories, Some(412.5));
        assert_eq!(item.manual_activity, Some(false));
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn list_item_keeps_unknown_fields() {
        let source = full_item();

        let items = parse(&json!([source.clone()])).unwrap();

        let item = items.first().unwrap();
        assert_eq!(item.extra.get("summarizedExerciseSets"), source.get("summarizedExerciseSets"));
        assert_eq!(item.extra.get("newField"), Some(&json!(1)));
        // Known fields are not duplicated into `extra`, but `isManualActivity` is not one of them.
        assert!(!item.extra.contains_key("activityId") && !item.extra.contains_key("manualActivity"));
        assert_eq!(item.extra.get("isManualActivity"), Some(&json!(false)));
        assert_eq!(serde_json::to_value(item).unwrap(), source);
    }

    #[test]
    fn activity_type_keeps_unknown_fields() {
        let items = parse(&json!([full_item()])).unwrap();

        let activity_type = &items.first().unwrap().activity_type;
        assert_eq!(activity_type.extra.get("typeId"), Some(&json!(13)));
        assert_eq!(activity_type.extra.get("parentTypeId"), Some(&json!(17)));
        assert!(!activity_type.extra.contains_key("typeKey"));
    }

    #[test]
    fn list_item_nulls() {
        let items = parse(&json!([{
            "activityId": 7,
            "activityName": null,
            "activityType": { "typeKey": "running" },
            "startTimeGMT": null,
            "duration": null,
            "averageHR": null,
            "calories": null,
            "manualActivity": null
        }]))
        .unwrap();

        let item = items.first().unwrap();
        assert_eq!(item.activity_id, 7);
        assert_eq!(item.activity_name, None);
        assert_eq!(item.start_time_gmt, None);
        assert_eq!(item.duration, None);
        assert_eq!(item.distance, None);
        assert_eq!(item.average_hr, None);
        assert_eq!(item.calories, None);
        assert_eq!(item.manual_activity, None);
    }

    #[test]
    fn absent_optional_fields_are_not_invented_when_serializing() {
        let source = json!({ "activityId": 7, "activityType": { "typeKey": "running" } });

        let items = parse(&json!([source.clone()])).unwrap();

        assert_eq!(serde_json::to_value(items.first().unwrap()).unwrap(), source);
    }

    #[test]
    fn whole_numbers_are_accepted_for_measurements() {
        let items =
            parse(&json!([{ "activityId": 7, "activityType": { "typeKey": "running" }, "duration": 3600, "calories": 400, "averageHR": 120 }])).unwrap();

        let item = items.first().unwrap();
        assert_eq!((item.duration, item.calories, item.average_hr), (Some(3600.0), Some(400.0), Some(120.0)));
    }

    #[test]
    fn empty_page_is_an_empty_list() {
        assert!(parse(&json!([])).unwrap().is_empty());
    }

    #[test]
    fn page_keeps_the_order_of_garmin() {
        let page = json!([
            { "activityId": 3, "activityType": { "typeKey": "running" } },
            { "activityId": 1, "activityType": { "typeKey": "strength_training" } },
            { "activityId": 2, "activityType": { "typeKey": "walking" } }
        ]);

        let ids: Vec<i64> = parse(&page).unwrap().iter().map(|item| item.activity_id).collect();

        assert_eq!(ids, [3, 1, 2]);
    }

    #[test]
    fn page_not_array_is_unexpected() {
        for body in [json!({ "activities": [] }), json!(null), json!("text"), json!(5)] {
            let reason = unexpected(parse(&body));
            assert!(reason.contains("unexpected shape"), "{reason}");
        }
    }

    #[test]
    fn page_with_broken_json_is_unexpected() {
        for body in [&b""[..], b"[{\"activityId\": 1", b"<html>Service Unavailable</html>", b"\xff\xfe"] {
            let reason = unexpected(parse_activity_page(body));
            assert!(reason.contains("not valid JSON"), "{reason}");
        }
    }

    #[test]
    fn element_without_an_id_or_a_type_is_unexpected() {
        assert!(matches!(parse(&json!([{ "activityType": { "typeKey": "running" } }])), Err(GarminError::UnexpectedResponse(_))));
        assert!(matches!(parse(&json!([{ "activityId": 1 }])), Err(GarminError::UnexpectedResponse(_))));
        assert!(matches!(parse(&json!([{ "activityId": 1, "activityType": {} }])), Err(GarminError::UnexpectedResponse(_))));
        assert!(matches!(parse(&json!([{ "activityId": "1", "activityType": { "typeKey": "running" } }])), Err(GarminError::UnexpectedResponse(_))));
    }

    #[test]
    fn error_text_does_not_echo_the_body() {
        let body = json!([{ "activityId": "Private Morning Run", "activityName": "Secret route", "activityType": { "typeKey": "running" } }]);

        let reason = unexpected(parse(&body));

        assert!(!reason.contains("Private") && !reason.contains("Secret"), "{reason}");
    }
}
