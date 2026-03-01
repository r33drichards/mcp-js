use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rand::{Rng, distributions::Alphanumeric};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

// ── OAuth types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes_supported: Option<Vec<String>>,
    pub response_types_supported: Vec<String>,
    pub grant_types_supported: Vec<String>,
    pub token_endpoint_auth_methods_supported: Vec<String>,
    pub code_challenge_methods_supported: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationRequest {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub grant_types: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    pub response_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationResponse {
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
}

// ── Internal types (built-in mode) ───────────────────────────────────────

#[derive(Debug, Clone)]
struct OAuthClient {
    _client_secret: Option<String>,
    redirect_uris: Vec<String>,
    _client_name: String,
}

#[derive(Debug, Clone)]
struct AuthSession {
    _client_id: String,
    redirect_uri: String,
    scope: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

// ── Configuration ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// The issuer URL advertised in metadata (the MCP server's own URL).
    pub issuer: String,
    /// External OIDC provider. When set, the server delegates auth to this provider.
    pub provider: Option<ExternalProviderConfig>,
}

#[derive(Debug, Clone)]
pub struct ExternalProviderConfig {
    /// Base URL of the OIDC provider (e.g. http://keycloak:8080/realms/mcp).
    pub url: String,
    /// Client ID used by the MCP server to call the introspection endpoint.
    pub client_id: String,
    /// Client secret used by the MCP server to call the introspection endpoint.
    pub client_secret: String,
}

// ── OAuth store ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct OAuthStore {
    config: OAuthConfig,
    http_client: reqwest::Client,
    // Built-in mode state (unused in external provider mode)
    clients: Arc<RwLock<HashMap<String, OAuthClient>>>,
    codes: Arc<RwLock<HashMap<String, AuthSession>>>,
    tokens: Arc<RwLock<HashMap<String, String>>>,
}

impl OAuthStore {
    pub fn new(config: OAuthConfig) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
            clients: Arc::new(RwLock::new(HashMap::new())),
            codes: Arc::new(RwLock::new(HashMap::new())),
            tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Validate a bearer token. In external mode, calls the provider's introspection endpoint.
    /// In built-in mode, checks the local token store.
    async fn validate_token(&self, token: &str) -> bool {
        if let Some(ref provider) = self.config.provider {
            self.introspect_token(provider, token).await
        } else {
            self.tokens.read().await.contains_key(token)
        }
    }

    /// Call the external provider's token introspection endpoint (RFC 7662).
    async fn introspect_token(&self, provider: &ExternalProviderConfig, token: &str) -> bool {
        let introspection_url = format!(
            "{}/protocol/openid-connect/token/introspect",
            provider.url
        );
        let resp = self
            .http_client
            .post(&introspection_url)
            .basic_auth(&provider.client_id, Some(&provider.client_secret))
            .form(&[("token", token)])
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                #[derive(Deserialize)]
                struct IntrospectionResponse {
                    active: bool,
                }
                match r.json::<IntrospectionResponse>().await {
                    Ok(body) => body.active,
                    Err(e) => {
                        tracing::warn!("OAuth introspection parse error: {}", e);
                        false
                    }
                }
            }
            Ok(r) => {
                tracing::warn!("OAuth introspection HTTP error: {}", r.status());
                false
            }
            Err(e) => {
                tracing::warn!("OAuth introspection request failed: {}", e);
                false
            }
        }
    }
}

