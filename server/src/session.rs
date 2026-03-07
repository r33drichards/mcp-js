use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use jsonwebtoken::jwk::JwkSet;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Result of a successful JWT verification: all token claims.
pub struct VerifiedSession {
    pub claims: serde_json::Value,
}

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

    /// Verify a JWT signature via JWKS and return all claims for policy evaluation.
    pub async fn verify(&self, token: &str) -> Option<VerifiedSession> {
        let header = decode_header(token).ok()?;
        let kid = header.kid.as_deref()?;
        let dk = self.key_store.get_key(kid).await?;

        let alg = header.alg;
        let mut validation = Validation::new(alg);
        // Keycloak client_credentials tokens may omit exp; be permissive.
        validation.required_spec_claims = Default::default();
        // Don't validate audience — Keycloak may not set aud for service accounts.
        validation.validate_aud = false;

        let token_data = decode::<serde_json::Value>(token, &dk, &validation).ok()?;

        Some(VerifiedSession {
            claims: token_data.claims,
        })
    }
}
