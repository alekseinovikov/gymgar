//! Reads the expiry of a Garmin access token (a JWT) so the token can be cached until it runs low.
//!
//! The signature is not checked: the token comes straight from Garmin over TLS and is only ever sent back to Garmin,
//! so we trust it and need nothing from it except `exp`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::GarminError;

#[derive(Deserialize)]
struct Claims {
    exp: i64,
}

/// The `exp` claim of `token` as a point in time.
///
/// Error texts describe what is wrong with the token but never quote it, because the token is a secret.
pub fn expires_at(token: &str) -> Result<DateTime<Utc>, GarminError> {
    let mut parts = token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(unexpected("the token is not a JWT"));
    };
    // The no-padding decoder rejects a trailing `=`, but some encoders add it; both spellings of the same bytes are accepted.
    let json = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).map_err(|_| unexpected("the token payload is not valid base64url"))?;
    let claims: Claims = serde_json::from_slice(&json).map_err(|error| {
        // Not `error.to_string()`: serde_json quotes the offending value, which comes from the token.
        if error.is_data() { unexpected("the token payload has no numeric exp claim") } else { unexpected("the token payload is not valid JSON") }
    })?;
    DateTime::from_timestamp(claims.exp, 0).ok_or_else(|| unexpected("the token exp claim is out of range"))
}

fn unexpected(reason: &str) -> GarminError {
    GarminError::UnexpectedResponse(reason.to_owned())
}

/// `header.payload.signature` in base64url with only `exp` in the payload that matters; for tests that need a token Garmin would issue.
#[cfg(test)]
pub(crate) fn fake_jwt(exp: i64) -> String {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"iss":"garmin","exp":{exp}}}"#));
    format!("{header}.{payload}.c2lnbmF0dXJl")
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};

    use super::*;

    fn token_with_payload(payload: &[u8]) -> String {
        format!("{}.{}.c2ln", URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#), URL_SAFE_NO_PAD.encode(payload))
    }

    fn unexpected(result: Result<DateTime<Utc>, GarminError>) -> String {
        match result {
            Err(GarminError::UnexpectedResponse(reason)) => reason,
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
    }

    #[test]
    fn reads_exp_from_the_payload() {
        let token = token_with_payload(br#"{"iss":"garmin","exp":1900000000}"#);
        assert_eq!(expires_at(&token).unwrap(), DateTime::from_timestamp(1_900_000_000, 0).unwrap());
    }

    #[test]
    fn tolerates_padding_in_the_payload() {
        // `{"exp":12}` is 10 bytes, so the padded encoding ends in `==`.
        let padded = URL_SAFE.encode(br#"{"exp":12}"#);
        assert!(padded.ends_with("=="), "test premise: {padded}");
        let token = format!("h.{padded}.s");
        assert_eq!(expires_at(&token).unwrap(), DateTime::from_timestamp(12, 0).unwrap());
    }

    #[test]
    fn reads_a_payload_that_needs_url_safe_characters() {
        // Characters 62 and 63 of the alphabet are `-` and `_` in base64url; `>>>` and `???` encode to them.
        let payload = br#"{"note":">>>???~~~","exp":1900000000}"#;
        let encoded = URL_SAFE_NO_PAD.encode(payload);
        assert!(encoded.contains('-') || encoded.contains('_'), "test premise: {encoded}");
        let token = format!("h.{encoded}.s");
        assert_eq!(expires_at(&token).unwrap(), DateTime::from_timestamp(1_900_000_000, 0).unwrap());
    }

    #[test]
    fn rejects_a_token_that_is_not_a_jwt() {
        for token in ["", "not-a-jwt", "only.two", "a.b.c.d"] {
            let reason = unexpected(expires_at(token));
            assert!(reason.contains("JWT"), "{token:?}: {reason}");
        }
    }

    #[test]
    fn rejects_a_payload_that_is_not_base64url() {
        assert!(unexpected(expires_at("h.%%%.s")).contains("base64url"));
    }

    #[test]
    fn rejects_a_payload_that_is_not_json() {
        assert!(unexpected(expires_at(&token_with_payload(b"not json"))).contains("JSON"));
    }

    #[test]
    fn rejects_a_payload_without_a_numeric_exp() {
        for payload in [&br#"{"iss":"garmin"}"#[..], br#"{"exp":"tomorrow"}"#, br#"{"exp":null}"#, b"[]"] {
            let reason = unexpected(expires_at(&token_with_payload(payload)));
            assert!(reason.contains("exp"), "{}: {reason}", String::from_utf8_lossy(payload));
        }
    }

    #[test]
    fn rejects_an_exp_outside_the_calendar() {
        let reason = unexpected(expires_at(&token_with_payload(format!(r#"{{"exp":{}}}"#, i64::MAX).as_bytes())));
        assert!(reason.contains("range"), "{reason}");
    }

    #[test]
    fn error_text_does_not_echo_the_token() {
        let token = token_with_payload(br#"{"exp":"s3cret-looking-value"}"#);
        let reason = unexpected(expires_at(&token));
        assert!(!reason.contains("s3cret-looking-value") && !reason.contains(&token), "{reason}");
    }
}