fn generate_random_string(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// GET /.well-known/oauth-authorization-server
///
/// In external provider mode, returns the provider's OIDC endpoints.
/// In built-in mode, returns this server's own OAuth endpoints.
async fn metadata_handler(State(store): State<Arc<OAuthStore>>) -> impl IntoResponse {
    if let Some(ref provider) = store.config.provider {
        // External provider mode: point clients to the provider's endpoints
        let base = &provider.url;
        let metadata = AuthorizationMetadata {
            issuer: base.clone(),
            authorization_endpoint: format!(
                "{}/protocol/openid-connect/auth",
                base
            ),
            token_endpoint: format!(
                "{}/protocol/openid-connect/token",
                base
            ),
            registration_endpoint: Some(format!(
                "{}/clients-registrations/openid-connect",
                base
            )),
            scopes_supported: Some(vec!["openid".to_string(), "profile".to_string()]),
            response_types_supported: vec!["code".to_string()],
            grant_types_supported: vec!["authorization_code".to_string()],
            token_endpoint_auth_methods_supported: vec![
                "client_secret_post".to_string(),
                "client_secret_basic".to_string(),
                "none".to_string(),
            ],
            code_challenge_methods_supported: vec!["S256".to_string(), "plain".to_string()],
        };
        (StatusCode::OK, Json(metadata))
    } else {
        // Built-in mode
        let issuer = &store.config.issuer;
        let metadata = AuthorizationMetadata {
            issuer: issuer.clone(),
            authorization_endpoint: format!("{}/oauth/authorize", issuer),
            token_endpoint: format!("{}/oauth/token", issuer),
            registration_endpoint: Some(format!("{}/oauth/register", issuer)),
            scopes_supported: Some(vec!["mcp".to_string()]),
            response_types_supported: vec!["code".to_string()],
            grant_types_supported: vec!["authorization_code".to_string()],
            token_endpoint_auth_methods_supported: vec![
                "client_secret_post".to_string(),
                "none".to_string(),
            ],
            code_challenge_methods_supported: vec!["S256".to_string(), "plain".to_string()],
        };
        (StatusCode::OK, Json(metadata))
    }
}

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code_challenge: Option<String>,
    #[serde(default)]
    code_challenge_method: Option<String>,
}

/// GET /oauth/authorize  (built-in mode only)
async fn authorize_handler(
    Query(params): Query<AuthorizeQuery>,
    State(store): State<Arc<OAuthStore>>,
) -> impl IntoResponse {
    if params.response_type != "code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "unsupported_response_type",
                "error_description": "Only 'code' response_type is supported"
            })),
        )
            .into_response();
    }

    let clients = store.clients.read().await;
    let client = match clients.get(&params.client_id) {
        Some(c) => c.clone(),
        None => {
            tracing::warn!("OAuth: unknown client_id: {}", params.client_id);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "error_description": "Unknown client_id"
                })),
            )
                .into_response();
        }
    };
    drop(clients);

    if !client.redirect_uris.contains(&params.redirect_uri) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_request",
                "error_description": "redirect_uri does not match registered URIs"
            })),
        )
            .into_response();
    }

    let auth_code = format!("mcp-code-{}", generate_random_string(32));

    let session = AuthSession {
        _client_id: params.client_id,
        redirect_uri: params.redirect_uri.clone(),
        scope: params.scope,
        code_challenge: params.code_challenge,
        code_challenge_method: params.code_challenge_method,
    };

    store
        .codes
        .write()
        .await
        .insert(auth_code.clone(), session);

    let mut redirect_url = format!("{}?code={}", params.redirect_uri, auth_code);
    if let Some(state) = params.state {
        redirect_url.push_str(&format!("&state={}", state));
    }

    tracing::info!("OAuth: authorized, redirecting");
    axum::response::Redirect::to(&redirect_url).into_response()
}

#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    client_id: String,
    #[allow(dead_code)]
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    code_verifier: Option<String>,
}

