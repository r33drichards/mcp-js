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

/// Verifies OAuth bearer tokens by calling a userinfo endpoint (e.g. GitHub's
/// /user API). Extracts user claims from the response and makes them available
/// for OPA policy evaluation via mcp_headers.
pub struct OAuthTokenVerifier {
    userinfo_url: String,
    /// JSON key in the userinfo response to use as the `sub` claim.
    /// For GitHub this is "id" (numeric user ID).
    sub_key: String,
    client: reqwest::Client,
}

impl OAuthTokenVerifier {
    pub fn new(userinfo_url: String, sub_key: Option<String>) -> Self {
        Self {
            userinfo_url,
            sub_key: sub_key.unwrap_or_else(|| "id".to_string()),
            client: reqwest::Client::new(),
        }
    }

    /// Validate a bearer token by calling the userinfo endpoint.
    /// Returns a JSON map of claims on success, or None if the token is invalid.
    pub async fn verify(&self, token: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
        let resp = self.client
            .get(&self.userinfo_url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "mcp-js")
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "OAuth userinfo request failed");
            return None;
        }

        let body: serde_json::Value = resp.json().await.ok()?;
        let obj = body.as_object()?;

        // Build claims map with `sub` derived from the configured key
        let mut claims = serde_json::Map::new();
        if let Some(sub_val) = obj.get(&self.sub_key) {
            let sub_str = match sub_val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            claims.insert("sub".to_string(), serde_json::Value::String(sub_str));
        }
        // Also include login if present (useful for display/debugging)
        if let Some(login) = obj.get("login") {
            claims.insert("login".to_string(), login.clone());
        }

        Some(claims)
    }
}
