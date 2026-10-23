//! Exchanging the user's Garmin Personal API key for a short-lived access token.
//!
//! A token lives 24 hours and there is no refresh token: when it runs low, the same key is exchanged again. The token is
//! cached in memory only (never written anywhere) and, like the key, never reaches `Debug`, `Display` or error text.
//!
//! A 401 on the exchange is reported as [`GarminError::KeyRejected`] and not retried. Retrying a data request after a 401
//! belongs to the first data request and is added together with it.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Mutex;

use super::{DEFAULT_BASE_URL, jwt};
use crate::GarminError;

const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";

/// The bytes a key may consist of: visible ASCII, without space (real keys use `[A-Za-z0-9._-]`).
const VISIBLE_ASCII: std::ops::RangeInclusive<u8> = 0x21..=0x7E;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Garmin answers with about 1.1 KB; the cap only protects against a broken or hostile server streaming without end.
const MAX_BODY_BYTES: usize = 16 * 1024;

/// A cached token is reused only while more than this is left, so a request never starts with a token about to expire.
const REFRESH_MARGIN: TimeDelta = TimeDelta::hours(1);

/// What is known about a freshly exchanged token. The token itself is only reachable through [`Auth::token`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenInfo {
    /// When the token stops working (its `exp` claim).
    pub expires_at: DateTime<Utc>,
}

struct CachedToken {
    token: SecretString,
    expires_at: DateTime<Utc>,
}

/// Turns one Personal API key into access tokens. Share one `Auth` per key: it serializes exchanges and keeps the token.
pub struct Auth {
    client: reqwest::Client,
    token_url: String,
    /// `ApiKey <key>`, marked sensitive so HTTP-level debug output hides it.
    authorization: HeaderValue,
    /// Held across the HTTP request on purpose: concurrent callers wait for one exchange instead of starting several,
    /// which would burn Garmin's rate limit.
    cache: Mutex<Option<CachedToken>>,
}

impl Auth {
    /// An `Auth` for the real Garmin Personal API. Fails with [`GarminError::InvalidKey`] if the key contains a byte outside visible ASCII (`0x21..=0x7E`).
    pub fn new(key: SecretString) -> Result<Self, GarminError> {
        Self::with_base_url(key, DEFAULT_BASE_URL)
    }

    /// Like [`Auth::new`] but against another server, such as a local mock.
    pub fn with_base_url(key: SecretString, base_url: &str) -> Result<Self, GarminError> {
        Self::build(key, base_url, REQUEST_TIMEOUT)
    }

    #[cfg(test)]
    fn with_timeout(key: SecretString, base_url: &str, timeout: Duration) -> Result<Self, GarminError> {
        Self::build(key, base_url, timeout)
    }

    fn build(key: SecretString, base_url: &str, timeout: Duration) -> Result<Self, GarminError> {
        // The key is checked here, before any request, and never truncated: real keys are 132 characters.
        // `HeaderValue::from_str` alone is too lax (it accepts bytes >= 128 and spaces), so only visible ASCII passes.
        if !key.expose_secret().bytes().all(|byte| VISIBLE_ASCII.contains(&byte)) {
            return Err(GarminError::InvalidKey);
        }
        let mut authorization = HeaderValue::from_str(&format!("ApiKey {}", key.expose_secret())).map_err(|_| GarminError::InvalidKey)?;
        authorization.set_sensitive(true);
        let client = reqwest::Client::builder().timeout(timeout).user_agent(concat!("Gymgar/", env!("CARGO_PKG_VERSION"))).build().map_err(network)?;
        Ok(Self { client, token_url: format!("{}{TOKEN_PATH}", base_url.trim_end_matches('/')), authorization, cache: Mutex::new(None) })
    }

    /// Always asks Garmin for a new token and caches it, replacing the previous one.
    pub async fn exchange(&self) -> Result<TokenInfo, GarminError> {
        let mut cache = self.cache.lock().await;
        let fresh = self.request_token().await?;
        let info = TokenInfo { expires_at: fresh.expires_at };
        *cache = Some(fresh);
        Ok(info)
    }

