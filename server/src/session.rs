use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use jsonwebtoken::jwk::JwkSet;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Marker for a successful JWT verification.

/// Cached JWKS key store that fetches public keys from a JWKS endpoint.
pub struct JwksKeyStore {
    jwks_url: String,
    client: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
}

impl JwksKeyStore {
    /// Create a new key store and perform an initial fetch.
    pub async fn new(jwks_url: String) -> Result<Self, String> {
        let client = reqwest::Client::new();
        let keys = Self::fetch_keys_inner(&client, &jwks_url).await?;
        Ok(Self {
            jwks_url,
            client,
            keys: RwLock::new(keys),
        })
    }

    async fn fetch_keys_inner(
        client: &reqwest::Client,
        url: &str,
    ) -> Result<HashMap<String, DecodingKey>, String> {
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("JWKS fetch failed: {}", e))?;
        let jwks: JwkSet = resp
            .json()
            .await
            .map_err(|e| format!("JWKS parse failed: {}", e))?;

        let mut map = HashMap::new();
        for jwk in &jwks.keys {
            if let Some(ref kid) = jwk.common.key_id {
                match DecodingKey::from_jwk(jwk) {
                    Ok(dk) => { map.insert(kid.clone(), dk); }
                    Err(e) => {
                        tracing::warn!(kid, "Skipping JWK: {}", e);
                    }
                }
            }
        }
        if map.is_empty() {
            return Err("JWKS endpoint returned no usable keys".to_string());
        }
        tracing::info!("Loaded {} key(s) from JWKS endpoint", map.len());
        Ok(map)
    }

    /// Get a cached key by kid, or refresh from the JWKS endpoint on cache miss.
    async fn get_key(&self, kid: &str) -> Option<DecodingKey> {
        {
            let cache = self.keys.read().await;
            if let Some(dk) = cache.get(kid) {
                return Some(dk.clone());
            }
        }
        tracing::info!(kid, "Unknown kid, refreshing JWKS keys");
        match Self::fetch_keys_inner(&self.client, &self.jwks_url).await {
            Ok(new_keys) => {
                let result = new_keys.get(kid).cloned();
                *self.keys.write().await = new_keys;
                result
            }
            Err(e) => {
                tracing::error!("JWKS refresh failed: {}", e);
                None
            }
        }
    }
}

/// Verifies JWTs against a JWKS endpoint. All token claims are returned so
/// downstream policies (e.g. OPA filesystem policy) can enforce arbitrary
/// claim-based rules. Session identity is determined by the X-MCP-Session-Id
/// header, not by JWT claims.
pub struct SessionVerifier {
    key_store: Arc<JwksKeyStore>,
}

impl SessionVerifier {
    pub fn new(key_store: Arc<JwksKeyStore>) -> Self {
        Self { key_store }
    }

    /// Verify a JWT signature via JWKS. Returns true if the token is valid.
    pub async fn verify(&self, token: &str) -> bool {
        let Some(header) = decode_header(token).ok() else { return false };
        let Some(kid) = header.kid.as_deref() else { return false };
        let Some(dk) = self.key_store.get_key(kid).await else { return false };

        let alg = header.alg;
        let mut validation = Validation::new(alg);
        // Keycloak client_credentials tokens may omit exp; be permissive.
        validation.required_spec_claims = Default::default();
        // Don't validate audience — Keycloak may not set aud for service accounts.
        validation.validate_aud = false;

        decode::<serde_json::Value>(token, &dk, &validation).is_ok()
    }
}

// ── HTTP bearer-token enforcement ────────────────────────────────────────

/// Extract the session JWT from an incoming request. `Authorization: Bearer
/// <jwt>` takes precedence; otherwise the raw `agent-session` header is used
/// (matching the MCP header-capture precedence).
pub fn extract_session_token(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(bearer) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return Some(bearer.to_string());
    }
    headers
        .get("agent-session")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Axum middleware that rejects any request lacking a JWKS-verified bearer
/// token with `401 Unauthorized`. Installed only when a [`SessionVerifier`]
/// exists (i.e. `--jwks-url` / `JWKS_URL` is configured), so servers with no
/// JWKS configured are unaffected. CORS preflight (`OPTIONS`) is exempt so the
/// browser handshake is not blocked before the real (authenticated) request.
pub async fn enforce_bearer_auth(
    axum::extract::State(verifier): axum::extract::State<Arc<SessionVerifier>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if request.method() == axum::http::Method::OPTIONS {
        return next.run(request).await;
    }

    let authorized = match extract_session_token(request.headers()) {
        Some(token) => verifier.verify(&token).await,
        None => false,
    };

    if authorized {
        next.run(request).await
    } else {
        tracing::warn!(
            method = %request.method(),
            uri = %request.uri(),
            "rejecting request without a valid JWT (JWKS enforcement enabled)"
        );
        (
            axum::http::StatusCode::UNAUTHORIZED,
            [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
            axum::Json(serde_json::json!({
                "error": "unauthorized",
                "message": "a valid Bearer token is required",
            })),
        )
            .into_response()
    }
}

/// Wrap `router` with [`enforce_bearer_auth`] when a verifier is present;
/// otherwise return it unchanged. This is how `--jwks-url` turns from
/// audit-logging into hard enforcement on the HTTP transports.
pub fn apply_auth_enforcement(
    router: axum::Router,
    verifier: &Option<Arc<SessionVerifier>>,
) -> axum::Router {
    match verifier {
        Some(verifier) => router.layer(axum::middleware::from_fn_with_state(
            verifier.clone(),
            enforce_bearer_auth,
        )),
        None => router,
    }
}

#[cfg(test)]
mod tests {
    use super::extract_session_token;

    fn header_map(pairs: &[(&str, &str)]) -> axum::http::HeaderMap {
        let mut map = axum::http::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn extract_session_token_reads_bearer() {
        let headers = header_map(&[("authorization", "Bearer abc.def.ghi")]);
        assert_eq!(extract_session_token(&headers).as_deref(), Some("abc.def.ghi"));
    }

    #[test]
    fn extract_session_token_falls_back_to_agent_session() {
        let headers = header_map(&[("agent-session", "raw.jwt.token")]);
        assert_eq!(extract_session_token(&headers).as_deref(), Some("raw.jwt.token"));
    }

    #[test]
    fn extract_session_token_prefers_bearer_over_agent_session() {
        let headers = header_map(&[
            ("authorization", "Bearer from-bearer"),
            ("agent-session", "from-agent-session"),
        ]);
        assert_eq!(extract_session_token(&headers).as_deref(), Some("from-bearer"));
    }

    #[test]
    fn extract_session_token_none_when_absent_or_empty() {
        assert_eq!(extract_session_token(&header_map(&[])), None);
        // A bare "Bearer" with no token is not a token.
        assert_eq!(extract_session_token(&header_map(&[("authorization", "Bearer ")])), None);
        // A non-bearer Authorization scheme is ignored (no agent-session fallback present).
        assert_eq!(extract_session_token(&header_map(&[("authorization", "Basic xyz")])), None);
    }
}
