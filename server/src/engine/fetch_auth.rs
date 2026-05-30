use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::{BoxFuture, FutureExt, Shared};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthTokenSourceConfig {
    pub header: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: Option<String>,
    pub refresh_buffer_secs: u64,
}

#[derive(Clone)]
pub struct OAuthClientCredentialsTokenSource {
    client: Client,
    config: OAuthTokenSourceConfig,
    state: Arc<Mutex<TokenSourceState>>,
}

impl std::fmt::Debug for OAuthClientCredentialsTokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClientCredentialsTokenSource")
            .field("header", &self.config.header)
            .field("token_url", &self.config.token_url)
            .field("client_id", &self.config.client_id)
            .field("client_secret", &"<redacted>")
            .field("scope", &self.config.scope)
            .field("refresh_buffer_secs", &self.config.refresh_buffer_secs)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
struct CachedToken {
    access_token: String,
    token_type: String,
    refresh_token: Option<String>,
    expires_at: SystemTime,
}

impl CachedToken {
    fn authorization_header_value(&self) -> String {
        format!("{} {}", self.token_type, self.access_token)
    }

    fn is_valid(&self, refresh_buffer_secs: u64) -> bool {
        match self
            .expires_at
            .checked_sub(Duration::from_secs(refresh_buffer_secs))
        {
            Some(refresh_at) => SystemTime::now() < refresh_at,
            None => false,
        }
    }
}

#[derive(Default)]
struct TokenSourceState {
    cached_token: Option<CachedToken>,
    in_flight: Option<InFlightRequest>,
    next_generation: u64,
    recent_failures: VecDeque<CompletedFailure>,
}

#[derive(Debug, Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwtExpiryClaims {
    exp: u64,
}

type SharedTokenFuture = Shared<BoxFuture<'static, Result<CachedToken, String>>>;

#[derive(Clone)]
struct InFlightRequest {
    generation: u64,
    future: SharedTokenFuture,
}

#[derive(Clone)]
struct CompletedFailure {
    generation: u64,
    error: String,
}

impl OAuthClientCredentialsTokenSource {
    pub fn new(client: Client, config: OAuthTokenSourceConfig) -> Self {
        Self {
            client,
            config,
            state: Arc::new(Mutex::new(TokenSourceState::default())),
        }
    }

    pub async fn authorization_header_value(&self) -> Result<String, String> {
        loop {
            let in_flight = {
                let mut state = self.state.lock().await;
                if let Some(token) = state.cached_token.as_ref() {
                    if token.is_valid(self.config.refresh_buffer_secs) {
                        return Ok(token.authorization_header_value());
                    }
                }

                if let Some(in_flight) = state.in_flight.clone() {
                    in_flight
                } else {
                    let refresh_token = state
                        .cached_token
                        .as_ref()
                        .and_then(|token| token.refresh_token.clone());
                    let this = self.clone();
                    let future = async move { this.refresh_or_reacquire(refresh_token).await }
                        .boxed()
                        .shared();
                    let in_flight = InFlightRequest {
                        generation: state.next_generation,
                        future,
                    };
                    state.next_generation += 1;
                    state.in_flight = Some(in_flight.clone());
                    in_flight
                }
            };

            let result = in_flight.future.await;

            let mut state = self.state.lock().await;
            match apply_in_flight_result(&mut state, in_flight.generation, result.clone()) {
                InFlightResolution::Applied => {
                    return result.map(|token| token.authorization_header_value());
                }
                InFlightResolution::AlreadyFailed(error) => {
                    return Err(error);
                }
                InFlightResolution::Stale => continue,
            }
        }
    }

    async fn refresh_or_reacquire(
        &self,
        refresh_token: Option<String>,
    ) -> Result<CachedToken, String> {
        if let Some(refresh_token) = refresh_token {
            match self
                .fetch_token(TokenGrant::RefreshToken {
                    refresh_token: refresh_token.clone(),
                })
                .await
            {
                Ok(mut token) => {
                    if token.refresh_token.is_none() {
                        token.refresh_token = Some(refresh_token);
                    }
                    return Ok(token);
                }
                Err(refresh_error) => {
                    return self
                        .fetch_token(TokenGrant::ClientCredentials)
                        .await
                        .map_err(|reacquire_error| {
                            format!(
                                "token refresh failed: {}; token reacquire failed: {}",
                                refresh_error, reacquire_error
                            )
                        });
                }
            }
        }

        self.fetch_token(TokenGrant::ClientCredentials).await
    }

