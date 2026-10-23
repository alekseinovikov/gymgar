//! Settings commands: `settings.get` and `settings.setLanguage`.
//!
//! The settings live in the database only; the session keeps no copy, so a language an agent sets is what the
//! next `settings.get` (and the UI's next refresh) reads.

use gymgar_domain::Language;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::{always, done};
use crate::params::{self, NoParams};
use crate::{CommandKind, CommandSpec, EngineError, Outcome, Session};

const GET: &str = "settings.get";
const SET_LANGUAGE: &str = "settings.setLanguage";

/// The languages offered, in the order the UI lists them.
const LANGUAGES: [Language; 3] = [Language::System, Language::En, Language::Ru];

/// The answer of `settings.get` and `settings.setLanguage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub language: Language,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SetLanguageParams {
    /// `system` follows the operating system language (English unless it is Russian), `en` is English, `ru` Russian.
    #[schemars(schema_with = "language_schema")]
    language: Language,
}

/// `Language` lives in the domain, which knows nothing of JSON Schema; its serialized names are spelled out here.
fn language_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "enum": LANGUAGES.map(|language| language.as_str()) })
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: GET,
            description: "Read the settings: the interface language (system, en or ru).",
            kind: CommandKind::Query,
            agent: true,
            destructive: false,
            params_schema: params::schema::<NoParams>,
            enabled: always,
            run: get,
        },
        CommandSpec {
            id: SET_LANGUAGE,
            description: "Set the interface language: system (follow the operating system), en or ru. An open Gymgar window switches at once.",
            kind: CommandKind::Mutation,
            agent: true,
            destructive: false,
            params_schema: params::schema::<SetLanguageParams>,
            enabled: always,
            run: set_language,
        },
    ]
}

fn get(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    params::parse::<NoParams>(GET, params)?;
    done(GET, &SettingsView { language: session.language()? })
}

fn set_language(session: &mut Session, params: Value) -> Result<Outcome, EngineError> {
    let SetLanguageParams { language } = params::parse(SET_LANGUAGE, params)?;
    session.store().set_language(language)?;
    log::info!("interface language set to {}", language.as_str());
    done(SET_LANGUAGE, &SettingsView { language })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_session;
    use crate::{Caller, commands};
    use serde_json::json;

    #[test]
    fn bad_params_are_errors() {
        let mut session = test_session();
        for params in [json!({"language": "de"}), json!({"lang": "ru"}), json!(42), json!(["ru"]), json!({"language": "ru", "extra": 1}), Value::Null] {
            for caller in [Caller::User, Caller::Agent] {
                match session.execute("settings.setLanguage", params.clone(), caller) {
                    Err(EngineError::BadParams { cmd, .. }) => assert_eq!(cmd, "settings.setLanguage"),
                    other => panic!("{params}: expected BadParams, got {other:?}"),
                }
            }
        }
        // Nothing was written.
        assert_eq!(session.language(), Ok(Language::System));
        assert!(matches!(session.execute("settings.get", json!({"x": 1}), Caller::User), Err(EngineError::BadParams { .. })));
    }

    #[test]
    fn set_language_persists() {
        let mut session = test_session();
        assert_eq!(session.execute("settings.get", Value::Null, Caller::User), Ok(Outcome::Done(json!({"language": "system"}))));
        for (caller, language) in [(Caller::User, "ru"), (Caller::Agent, "en"), (Caller::User, "system"), (Caller::Agent, "ru")] {
            let answer = session.execute("settings.setLanguage", json!({"language": language}), caller);
            assert_eq!(answer, Ok(Outcome::Done(json!({"language": language}))));
            assert_eq!(session.execute("settings.get", json!({}), Caller::Agent), Ok(Outcome::Done(json!({"language": language}))));
        }
        assert_eq!(session.language(), Ok(Language::Ru));
    }

    #[test]
    fn settings_view_round_trips() {
        let view = SettingsView { language: Language::En };
        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json, json!({"language": "en"}));
        assert_eq!(serde_json::from_value::<SettingsView>(json).unwrap(), view);
    }

    #[test]
    fn set_language_schema_lists_the_languages() {
        let spec = commands::find("settings.setLanguage").unwrap();
        let schema = (spec.params_schema)();
        assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
        assert_eq!(schema.get("required"), Some(&json!(["language"])));
        let language = schema.get("properties").and_then(|p| p.get("language")).unwrap();
        assert_eq!(language.get("type"), Some(&json!("string")));
        let values: Vec<&str> = language.get("enum").and_then(Value::as_array).unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(values, ["system", "en", "ru"]);
        for value in values {
            assert!(Language::parse(value).is_some(), "{value}");
        }
    }
}
