//! Data requests to the Personal API: the activity list and the original file of one activity.
//!
//! A [`Client`] owns the [`Auth`] of one key and sends `Authorization: Bearer <token>` with every request. The access token is
//! short-lived and can be revoked at any time, so a 401 on a data request is answered with exactly one more exchange of the key
//! and exactly one more try; a second 401 means the key itself is no good ([`GarminError::KeyRejected`]).
//!
//! No other retry, backoff or pacing happens here: a 429, a 5xx or a network error is returned as it is, and the caller (the sync
//! job) decides when to try again. Bodies are returned as raw bytes, so the caller can store them untouched and parse them later.
//! The key and the token never reach `Debug`, `Display` or error text.

use std::fmt;

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use secrecy::{ExposeSecret, SecretString};

use super::http::{REQUEST_TIMEOUT, http_client, network, read_capped, status_error};
use super::{Auth, DEFAULT_BASE_URL, TokenInfo};
use crate::GarminError;

const LIST_PATH: &str = "/activitylist-service/activities/search/activities";
const FILE_PATH: &str = "/download-service/files/activity";

/// A page of the list is some 100 elements of a few KB each; the cap protects against a server that never stops.
const MAX_LIST_BYTES: usize = 16 * 1024 * 1024;
/// The ZIP of one activity (a `.fit` file inside) is well under a megabyte; this leaves room for the longest activities.
const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;

/// A client for the Personal API data endpoints, for one key. Share one `Client` per key: it shares the token between requests.
pub struct Client {
    auth: Auth,
    http: reqwest::Client,
    base_url: String,
    max_list_bytes: usize,
    max_file_bytes: usize,
}

impl Client {
    /// A `Client` for the real Garmin Personal API. Fails with [`GarminError::InvalidKey`] for a key that cannot be sent.
    pub fn new(key: SecretString) -> Result<Self, GarminError> {
        Self::with_base_url(key, DEFAULT_BASE_URL)
    }

    /// Like [`Client::new`] but against another server, such as a local mock.
    pub fn with_base_url(key: SecretString, base_url: &str) -> Result<Self, GarminError> {
        Self::build(key, base_url, MAX_LIST_BYTES, MAX_FILE_BYTES)
    }

    #[cfg(test)]
    fn with_limits(key: SecretString, base_url: &str, max_list_bytes: usize, max_file_bytes: usize) -> Result<Self, GarminError> {
        Self::build(key, base_url, max_list_bytes, max_file_bytes)
    }

    fn build(key: SecretString, base_url: &str, max_list_bytes: usize, max_file_bytes: usize) -> Result<Self, GarminError> {
        // The key goes into `Auth` and nowhere else: only the token is ever needed here.
        let auth = Auth::with_base_url(key, base_url)?;
        let http = http_client(REQUEST_TIMEOUT)?;
        Ok(Self { auth, http, base_url: base_url.trim_end_matches('/').to_owned(), max_list_bytes, max_file_bytes })
    }

    /// Always exchanges the key for a new token. This is how a key is checked without fetching anything.
    pub async fn exchange(&self) -> Result<TokenInfo, GarminError> {
        self.auth.exchange().await
    }

    /// One page of the activity list as Garmin sent it (a JSON array; see [`parse_activity_page`](super::parse_activity_page)),
    /// newest first, starting at element `start`, at most `limit` elements.
    pub async fn activities_page(&self, start: u32, limit: u32) -> Result<Vec<u8>, GarminError> {
        self.get(&format!("{}{LIST_PATH}?start={start}&limit={limit}", self.base_url), self.max_list_bytes).await
    }

    /// The original file of an activity (a ZIP with one `.fit` inside) as Garmin sent it. A file that does not exist is
    /// [`GarminError::NotFound`].
    pub async fn activity_file(&self, id: i64) -> Result<Vec<u8>, GarminError> {
        self.get(&format!("{}{FILE_PATH}/{id}", self.base_url), self.max_file_bytes).await
    }

    /// One GET with the 401 policy: try with the cached token; on a 401 exchange the key once and try once more.
    async fn get(&self, url: &str, limit: usize) -> Result<Vec<u8>, GarminError> {
        let token = self.auth.token().await?;
        match self.fetch(url, &token, limit).await {
            Err(GarminError::KeyRejected) => {}
            outcome => return outcome,
        }
        // The token was refused. If the exchange is refused too, that error (`KeyRejected`) ends the request here.
        let (token, _) = self.auth.exchange_token().await?;
        self.fetch(url, &token, limit).await
    }