    /// The cached token while more than an hour of it is left, otherwise a newly exchanged one.
    pub async fn token(&self) -> Result<SecretString, GarminError> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && cached.expires_at.signed_duration_since(Utc::now()) > REFRESH_MARGIN
        {
            return Ok(cached.token.clone());
        }
        let fresh = self.request_token().await?;
        let token = fresh.token.clone();
        *cache = Some(fresh);
        Ok(token)
    }

    async fn request_token(&self) -> Result<CachedToken, GarminError> {
        let response =
            self.client.post(&self.token_url).header(AUTHORIZATION, self.authorization.clone()).header(ACCEPT, "*/*").send().await.map_err(network)?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        let body = read_body(response).await?;
        let token = body.trim();
        let expires_at = jwt::expires_at(token)?;
        Ok(CachedToken { token: SecretString::from(token), expires_at })
    }
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Auth").field("token_url", &self.token_url).finish_non_exhaustive()
    }
}

fn status_error(status: StatusCode) -> GarminError {
    if status == StatusCode::UNAUTHORIZED {
        GarminError::KeyRejected
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        GarminError::RateLimited
    } else if status.is_server_error() {
        GarminError::Unavailable { status: status.as_u16() }
    } else {
        GarminError::UnexpectedResponse(format!("HTTP {}", status.as_u16()))
    }
}

/// The body as text, refusing more than [`MAX_BODY_BYTES`]. The request timeout bounds how long this can take.
async fn read_body(mut response: reqwest::Response) -> Result<String, GarminError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            return Err(GarminError::UnexpectedResponse("the token response is too large".to_owned()));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| GarminError::UnexpectedResponse("the token response is not valid UTF-8".to_owned()))
}

