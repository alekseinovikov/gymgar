//! UI language setting.
//!
//! Pure data: the choice the user makes in settings. Resolving `System` to a concrete locale and loading the
//! translation catalogs is the UI layer's job, not the domain's.

use serde::{Deserialize, Serialize};

/// Interface language. `System` follows the operating system locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    System,
    En,
    Ru,
}

impl Language {
    /// Stable machine name, used for storage and in serialized form.
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::System => "system",
            Language::En => "en",
            Language::Ru => "ru",
        }
    }

    /// Inverse of [`Language::as_str`]; `None` for anything else.
    pub fn parse(s: &str) -> Option<Language> {
        match s {
            "system" => Some(Language::System),
            "en" => Some(Language::En),
            "ru" => Some(Language::Ru),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Language; 3] = [Language::System, Language::En, Language::Ru];

    #[test]
    fn language_round_trip() {
        for language in ALL {
            assert_eq!(Language::parse(language.as_str()), Some(language));
        }
        assert_eq!(Language::parse("de"), None);
        assert_eq!(Language::parse("EN"), None);
    }

    #[test]
    fn serializes_as_lowercase_strings() {
        for language in ALL {
            let json = serde_json::to_string(&language).unwrap();
            assert_eq!(json, format!("\"{}\"", language.as_str()));
            assert_eq!(serde_json::from_str::<Language>(&json).unwrap(), language);
        }
        assert!(serde_json::from_str::<Language>("\"de\"").is_err());
    }
}