    async fn fetch(&self, url: &str, token: &SecretString, limit: usize) -> Result<Vec<u8>, GarminError> {
        // The text does not quote the token. A JWT is plain ASCII, so this only fails for a token Garmin should never have issued.
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
            .map_err(|_| GarminError::UnexpectedResponse("the access token cannot be sent as a header".to_owned()))?;
        authorization.set_sensitive(true);
        let response = self.http.get(url).header(AUTHORIZATION, authorization).header(ACCEPT, "*/*").send().await.map_err(network)?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        read_capped(response, limit).await
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").field("base_url", &self.base_url).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};
    use secrecy::SecretString;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::jwt::fake_jwt;
    use super::*;

    const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
    const LIST_PATH: &str = "/activitylist-service/activities/search/activities";
    const FILE_PATH: &str = "/download-service/files/activity/42";
    const KEY: &str = "k-123";

    fn secret(key: &str) -> SecretString {
        SecretString::from(key.to_owned())
    }

    fn client_for(server: &MockServer) -> Client {
        Client::with_base_url(secret(KEY), &server.uri()).unwrap()
    }

    fn now_plus(delta: TimeDelta) -> i64 {
        (Utc::now() + delta).timestamp()
    }

    /// A token that is good for a day, so the cache keeps it.
    fn long_lived() -> String {
        fake_jwt(now_plus(TimeDelta::hours(24)))
    }

    /// Lets every token exchange succeed with `token`; the mock expects exactly `times` exchanges.
    async fn mount_exchange(server: &MockServer, token: &str, times: impl Into<wiremock::Times>) {
        Mock::given(method("POST")).and(path(TOKEN_PATH)).respond_with(ResponseTemplate::new(200).set_body_string(token)).expect(times).mount(server).await;
    }

    async fn mount_get(server: &MockServer, at: &str, response: ResponseTemplate, times: impl Into<wiremock::Times>) {
        Mock::given(method("GET")).and(path(at)).respond_with(response).expect(times).mount(server).await;
    }

    #[tokio::test]
    async fn page_ok_sends_bearer_and_query() {
        let server = MockServer::start().await;
        let token = long_lived();
        mount_exchange(&server, &token, 1).await;
        let body = b"[ {\"activityId\": 1} ]\n".to_vec();
        Mock::given(method("GET"))
            .and(path(LIST_PATH))
            .and(header("Authorization", format!("Bearer {token}").as_str()))
            .and(query_param("start", "100"))
            .and(query_param("limit", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .expect(1)
            .mount(&server)
            .await;

        let received = client_for(&server).activities_page(100, 100).await.unwrap();

        assert_eq!(received, body);
        server.verify().await;
    }

    #[tokio::test]
    async fn data_requests_send_a_gymgar_user_agent() {
        let server = MockServer::start().await;
        mount_exchange(&server, &long_lived(), 1).await;
        mount_get(&server, LIST_PATH, ResponseTemplate::new(200).set_body_string("[]"), 1).await;

        client_for(&server).activities_page(0, 100).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let get = requests.iter().find(|request| request.method == wiremock::http::Method::GET).unwrap();
        assert_eq!(get.headers.get("user-agent").unwrap().to_str().unwrap(), concat!("Gymgar/", env!("CARGO_PKG_VERSION")));
    }

    #[tokio::test]
    async fn file_ok_returns_bytes() {
        let server = MockServer::start().await;
        let token = long_lived();
        mount_exchange(&server, &token, 1).await;
        // Not UTF-8 and not JSON: the first bytes of a ZIP archive.
        let archive = vec![0x50, 0x4B, 0x03, 0x04, 0xFF, 0x00, 0xFE, 0x80];
        Mock::given(method("GET"))
            .and(path(FILE_PATH))
            .and(header("Authorization", format!("Bearer {token}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(archive.clone()))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(client_for(&server).activity_file(42).await.unwrap(), archive);
        server.verify().await;
    }

    #[tokio::test]
    async fn file_404_is_not_found() {
        let server = MockServer::start().await;
        mount_exchange(&server, &long_lived(), 1).await;
        mount_get(&server, FILE_PATH, ResponseTemplate::new(404), 1).await;

        assert_eq!(client_for(&server).activity_file(42).await.unwrap_err(), GarminError::NotFound);
        server.verify().await;
    }

    #[tokio::test]
    async fn data_401_refreshes_once_then_succeeds() {
        let server = MockServer::start().await;
        mount_exchange(&server, &long_lived(), 2).await;
        Mock::given(method("GET")).and(path(LIST_PATH)).respond_with(ResponseTemplate::new(401)).up_to_n_times(1).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path(LIST_PATH)).respond_with(ResponseTemplate::new(200).set_body_string("[]")).expect(1).mount(&server).await;

        let body = client_for(&server).activities_page(0, 100).await.unwrap();

        assert_eq!(body, b"[]");
        server.verify().await;
    }

    #[tokio::test]
    async fn the_retry_uses_the_new_token() {
        let server = MockServer::start().await;
        let stale = fake_jwt(now_plus(TimeDelta::hours(24)));
        let fresh = fake_jwt(now_plus(TimeDelta::hours(23)));
        assert_ne!(stale, fresh);
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string(stale.clone()))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string(fresh.clone()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(LIST_PATH))
            .and(header("Authorization", format!("Bearer {stale}").as_str()))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(LIST_PATH))
            .and(header("Authorization", format!("Bearer {fresh}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
            .expect(1)
            .mount(&server)
            .await;

        client_for(&server).activities_page(0, 100).await.unwrap();

        server.verify().await;
    }

    #[tokio::test]
    async fn the_retry_does_not_exchange_again_for_a_short_lived_token() {
        // A token with under an hour left is never served from the cache, so asking the cache again after the exchange
        // would start a third exchange. The retry must use the token the exchange just returned.
        let server = MockServer::start().await;
        mount_exchange(&server, &fake_jwt(now_plus(TimeDelta::minutes(30))), 2).await;
        Mock::given(method("GET")).and(path(LIST_PATH)).respond_with(ResponseTemplate::new(401)).up_to_n_times(1).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path(LIST_PATH)).respond_with(ResponseTemplate::new(200).set_body_string("[]")).expect(1).mount(&server).await;

        client_for(&server).activities_page(0, 100).await.unwrap();

        server.verify().await;
    }

    #[tokio::test]
    async fn data_401_twice_is_key_rejected() {
        let server = MockServer::start().await;
        mount_exchange(&server, &long_lived(), 2).await;
        mount_get(&server, LIST_PATH, ResponseTemplate::new(401), 2).await;

        assert_eq!(client_for(&server).activities_page(0, 100).await.unwrap_err(), GarminError::KeyRejected);
        server.verify().await;
    }

    #[tokio::test]
    async fn data_401_then_rejected_exchange_is_key_rejected_without_a_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string(long_lived()))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path(TOKEN_PATH)).respond_with(ResponseTemplate::new(401)).expect(1).mount(&server).await;
        mount_get(&server, FILE_PATH, ResponseTemplate::new(401), 1).await;

        assert_eq!(client_for(&server).activity_file(42).await.unwrap_err(), GarminError::KeyRejected);
        server.verify().await;
    }

    #[tokio::test]
    async fn other_statuses_are_not_retried() {
        let cases = [
            (429, GarminError::RateLimited),
            (503, GarminError::Unavailable { status: 503 }),
            (404, GarminError::NotFound),
            (403, GarminError::UnexpectedResponse("HTTP 403".to_owned())),
        ];
        for (status, expected) in cases {
            let server = MockServer::start().await;
            mount_exchange(&server, &long_lived(), 1).await;
            mount_get(&server, LIST_PATH, ResponseTemplate::new(status), 1).await;

            assert_eq!(client_for(&server).activities_page(0, 100).await.unwrap_err(), expected, "status {status}");
            server.verify().await;
        }
    }

    #[tokio::test]
    async fn the_token_is_reused_between_requests() {
        let server = MockServer::start().await;
        mount_exchange(&server, &long_lived(), 1).await;
        mount_get(&server, LIST_PATH, ResponseTemplate::new(200).set_body_string("[]"), 2).await;
        let client = client_for(&server);

        client.activities_page(0, 100).await.unwrap();
        client.activities_page(100, 100).await.unwrap();

        server.verify().await;
    }

    #[tokio::test]
    async fn a_failed_exchange_before_the_request_is_reported_and_nothing_is_fetched() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(TOKEN_PATH)).respond_with(ResponseTemplate::new(503)).expect(1).mount(&server).await;
        mount_get(&server, LIST_PATH, ResponseTemplate::new(200), 0).await;

        assert_eq!(client_for(&server).activities_page(0, 100).await.unwrap_err(), GarminError::Unavailable { status: 503 });
        server.verify().await;
    }