/// POST /oauth/token  (built-in mode only)
async fn token_handler(
    State(store): State<Arc<OAuthStore>>,
    request: Request<Body>,
) -> impl IntoResponse {
    let bytes = match axum::body::to_bytes(request.into_body(), 1024 * 64).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "error_description": "Cannot read request body"
                })),
            )
                .into_response();
        }
    };

    let token_req: TokenRequest = match serde_urlencoded::from_bytes(&bytes) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "error_description": format!("Cannot parse form data: {}", e)
                })),
            )
                .into_response();
        }
    };

    if token_req.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "unsupported_grant_type",
                "error_description": "Only authorization_code is supported"
            })),
        )
            .into_response();
    }

    let session = match store.codes.write().await.remove(&token_req.code) {
        Some(s) => s,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_grant",
                    "error_description": "Invalid or expired authorization code"
                })),
            )
                .into_response();
        }
    };

    if !token_req.redirect_uri.is_empty() && token_req.redirect_uri != session.redirect_uri {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "redirect_uri mismatch"
            })),
        )
            .into_response();
    }

    // PKCE verification
    if let Some(ref challenge) = session.code_challenge {
        let verifier = match &token_req.code_verifier {
            Some(v) => v,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid_grant",
                        "error_description": "code_verifier required"
                    })),
                )
                    .into_response();
            }
        };

        let method = session
            .code_challenge_method
            .as_deref()
            .unwrap_or("plain");
        let valid = match method {
            "S256" => {
                let hash = Sha256::digest(verifier.as_bytes());
                let computed = base64url_encode(&hash);
                computed == *challenge
            }
            "plain" => verifier == challenge,
            _ => false,
        };

        if !valid {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_grant",
                    "error_description": "PKCE verification failed"
                })),
            )
                .into_response();
        }
    }

    let access_token = format!("mcp-at-{}", generate_random_string(48));
    store
        .tokens
        .write()
        .await
        .insert(access_token.clone(), token_req.client_id);

    let response = TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        refresh_token: None,
        scope: session.scope,
    };

    tracing::info!("OAuth: issued access token");
    (StatusCode::OK, Json(response)).into_response()
}

/// POST /oauth/register  (RFC 7591 Dynamic Client Registration, built-in mode only)
async fn register_handler(
    State(store): State<Arc<OAuthStore>>,
    Json(req): Json<ClientRegistrationRequest>,
) -> impl IntoResponse {
    if req.redirect_uris.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_request",
                "error_description": "At least one redirect_uri is required"
            })),
        )
            .into_response();
    }

    let client_id = format!("client-{}", Uuid::new_v4());
    let client_secret = generate_random_string(32);

    let client = OAuthClient {
        _client_secret: Some(client_secret.clone()),
        redirect_uris: req.redirect_uris.clone(),
        _client_name: req.client_name.clone(),
    };

    store
        .clients
        .write()
        .await
        .insert(client_id.clone(), client);

    let response = ClientRegistrationResponse {
        client_id,
        client_secret: Some(client_secret),
        client_name: req.client_name,
        redirect_uris: req.redirect_uris,
    };

    tracing::info!("OAuth: registered new client '{}'", response.client_id);
    (StatusCode::CREATED, Json(response)).into_response()
}

// ── Token validation middleware ──────────────────────────────────────────

pub async fn token_validation_middleware(
    State(store): State<Arc<OAuthStore>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let auth_header = request.headers().get("Authorization");
    let token = match auth_header {
        Some(header) => {
            let header_str = header.to_str().unwrap_or("");
            if let Some(stripped) = header_str.strip_prefix("Bearer ") {
                stripped.to_string()
            } else {
                tracing::debug!("OAuth: missing Bearer prefix in Authorization header");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
        None => {
            tracing::debug!("OAuth: missing Authorization header");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    if store.validate_token(&token).await {
        next.run(request).await
    } else {
        tracing::debug!("OAuth: invalid bearer token");
        StatusCode::UNAUTHORIZED.into_response()
    }
}

// ── Router builder ───────────────────────────────────────────────────────

/// Build an Axum router with OAuth endpoints.
///
/// In **external provider mode**, only the metadata discovery endpoint is served;
/// authorize/token/register are handled by the external provider directly.
///
/// In **built-in mode**, all OAuth endpoints are served locally.
pub fn oauth_router(store: Arc<OAuthStore>) -> Router {
    let mut router = Router::new().route(
        "/.well-known/oauth-authorization-server",
        get(metadata_handler),
    );

    // Only add built-in OAuth endpoints when not using an external provider
    if store.config.provider.is_none() {
        router = router
            .route("/oauth/authorize", get(authorize_handler))
            .route("/oauth/token", post(token_handler))
            .route("/oauth/register", post(register_handler));
    }

    router.with_state(store)
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Base64-URL-encode without padding (RFC 4648 section 5), used for PKCE S256.
fn base64url_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() { data[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        encoded.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        encoded.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if i + 1 < data.len() {
            encoded.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        }
        if i + 2 < data.len() {
            encoded.push(CHARS[(triple & 0x3F) as usize] as char);
        }
        i += 3;
    }
    encoded
}