    async fn fetch_token(&self, grant: TokenGrant) -> Result<CachedToken, String> {
        let mut form = vec![
            ("grant_type", grant.grant_type().to_string()),
            ("client_id", self.config.client_id.clone()),
            ("client_secret", self.config.client_secret.clone()),
        ];
        if let Some(refresh_token) = grant.refresh_token() {
            form.push(("refresh_token", refresh_token.to_string()));
        }
        if matches!(grant, TokenGrant::ClientCredentials) {
            if let Some(scope) = self.config.scope.as_ref() {
                form.push(("scope", scope.clone()));
            }
        }

        let response = self
            .client
            .post(&self.config.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|error| format!("token request failed: {error}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("unable to read error body: {error}"));
            return Err(format!("token endpoint returned {status}: {body}"));
        }

        let payload: TokenEndpointResponse = response
            .json()
            .await
            .map_err(|error| format!("token response JSON was invalid: {error}"))?;

        let expires_at = derive_expiry(&payload.access_token, payload.expires_in)?;
        let token_type = payload
            .token_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Bearer")
            .to_string();

        Ok(CachedToken {
            access_token: payload.access_token,
            token_type,
            refresh_token: payload.refresh_token,
            expires_at,
        })
    }
}

enum InFlightResolution {
    Applied,
    AlreadyFailed(String),
    Stale,
}

fn apply_in_flight_result(
    state: &mut TokenSourceState,
    generation: u64,
    result: Result<CachedToken, String>,
) -> InFlightResolution {
    if let Some(error) = state
        .recent_failures
        .iter()
        .find(|failure| failure.generation == generation)
        .map(|failure| failure.error.clone())
    {
        return InFlightResolution::AlreadyFailed(error);
    }

    let Some(in_flight) = state.in_flight.as_ref() else {
        return InFlightResolution::Stale;
    };

    if in_flight.generation != generation {
        return InFlightResolution::Stale;
    }

    state.in_flight = None;
    match result {
        Ok(token) => state.cached_token = Some(token),
        Err(error) => {
            state.cached_token = None;
            remember_failure(state, generation, error);
        }
    }
    InFlightResolution::Applied
}

fn remember_failure(state: &mut TokenSourceState, generation: u64, error: String) {
    const MAX_RECORDED_FAILURES: usize = 8;

    if let Some(existing) = state
        .recent_failures
        .iter_mut()
        .find(|failure| failure.generation == generation)
    {
        existing.error = error;
        return;
    }

    if state.recent_failures.len() >= MAX_RECORDED_FAILURES {
        state.recent_failures.pop_front();
    }

    state.recent_failures
        .push_back(CompletedFailure { generation, error });
}

#[derive(Clone, Debug)]
enum TokenGrant {
    ClientCredentials,
    RefreshToken { refresh_token: String },
}

impl TokenGrant {
    fn grant_type(&self) -> &'static str {
        match self {
            TokenGrant::ClientCredentials => "client_credentials",
            TokenGrant::RefreshToken { .. } => "refresh_token",
        }
    }

    fn refresh_token(&self) -> Option<&str> {
        match self {
            TokenGrant::ClientCredentials => None,
            TokenGrant::RefreshToken { refresh_token } => Some(refresh_token.as_str()),
        }
    }
}

