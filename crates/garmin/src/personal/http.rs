//! What the token request and the data requests have in common: the HTTP client, how a status becomes a [`GarminError`],
//! how a body is read, and how a transport failure is worded.
//!
//! Nothing here sees a key or a token: those only ever sit in a header value, which none of these functions echo.

use std::error::Error;
use std::time::Duration;

use reqwest::StatusCode;

use crate::GarminError;

/// How long one request may take, from connecting to the last byte of the body.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A client that identifies itself as Gymgar and gives up after `timeout`.
pub(super) fn http_client(timeout: Duration) -> Result<reqwest::Client, GarminError> {
    reqwest::Client::builder().timeout(timeout).user_agent(concat!("Gymgar/", env!("CARGO_PKG_VERSION"))).build().map_err(network)
}

/// What a non-success status means. `401` is [`GarminError::KeyRejected`] for every request: on the token request it is the key
/// that is refused, on a data request it is the token, which the client answers by exchanging the key once more.
pub(super) fn status_error(status: StatusCode) -> GarminError {
    if status == StatusCode::UNAUTHORIZED {
        GarminError::KeyRejected
    } else if status == StatusCode::NOT_FOUND {
        GarminError::NotFound
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        GarminError::RateLimited
    } else if status.is_server_error() {
        GarminError::Unavailable { status: status.as_u16() }
    } else {
        GarminError::UnexpectedResponse(format!("HTTP {}", status.as_u16()))
    }
}

/// The whole body, refusing more than `limit` bytes. The request timeout bounds how long this can take; the limit bounds the memory,
/// because the size comes from the server and a broken or hostile one could stream without end.
pub(super) async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, GarminError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(GarminError::UnexpectedResponse(format!("response larger than {limit} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The error and its causes on one line ("error sending request: ...: Connection refused"): the top-level text alone says
/// nothing useful. Neither the key nor the token can be in there: they only ever sit in a header value, which HTTP errors do not echo.
pub(super) fn network(error: reqwest::Error) -> GarminError {
    let error = error.without_url();
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        text.push_str(": ");
        text.push_str(&next.to_string());
        cause = next.source();
    }
    GarminError::Network(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_errors() {
        let cases = [
            (401, GarminError::KeyRejected),
            (404, GarminError::NotFound),
            (429, GarminError::RateLimited),
            (500, GarminError::Unavailable { status: 500 }),
            (503, GarminError::Unavailable { status: 503 }),
            (400, GarminError::UnexpectedResponse("HTTP 400".to_owned())),
            (403, GarminError::UnexpectedResponse("HTTP 403".to_owned())),
            (418, GarminError::UnexpectedResponse("HTTP 418".to_owned())),
            (302, GarminError::UnexpectedResponse("HTTP 302".to_owned())),
        ];
        for (status, expected) in cases {
            assert_eq!(status_error(StatusCode::from_u16(status).unwrap()), expected, "status {status}");
        }
    }
}