/// The error and its causes on one line ("error sending request: ...: Connection refused"): the top-level text alone says
/// nothing useful. Neither the key nor the token can be in there: they only ever sit in a header value, which HTTP errors do not echo.
fn network(error: reqwest::Error) -> GarminError {
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
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use secrecy::ExposeSecret;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
    const KEY: &str = "k-123";

    /// `header.payload.signature` in base64url, with only `exp` in the payload that matters.
    fn fake_jwt(exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"iss":"garmin","exp":{exp}}}"#));
        format!("{header}.{payload}.c2lnbmF0dXJl")
    }

    fn secret(key: &str) -> SecretString {
        SecretString::from(key.to_owned())
    }

    fn auth_for(server: &MockServer) -> Auth {
        Auth::with_base_url(secret(KEY), &server.uri()).unwrap()
    }

    fn unix(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn now_plus(delta: chrono::TimeDelta) -> i64 {
        (Utc::now() + delta).timestamp()
    }

    /// A server that answers every token request with `response`.
    async fn server_answering(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(TOKEN_PATH)).respond_with(response).mount(&server).await;
        server
    }

    #[tokio::test]
    async fn exchange_ok_returns_expiry() {
        let server = MockServer::start().await;
        let expires = 1_900_000_000;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .and(header("Authorization", "ApiKey k-123"))
            .and(header("Accept", "*/*"))
            .respond_with(ResponseTemplate::new(200).set_body_string(fake_jwt(expires)))
            .expect(1)
            .mount(&server)
            .await;

        let info = auth_for(&server).exchange().await.unwrap();

        assert_eq!(info.expires_at, unix(expires));
        server.verify().await;
    }

    #[tokio::test]
    async fn exchange_sends_an_empty_body_and_a_gymgar_user_agent() {
        let server = server_answering(ResponseTemplate::new(200).set_body_string(fake_jwt(1_900_000_000))).await;

        auth_for(&server).exchange().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let request = requests.first().unwrap();
        assert!(request.body.is_empty());
        let agent = request.headers.get("user-agent").unwrap().to_str().unwrap();
        assert_eq!(agent, concat!("Gymgar/", env!("CARGO_PKG_VERSION")));
    }

    #[tokio::test]
    async fn exchange_trims_whitespace_around_the_token() {
        let jwt = fake_jwt(1_900_000_000);
        let server = server_answering(ResponseTemplate::new(200).set_body_string(format!("{jwt}\n"))).await;
        let auth = auth_for(&server);

        auth.exchange().await.unwrap();

        assert_eq!(auth.token().await.unwrap().expose_secret(), jwt);
    }

    #[tokio::test]
    async fn exchange_401_is_key_rejected() {
        let server = server_answering(ResponseTemplate::new(401)).await;
        assert_eq!(auth_for(&server).exchange().await.unwrap_err(), GarminError::KeyRejected);
    }

    #[tokio::test]
    async fn exchange_429_is_rate_limited() {
        let server = server_answering(ResponseTemplate::new(429)).await;
        assert_eq!(auth_for(&server).exchange().await.unwrap_err(), GarminError::RateLimited);
    }

    #[tokio::test]
    async fn exchange_503_is_unavailable() {
        let server = server_answering(ResponseTemplate::new(503)).await;
        match auth_for(&server).exchange().await.unwrap_err() {
            GarminError::Unavailable { status } => assert_eq!(status, 503),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exchange_other_error_status_is_unexpected() {
        let server = server_answering(ResponseTemplate::new(403)).await;
        assert_eq!(auth_for(&server).exchange().await.unwrap_err(), GarminError::UnexpectedResponse("HTTP 403".to_owned()));
    }

    #[tokio::test]
    async fn exchange_garbage_body_is_unexpected() {
        let server = server_answering(ResponseTemplate::new(200).set_body_string("not-a-jwt")).await;
        assert!(matches!(auth_for(&server).exchange().await.unwrap_err(), GarminError::UnexpectedResponse(_)));
    }

    #[tokio::test]
    async fn exchange_empty_body_is_unexpected() {
        let server = server_answering(ResponseTemplate::new(200)).await;
        assert!(matches!(auth_for(&server).exchange().await.unwrap_err(), GarminError::UnexpectedResponse(_)));
    }

    #[tokio::test]
    async fn exchange_oversized_body_is_unexpected() {
        let server = server_answering(ResponseTemplate::new(200).set_body_string("a".repeat(1024 * 1024))).await;
        match auth_for(&server).exchange().await.unwrap_err() {
            GarminError::UnexpectedResponse(reason) => assert!(reason.contains("too large"), "{reason}"),
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exchange_timeout_is_network() {
        let delayed = ResponseTemplate::new(200).set_body_string(fake_jwt(1_900_000_000)).set_delay(Duration::from_secs(2));
        let server = server_answering(delayed).await;
        let auth = Auth::with_timeout(secret(KEY), &server.uri(), Duration::from_secs(1)).unwrap();

        assert!(matches!(auth.exchange().await.unwrap_err(), GarminError::Network(_)));
    }

    #[tokio::test]
    async fn exchange_connection_refused_is_network() {
        // Bind a port, then free it, so nothing listens there.
        let address = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let auth = Auth::with_base_url(secret(KEY), &format!("http://{address}")).unwrap();

        assert!(matches!(auth.exchange().await.unwrap_err(), GarminError::Network(_)));
    }

    #[tokio::test]
    async fn long_key_is_sent_whole() {
        let key = "aB3._-".repeat(50);
        assert_eq!(key.len(), 300);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .and(header("Authorization", format!("ApiKey {key}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string(fake_jwt(1_900_000_000)))
            .expect(1)
            .mount(&server)
            .await;

        Auth::with_base_url(secret(&key), &server.uri()).unwrap().exchange().await.unwrap();

        server.verify().await;
    }

    #[tokio::test]
    async fn non_ascii_key_is_invalid_key_without_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;

        let error = Auth::with_base_url(secret("ключ\nabc"), &server.uri()).unwrap_err();

        assert_eq!(error, GarminError::InvalidKey);
        server.verify().await;
    }

    #[tokio::test]
    async fn cyrillic_only_key_is_invalid_key_without_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;

        let error = Auth::with_base_url(secret("ключ"), &server.uri()).unwrap_err();

        assert_eq!(error, GarminError::InvalidKey);
        server.verify().await;
    }

    #[tokio::test]
    async fn key_with_inner_space_is_invalid_key_without_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;

        let error = Auth::with_base_url(secret("abc def"), &server.uri()).unwrap_err();

        assert_eq!(error, GarminError::InvalidKey);
        server.verify().await;
    }

    #[tokio::test]
    async fn base_url_may_end_with_a_slash() {
        let server = server_answering(ResponseTemplate::new(200).set_body_string(fake_jwt(1_900_000_000))).await;
        let auth = Auth::with_base_url(secret(KEY), &format!("{}/", server.uri())).unwrap();

        assert_eq!(auth.exchange().await.unwrap().expires_at, unix(1_900_000_000));
    }

    #[tokio::test]
    async fn token_is_cached_until_one_hour_left() {
        let long_lived = fake_jwt(now_plus(chrono::TimeDelta::hours(24)));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string(long_lived.clone()))
            .expect(1)
            .mount(&server)
            .await;
        let auth = auth_for(&server);

        let first = auth.token().await.unwrap();
        let second = auth.token().await.unwrap();

        assert_eq!(first.expose_secret(), long_lived);
        assert_eq!(second.expose_secret(), long_lived);
        server.verify().await;
    }

    #[tokio::test]
    async fn token_is_exchanged_again_when_under_an_hour_is_left() {
        let short_lived = fake_jwt(now_plus(chrono::TimeDelta::minutes(30)));
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(TOKEN_PATH)).respond_with(ResponseTemplate::new(200).set_body_string(short_lived)).expect(2).mount(&server).await;
        let auth = auth_for(&server);

        auth.token().await.unwrap();
        auth.token().await.unwrap();

        server.verify().await;
    }

    #[tokio::test]
    async fn exchange_always_asks_garmin_and_refreshes_the_cache() {
        let long_lived = fake_jwt(now_plus(chrono::TimeDelta::hours(24)));
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(TOKEN_PATH)).respond_with(ResponseTemplate::new(200).set_body_string(long_lived)).expect(2).mount(&server).await;
        let auth = auth_for(&server);

        auth.token().await.unwrap();
        auth.exchange().await.unwrap();
        auth.token().await.unwrap();

        server.verify().await;
    }

    #[tokio::test]
    async fn failed_exchange_is_not_cached() {
        let server = server_answering(ResponseTemplate::new(503)).await;
        let auth = auth_for(&server);

        assert!(auth.token().await.is_err());
        assert!(auth.token().await.is_err());

        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn concurrent_token_calls_share_one_exchange() {
        let long_lived = fake_jwt(now_plus(chrono::TimeDelta::hours(24)));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string(long_lived).set_delay(Duration::from_millis(200)))
            .expect(1)
            .mount(&server)
            .await;
        let auth = auth_for(&server);

        let (a, b, c) = tokio::join!(auth.token(), auth.token(), auth.token());

        assert!(a.is_ok() && b.is_ok() && c.is_ok());
        server.verify().await;
    }

    #[tokio::test]
    async fn errors_do_not_contain_key() {
        let variants = [
            GarminError::InvalidKey,
            GarminError::KeyRejected,
            GarminError::RateLimited,
            GarminError::Unavailable { status: 503 },
            GarminError::Network("connection refused".to_owned()),
            GarminError::UnexpectedResponse("HTTP 418".to_owned()),
        ];
        for error in variants {
            assert!(!format!("{error:?}").contains(KEY), "{error:?}");
            assert!(!format!("{error}").contains(KEY), "{error}");
        }

        // The same for errors produced by real exchanges, including a body that looks like a token.
        for response in [
            ResponseTemplate::new(401),
            ResponseTemplate::new(429),
            ResponseTemplate::new(503),
            ResponseTemplate::new(418),
            ResponseTemplate::new(200).set_body_string("not-a-jwt"),
            ResponseTemplate::new(200).set_body_string("k-123"),
        ] {
            let server = server_answering(response).await;
            let error = auth_for(&server).exchange().await.unwrap_err();
            assert!(!format!("{error:?}").contains(KEY), "{error:?}");
            assert!(!format!("{error}").contains(KEY), "{error}");
        }
    }

    #[tokio::test]
    async fn garbage_body_is_not_echoed_in_the_error() {
        let server = server_answering(ResponseTemplate::new(200).set_body_string("not-a-jwt")).await;
        let error = auth_for(&server).exchange().await.unwrap_err();
        assert!(!format!("{error:?}").contains("not-a-jwt"), "{error:?}");
    }

    #[tokio::test]
    async fn debug_output_hides_key_and_token() {
        let jwt = fake_jwt(1_900_000_000);
        let server = server_answering(ResponseTemplate::new(200).set_body_string(jwt.clone())).await;
        let auth = auth_for(&server);
        let info = auth.exchange().await.unwrap();
        let token = auth.token().await.unwrap();

        for text in [format!("{auth:?}"), format!("{info:?}"), format!("{token:?}")] {
            assert!(!text.contains(KEY) && !text.contains(&jwt), "{text}");
        }
    }
}
