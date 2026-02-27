//! General-purpose Open Policy Agent (OPA) client.
//!
//! Evaluates policies via the OPA REST API. Designed to be reusable across
//! different intercepted operations (fetch, timers, etc.).

use serde::{Deserialize, Serialize};

/// Async OPA client. When called from a synchronous V8 callback inside
/// `spawn_blocking`, use `tokio::runtime::Handle::current().block_on()`
/// to bridge into the async runtime.
#[derive(Clone, Debug)]
pub struct OpaClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct OpaRequest<T: Serialize> {
    input: T,
}

#[derive(Deserialize)]
struct OpaResponse {
    result: Option<OpaResult>,
}

#[derive(Deserialize)]
struct OpaResult {
    allow: Option<bool>,
}

impl OpaClient {
    pub fn new(base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("Failed to create OPA HTTP client");
        Self { base_url, client }
    }

    /// Evaluate an OPA policy. Returns `Ok(true)` if the policy allows the
    /// operation, `Ok(false)` if denied, or `Err` on connectivity / parse errors.
    ///
    /// `policy_path` is appended to `/v1/data/` — e.g. `"mcp/fetch"` becomes
    /// `POST {base_url}/v1/data/mcp/fetch`.
    pub async fn evaluate<T: Serialize>(&self, policy_path: &str, input: &T) -> Result<bool, String> {
        let url = format!("{}/v1/data/{}", self.base_url.trim_end_matches('/'), policy_path);
        let body = OpaRequest { input };

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("OPA request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("OPA returned HTTP {}", resp.status()));
        }

        let opa_resp: OpaResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse OPA response: {}", e))?;

        Ok(opa_resp
            .result
            .and_then(|r| r.allow)
            .unwrap_or(false))
    }
}
