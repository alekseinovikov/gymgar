//! The crate's error type. No variant carries an API key or a token, so any of them can be logged and shown as is.

/// Why a Garmin call failed. The text never contains a key or a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GarminError {
    /// The key contains a byte outside visible ASCII (`0x21..=0x7E`): non-ASCII characters, spaces or control characters.
    /// Real keys never do, so no request was made.
    #[error("the API key contains characters other than visible ASCII")]
    InvalidKey,
    /// Garmin refused the key: it expired, was deleted, or was never valid.
    #[error("Garmin rejected the API key (expired, deleted or invalid)")]
    KeyRejected,
    /// Garmin answered 404: there is nothing at that address, for example an activity file that was deleted.
    #[error("Garmin has nothing at that address (HTTP 404)")]
    NotFound,
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