fn derive_expiry(access_token: &str, expires_in: Option<u64>) -> Result<SystemTime, String> {
    if let Some(expires_in) = expires_in {
        return SystemTime::now()
            .checked_add(Duration::from_secs(expires_in))
            .ok_or_else(|| "token expiry overflowed system clock".to_string());
    }

    let mut validation = Validation::new(Algorithm::HS256);
    validation.insecure_disable_signature_validation();
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    validation.algorithms = vec![
        Algorithm::HS256,
        Algorithm::HS384,
        Algorithm::HS512,
        Algorithm::ES256,
        Algorithm::ES384,
        Algorithm::RS256,
        Algorithm::RS384,
        Algorithm::RS512,
        Algorithm::PS256,
        Algorithm::PS384,
        Algorithm::PS512,
        Algorithm::EdDSA,
    ];

    let token_data = jsonwebtoken::decode::<JwtExpiryClaims>(
        access_token,
        &DecodingKey::from_secret(&[]),
        &validation,
    )
    .map_err(|error| {
        format!("token response missing expires_in and JWT exp could not be decoded: {error}")
    })?;

    UNIX_EPOCH
        .checked_add(Duration::from_secs(token_data.claims.exp))
        .ok_or_else(|| "token JWT exp overflowed system clock".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use axum::{
        Json, Router,
        extract::{Form, State},
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::post,
    };
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde::Serialize;
    use serde_json::{Value, json};
    use tokio::sync::Mutex;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TokenRequestRecord {
        grant_type: String,
        client_id: String,
        client_secret: String,
        refresh_token: Option<String>,
        scope: Option<String>,
    }

    #[derive(Clone, Debug)]
    struct TokenResponseSpec {
        status: StatusCode,
        body: Value,
        delay: Duration,
    }

    impl TokenResponseSpec {
        fn success(body: Value) -> Self {
            Self {
                status: StatusCode::OK,
                body,
                delay: Duration::from_millis(0),
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }

        fn failure(status: StatusCode, body: Value) -> Self {
            Self {
                status,
                body,
                delay: Duration::from_millis(0),
            }
        }
    }

    #[derive(Clone)]
    struct TokenServerState {
        responses: Arc<Mutex<VecDeque<TokenResponseSpec>>>,
        requests: Arc<Mutex<Vec<TokenRequestRecord>>>,
    }

    struct TestTokenServer {
        base_url: String,
        state: TokenServerState,
    }

    impl TestTokenServer {
        fn token_url(&self) -> String {
            format!("{}/token", self.base_url)
        }

        async fn requests(&self) -> Vec<TokenRequestRecord> {
            self.state.requests.lock().await.clone()
        }
    }

    async fn start_token_server(responses: Vec<TokenResponseSpec>) -> TestTokenServer {
        async fn token_handler(
            State(state): State<TokenServerState>,
            Form(form): Form<HashMap<String, String>>,
        ) -> Response {
            let record = TokenRequestRecord {
                grant_type: form.get("grant_type").cloned().unwrap_or_default(),
                client_id: form.get("client_id").cloned().unwrap_or_default(),
                client_secret: form.get("client_secret").cloned().unwrap_or_default(),
                refresh_token: form.get("refresh_token").cloned(),
                scope: form.get("scope").cloned(),
            };
            state.requests.lock().await.push(record);

            let response = state
                .responses
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| TokenResponseSpec::failure(StatusCode::INTERNAL_SERVER_ERROR, json!({
                    "error": "no_more_responses"
                })));

            if !response.delay.is_zero() {
                tokio::time::sleep(response.delay).await;
            }

            (response.status, Json(response.body)).into_response()
        }

        let state = TokenServerState {
            responses: Arc::new(Mutex::new(VecDeque::from(responses))),
            requests: Arc::new(Mutex::new(Vec::new())),
        };

        let app = Router::new()
            .route("/token", post(token_handler))
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        TestTokenServer {
            base_url: format!("http://{}", address),
            state,
        }
    }

    fn test_config(token_url: String) -> OAuthTokenSourceConfig {
        OAuthTokenSourceConfig {
            header: "Authorization".to_string(),
            token_url,
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            scope: Some("read:all".to_string()),
            refresh_buffer_secs: 0,
        }
    }

    #[derive(Serialize)]
    struct JwtClaims {
        exp: u64,
        sub: &'static str,
    }

    fn jwt_with_exp(seconds_from_now: u64) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        encode(
            &Header::default(),
            &JwtClaims {
                exp: now + seconds_from_now,
                sub: "fetch-auth-test",
            },
            &EncodingKey::from_secret(b"fetch-auth-test-secret"),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn authorization_header_value_reuses_cached_token_before_expiry() {
        let server = start_token_server(vec![TokenResponseSpec::success(json!({
            "access_token": "first-token",
            "token_type": "Bearer",
            "expires_in": 3600
        }))])
        .await;

        let source = OAuthClientCredentialsTokenSource::new(
            Client::new(),
            test_config(server.token_url()),
        );

        let first = source.authorization_header_value().await.unwrap();
        let second = source.authorization_header_value().await.unwrap();

        assert_eq!(first, "Bearer first-token");
        assert_eq!(second, "Bearer first-token");

        let requests = server.requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].grant_type, "client_credentials");
        assert_eq!(requests[0].scope.as_deref(), Some("read:all"));
    }

    #[tokio::test]
    async fn authorization_header_value_uses_refresh_token_after_expiry() {
        let server = start_token_server(vec![
            TokenResponseSpec::success(json!({
                "access_token": "short-lived",
                "token_type": "Bearer",
                "expires_in": 1,
                "refresh_token": "refresh-1"
            })),
            TokenResponseSpec::success(json!({
                "access_token": "refreshed-token",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "refresh-2"
            })),
        ])
        .await;

        let source = OAuthClientCredentialsTokenSource::new(
            Client::new(),
            test_config(server.token_url()),
        );

        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer short-lived"
        );

        tokio::time::sleep(Duration::from_millis(1_100)).await;

        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer refreshed-token"
        );

        let requests = server.requests().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].grant_type, "refresh_token");
        assert_eq!(requests[1].refresh_token.as_deref(), Some("refresh-1"));
    }

    #[tokio::test]
    async fn authorization_header_value_preserves_refresh_token_when_refresh_response_omits_it() {
        let server = start_token_server(vec![
            TokenResponseSpec::success(json!({
                "access_token": "short-lived",
                "token_type": "Bearer",
                "expires_in": 1,
                "refresh_token": "refresh-1"
            })),
            TokenResponseSpec::success(json!({
                "access_token": "refreshed-without-new-refresh",
                "token_type": "Bearer",
                "expires_in": 1
            })),
            TokenResponseSpec::success(json!({
                "access_token": "refreshed-again",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "refresh-2"
            })),
        ])
        .await;

        let source = OAuthClientCredentialsTokenSource::new(
            Client::new(),
            test_config(server.token_url()),
        );

        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer short-lived"
        );

        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer refreshed-without-new-refresh"
        );

        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer refreshed-again"
        );

        let requests = server.requests().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1].grant_type, "refresh_token");
        assert_eq!(requests[1].refresh_token.as_deref(), Some("refresh-1"));
        assert_eq!(requests[2].grant_type, "refresh_token");
        assert_eq!(requests[2].refresh_token.as_deref(), Some("refresh-1"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn authorization_header_value_replays_shared_failure_without_retry_fan_out() {
        let server = start_token_server(vec![
            TokenResponseSpec::success(json!({
                "access_token": "short-lived",
                "token_type": "Bearer",
                "expires_in": 1
            })),
            TokenResponseSpec::failure(
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "invalid_grant"
                }),
            )
            .with_delay(Duration::from_millis(150)),
        ])
        .await;

        let source = Arc::new(OAuthClientCredentialsTokenSource::new(
            Client::new(),
            test_config(server.token_url()),
        ));

        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer short-lived"
        );

        tokio::time::sleep(Duration::from_millis(1_100)).await;

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let source = source.clone();
            tasks.push(tokio::spawn(async move {
                source.authorization_header_value().await.unwrap_err()
            }));
        }

        let results = futures::future::join_all(tasks).await;
        for result in results {
            let error = result.unwrap();
            assert!(error.contains("invalid_grant"), "unexpected error: {error}");
        }

        let requests = server.requests().await;
        assert_eq!(requests.len(), 2);
    }

    #[tokio::test]
    async fn authorization_header_value_reacquires_after_refresh_failure() {
        let server = start_token_server(vec![
            TokenResponseSpec::success(json!({
                "access_token": "short-lived",
                "token_type": "Bearer",
                "expires_in": 1,
                "refresh_token": "refresh-1"
            })),
            TokenResponseSpec::failure(StatusCode::BAD_REQUEST, json!({
                "error": "invalid_grant"
            })),
            TokenResponseSpec::success(json!({
                "access_token": "reacquired-token",
                "token_type": "Bearer",
                "expires_in": 3600
            })),
        ])
        .await;

        let source = OAuthClientCredentialsTokenSource::new(
            Client::new(),
            test_config(server.token_url()),
        );

        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer short-lived"
        );

        tokio::time::sleep(Duration::from_millis(1_100)).await;

        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer reacquired-token"
        );

        let requests = server.requests().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].grant_type, "client_credentials");
        assert_eq!(requests[1].grant_type, "refresh_token");
        assert_eq!(requests[2].grant_type, "client_credentials");
    }

    #[tokio::test]
    async fn authorization_header_value_uses_jwt_exp_when_expires_in_is_missing() {
        let jwt = jwt_with_exp(2);
        let server = start_token_server(vec![
            TokenResponseSpec::success(json!({
                "access_token": jwt,
                "token_type": "Bearer"
            })),
            TokenResponseSpec::success(json!({
                "access_token": "jwt-fallback-refreshed",
                "token_type": "Bearer",
                "expires_in": 3600
            })),
        ])
        .await;

        let mut config = test_config(server.token_url());
        config.scope = None;
        let source = OAuthClientCredentialsTokenSource::new(Client::new(), config);

        let first = source.authorization_header_value().await.unwrap();
        tokio::time::sleep(Duration::from_millis(2_100)).await;
        let second = source.authorization_header_value().await.unwrap();

        assert!(first.starts_with("Bearer eyJ"));
        assert_eq!(second, "Bearer jwt-fallback-refreshed");

        let requests = server.requests().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].grant_type, "client_credentials");
        assert_eq!(requests[1].grant_type, "client_credentials");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn authorization_header_value_coalesces_concurrent_reacquire_requests() {
        let server = start_token_server(vec![
            TokenResponseSpec::success(json!({
                "access_token": "short-lived",
                "token_type": "Bearer",
                "expires_in": 1
            })),
            TokenResponseSpec::success(json!({
                "access_token": "shared-token",
                "token_type": "Bearer",
                "expires_in": 3600
            }))
            .with_delay(Duration::from_millis(150)),
        ])
        .await;

        let source = Arc::new(OAuthClientCredentialsTokenSource::new(
            Client::new(),
            test_config(server.token_url()),
        ));

        assert_eq!(
            source.authorization_header_value().await.unwrap(),
            "Bearer short-lived"
        );

        tokio::time::sleep(Duration::from_millis(1_100)).await;

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let source = source.clone();
            tasks.push(tokio::spawn(async move {
                source.authorization_header_value().await.unwrap()
            }));
        }

        let results = futures::future::join_all(tasks).await;
        for result in results {
            assert_eq!(result.unwrap(), "Bearer shared-token");
        }

        let requests = server.requests().await;
        assert_eq!(requests.len(), 2);
    }

    #[tokio::test]
    async fn stale_waiter_from_old_future_observes_newer_state_before_returning() {
        let source = OAuthClientCredentialsTokenSource::new(
            Client::new(),
            OAuthTokenSourceConfig {
                header: "Authorization".to_string(),
                token_url: "http://127.0.0.1/unused".to_string(),
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                scope: None,
                refresh_buffer_secs: 0,
            },
        );

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let older_token = CachedToken {
            access_token: "older-token".to_string(),
            token_type: "Bearer".to_string(),
            refresh_token: Some("older-refresh".to_string()),
            expires_at: SystemTime::now() + Duration::from_secs(60),
        };

        let old_future = async move {
            let _ = entered_tx.send(());
            release_rx.await.unwrap()
        }
        .boxed()
        .shared();

        {
            let mut state = source.state.lock().await;
            state_set_in_flight_for_test(&mut state, 0, old_future);
        }

        let source_for_waiter = source.clone();
        let waiter = tokio::spawn(async move {
            source_for_waiter.authorization_header_value().await.unwrap()
        });

        entered_rx.await.unwrap();

        let newer_token = CachedToken {
            access_token: "newer-token".to_string(),
            token_type: "Bearer".to_string(),
            refresh_token: Some("newer-refresh".to_string()),
            expires_at: SystemTime::now() + Duration::from_secs(120),
        };
        let newer_future = futures::future::pending::<Result<CachedToken, String>>()
            .boxed()
            .shared();

        let mut guard = source.state.lock().await;
        release_tx.send(Ok(older_token.clone())).unwrap();
        state_set_cached_token_for_test(&mut guard, newer_token.clone());
        state_set_in_flight_for_test(&mut guard, 1, newer_future.clone());
        drop(guard);

        assert_eq!(waiter.await.unwrap(), "Bearer newer-token");

        let state = source.state.lock().await;
        let cached = state.cached_token.as_ref().expect("newer cached token should remain");
        assert_eq!(cached.access_token, "newer-token");
        assert!(state.in_flight.is_some(), "newer in-flight future should remain");
    }

    #[test]
    fn oauth_token_source_debug_redacts_client_secret() {
        let source = OAuthClientCredentialsTokenSource::new(
            Client::new(),
            OAuthTokenSourceConfig {
                header: "Authorization".to_string(),
                token_url: "https://issuer.example/token".to_string(),
                client_id: "client-id".to_string(),
                client_secret: "super-secret".to_string(),
                scope: Some("read:all".to_string()),
                refresh_buffer_secs: 30,
            },
        );

        let debug_output = format!("{source:?}");

        assert!(debug_output.contains("<redacted>"));
        assert!(!debug_output.contains("super-secret"));
    }

    fn state_set_cached_token_for_test(state: &mut TokenSourceState, token: CachedToken) {
        state.cached_token = Some(token);
    }

    fn state_set_in_flight_for_test(
        state: &mut TokenSourceState,
        generation: u64,
        future: SharedTokenFuture,
    ) {
        state.in_flight = Some(InFlightRequest { generation, future });
        state.next_generation = generation + 1;
    }
}