    #[tokio::test]
    async fn exchange_reports_the_expiry() {
        let server = MockServer::start().await;
        mount_exchange(&server, &fake_jwt(1_900_000_000), 1).await;

        let info = client_for(&server).exchange().await.unwrap();

        assert_eq!(info.expires_at, chrono::DateTime::from_timestamp(1_900_000_000, 0).unwrap());
        server.verify().await;
    }

    #[tokio::test]
    async fn exchange_401_is_key_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(TOKEN_PATH)).respond_with(ResponseTemplate::new(401)).mount(&server).await;

        assert_eq!(client_for(&server).exchange().await.unwrap_err(), GarminError::KeyRejected);
    }

    #[tokio::test]
    async fn invalid_key_is_rejected_before_any_request() {
        let server = MockServer::start().await;

        assert_eq!(Client::with_base_url(secret("ключ"), &server.uri()).unwrap_err(), GarminError::InvalidKey);
        assert_eq!(Client::new(secret("abc def")).unwrap_err(), GarminError::InvalidKey);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn base_url_may_end_with_a_slash() {
        let server = MockServer::start().await;
        mount_exchange(&server, &long_lived(), 1).await;
        mount_get(&server, LIST_PATH, ResponseTemplate::new(200).set_body_string("[]"), 1).await;
        let client = Client::with_base_url(secret(KEY), &format!("{}/", server.uri())).unwrap();

        assert_eq!(client.activities_page(0, 100).await.unwrap(), b"[]");
        server.verify().await;
    }

    #[tokio::test]
    async fn body_over_limit_is_unexpected() {
        let server = MockServer::start().await;
        mount_exchange(&server, &long_lived(), 1).await;
        mount_get(&server, LIST_PATH, ResponseTemplate::new(200).set_body_bytes(vec![b' '; 9]), 1).await;
        mount_get(&server, FILE_PATH, ResponseTemplate::new(200).set_body_bytes(vec![0; 17]), 1).await;
        let client = Client::with_limits(secret(KEY), &server.uri(), 8, 16).unwrap();

        assert_eq!(client.activities_page(0, 100).await.unwrap_err(), GarminError::UnexpectedResponse("response larger than 8 bytes".to_owned()));
        assert_eq!(client.activity_file(42).await.unwrap_err(), GarminError::UnexpectedResponse("response larger than 16 bytes".to_owned()));
        server.verify().await;
    }

    #[tokio::test]
    async fn body_exactly_at_the_limit_is_accepted() {
        let server = MockServer::start().await;
        mount_exchange(&server, &long_lived(), 1).await;
        mount_get(&server, LIST_PATH, ResponseTemplate::new(200).set_body_bytes(vec![b' '; 8]), 1).await;
        mount_get(&server, FILE_PATH, ResponseTemplate::new(200).set_body_bytes(vec![0; 16]), 1).await;
        let client = Client::with_limits(secret(KEY), &server.uri(), 8, 16).unwrap();

        assert_eq!(client.activities_page(0, 100).await.unwrap().len(), 8);
        assert_eq!(client.activity_file(42).await.unwrap().len(), 16);
    }

    #[test]
    fn real_limits_are_16_and_32_mebibytes() {
        assert_eq!(MAX_LIST_BYTES, 16 * 1024 * 1024);
        assert_eq!(MAX_FILE_BYTES, 32 * 1024 * 1024);
    }

    #[tokio::test]
    async fn connection_refused_is_network() {
        // Bind a port, then free it, so nothing listens there.
        let address = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let client = Client::with_base_url(secret(KEY), &format!("http://{address}")).unwrap();

        assert!(matches!(client.activities_page(0, 100).await.unwrap_err(), GarminError::Network(_)));
    }

    #[tokio::test]
    async fn errors_and_debug_output_do_not_contain_key_or_token() {
        let token = long_lived();
        for status in [401, 404, 429, 503, 418] {
            let server = MockServer::start().await;
            mount_exchange(&server, &token, 1..=2).await;
            mount_get(&server, FILE_PATH, ResponseTemplate::new(status), 1..=2).await;
            let client = client_for(&server);

            let error = client.activity_file(42).await.unwrap_err();

            for text in [format!("{error:?}"), format!("{error}"), format!("{client:?}")] {
                assert!(!text.contains(KEY) && !text.contains(&token), "{text}");
            }
        }
    }
}
