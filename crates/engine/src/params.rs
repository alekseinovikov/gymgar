//! Typed command params: parsing untrusted JSON into a command's params struct, and its JSON Schema.
//!
//! Params arrive from the UI, the CLI and agents alike, so a bad value is always an [`EngineError::BadParams`],
//! never a panic. Params structs use `#[serde(deny_unknown_fields, rename_all = "camelCase")]`, so a mistyped key is
//! an error instead of being silently ignored.

use schemars::{JsonSchema, Schema};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::EngineError;

/// No params: the command takes none, so only `{}` (or `null`) is accepted.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NoParams {}

/// Parses the params of command `cmd`. `null` means "no params" and is read as `{}`; anything that is not an
/// object is rejected (serde would otherwise accept a struct written as an array).
pub fn parse<P: DeserializeOwned>(cmd: &str, v: Value) -> Result<P, EngineError> {
    let object = match v {
        Value::Null => Value::Object(Map::new()),
        Value::Object(_) => v,
        other => return Err(bad_params(cmd, format!("params must be a JSON object, got {}", json_type(&other)))),
    };
    serde_json::from_value(object).map_err(|error| bad_params(cmd, error.to_string()))
}

/// The JSON Schema of params struct `P`, for [`crate::CommandSpec::params_schema`].
pub fn schema<P: JsonSchema>() -> Schema {
    schemars::schema_for!(P)
}

fn bad_params(cmd: &str, msg: String) -> EngineError {
    EngineError::BadParams { cmd: cmd.to_owned(), msg }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct Sample {
        some_field: u8,
    }

    #[test]
    fn null_is_an_empty_object() {
        assert!(parse::<NoParams>("x.y", Value::Null).is_ok());
        assert!(parse::<NoParams>("x.y", json!({})).is_ok());
    }

    #[test]
    fn non_objects_are_bad_params() {
        for v in [json!(42), json!("a"), json!(true), json!([]), json!([7])] {
            match parse::<Sample>("x.y", v.clone()) {
                Err(EngineError::BadParams { cmd, msg }) => {
                    assert_eq!(cmd, "x.y");
                    assert!(msg.contains("must be a JSON object"), "{v}: {msg}");
                }
                other => panic!("{v}: expected BadParams, got {other:?}"),
            }
        }
    }

    #[test]
    fn fields_are_camel_case_and_strict() {
        assert_eq!(parse::<Sample>("x.y", json!({"someField": 3})).unwrap(), Sample { some_field: 3 });
        for v in [json!({"some_field": 3}), json!({"someField": 3, "extra": 1}), json!({"someField": 300}), json!({})] {
            assert!(matches!(parse::<Sample>("x.y", v.clone()), Err(EngineError::BadParams { .. })), "{v}");
        }
        assert!(matches!(parse::<NoParams>("x.y", json!({"extra": 1})), Err(EngineError::BadParams { .. })));
    }

    #[test]
    fn no_params_schema_is_a_closed_object() {
        let schema = schema::<NoParams>();
        assert_eq!(schema.get("type"), Some(&json!("object")));
        assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
    }
}
