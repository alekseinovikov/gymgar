//! The crate's error type. No variant carries an API key or a token, so any of them can be logged and shown as is.

/// Why a Garmin call failed. The text never contains a key or a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GarminError {
    /// The key cannot be sent as an HTTP header (non-ASCII or control characters). No request was made.
    #[error("the API key contains characters that are not allowed in an HTTP header")]
    InvalidKey,
    /// Garmin refused the key: it expired, was deleted, or was never valid.
    #[error("Garmin rejected the API key (expired, deleted or invalid)")]
    KeyRejected,
    /// Garmin answered 429: too many requests.
    #[error("Garmin rate limit reached, try again later")]
    RateLimited,
    /// Garmin answered with a 5xx status.
    #[error("Garmin is unavailable (HTTP {status})")]
    Unavailable { status: u16 },
    /// The request did not complete: connection failure, TLS error, timeout.
    #[error("network error: {0}")]
    Network(String),
    /// Garmin answered, but not in a way we understand (unexpected status, body that is not a token).
    #[error("unexpected response from Garmin: {0}")]
    UnexpectedResponse(String),
}
