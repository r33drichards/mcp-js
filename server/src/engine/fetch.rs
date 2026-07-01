//! OPA-gated `fetch()` implementation for the JavaScript runtime.
//!
//! Uses a deno_core async op (`op_fetch`) to perform truly non-blocking HTTP
//! requests. Each request is evaluated against an OPA policy before execution.
//!
//! The `fetch()` function follows the web standard Fetch API:
//! ```js
//! const resp = await fetch("https://example.com");
//! resp.ok              // boolean
//! resp.status          // number
//! resp.statusText      // string
//! resp.url             // string
//! resp.headers         // Headers object with .get(name)
//! await resp.text()    // body as string (Promise)
//! await resp.json()    // parsed JSON (Promise)
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;

use super::fetch_auth::{OAuthClientCredentialsTokenSource, OAuthTokenSourceConfig};
use super::opa::PolicyChain;

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for the fetch() function, including policy settings.
/// Stored in deno_core's `OpState` for access from async ops.
#[derive(Clone, Debug)]
pub struct FetchConfig {
    pub policy_chain: Arc<PolicyChain>,
    pub http_client: reqwest::Client,
    pub header_rules: Vec<HeaderRule>,
}

impl FetchConfig {
    /// Create from a [`PolicyChain`] (used with `--policies-json`).
    pub fn new_with_chain(chain: Arc<PolicyChain>) -> Self {
        let http_client = build_fetch_http_client();
        Self {
            policy_chain: chain,
            http_client,
            header_rules: Vec::new(),
        }
    }

    pub fn with_header_rules(mut self, rules: Vec<HeaderRule>) -> Self {
        self.header_rules = rules;
        self
    }
}

pub fn default_refresh_buffer_secs() -> u64 {
    30
}

#[derive(Clone, PartialEq, Eq)]
pub struct OAuthClientCredentialsConfig {
    pub header_name: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: Option<String>,
    pub refresh_buffer_secs: u64,
}

impl std::fmt::Debug for OAuthClientCredentialsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClientCredentialsConfig")
            .field("header_name", &self.header_name)
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scope", &self.scope)
            .field("refresh_buffer_secs", &self.refresh_buffer_secs)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderInjection {
    Static {
        headers: HashMap<String, String>,
    },
    OAuthClientCredentials(OAuthClientCredentialsConfig),
}

/// A rule for injecting headers into outgoing fetch requests.
#[derive(Clone, Debug)]
pub struct HeaderRule {
    /// Host to match (exact match or leading-wildcard like "*.github.com"). Case-insensitive.
    pub host: String,
    /// HTTP methods to match (e.g., ["GET", "POST"]). Empty means all methods.
    pub methods: Vec<String>,
    /// Injection strategy for matching requests.
    pub injection: HeaderInjection,
    dynamic_auth_source: Option<Arc<OAuthClientCredentialsTokenSource>>,
}

impl HeaderRule {
    pub fn new(host: String, methods: Vec<String>, injection: HeaderInjection) -> Result<Self> {
        let host = require_non_empty("host", host)?;
        let injection = normalize_injection(injection)?;
        let dynamic_auth_source = build_dynamic_auth_source(&injection);

        Ok(Self {
            host,
            methods: normalize_methods(methods),
            injection,
            dynamic_auth_source,
        })
    }

    pub fn static_header(
        host: String,
        methods: Vec<String>,
        header_name: String,
        header_value: String,
    ) -> Result<Self> {
        let mut headers = HashMap::new();
        headers.insert(
            require_non_empty("header", header_name)?,
            require_non_empty("value", header_value)?,
        );
        Self::new(host, methods, HeaderInjection::Static { headers })
    }

    pub fn oauth_client_credentials(
        host: String,
        methods: Vec<String>,
        config: OAuthClientCredentialsConfig,
    ) -> Result<Self> {
        Self::new(
            host,
            methods,
            HeaderInjection::OAuthClientCredentials(config),
        )
    }

    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    pub fn static_headers(&self) -> Option<&HashMap<String, String>> {
        match &self.injection {
            HeaderInjection::Static { headers } => Some(headers),
            HeaderInjection::OAuthClientCredentials(_) => None,
        }
    }

    pub fn dynamic_auth(&self) -> Option<&OAuthClientCredentialsConfig> {
        match &self.injection {
            HeaderInjection::Static { .. } => None,
            HeaderInjection::OAuthClientCredentials(config) => Some(config),
        }
    }

    fn matches(&self, request_host: &str, request_method: &str) -> bool {
        if !self.methods.is_empty() {
            let method_upper = request_method.to_uppercase();
            if !self.methods.iter().any(|m| m.eq_ignore_ascii_case(&method_upper)) {
                return false;
            }
        }

        let pattern = self.host.to_lowercase();
        let host = request_host.to_lowercase();

        if let Some(suffix) = pattern.strip_prefix('*') {
            // "*.github.com" matches "api.github.com" and "github.com"
            host == pattern[2..] || host.ends_with(suffix)
        } else {
            host == pattern
        }
    }
}

impl PartialEq for HeaderRule {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host
            && self.methods == other.methods
            && self.injection == other.injection
    }
}

impl Eq for HeaderRule {}

fn require_non_empty(field: &str, value: String) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("fetch rule '{}' cannot be empty", field);
    }
    Ok(trimmed.to_string())
}

fn normalize_injection(injection: HeaderInjection) -> Result<HeaderInjection> {
    match injection {
        HeaderInjection::Static { headers } => Ok(HeaderInjection::Static {
            headers: normalize_static_headers(headers)?,
        }),
        HeaderInjection::OAuthClientCredentials(config) => Ok(
            HeaderInjection::OAuthClientCredentials(normalize_oauth_config(config)?),
        ),
    }
}

fn normalize_static_headers(headers: HashMap<String, String>) -> Result<HashMap<String, String>> {
    if headers.is_empty() {
        anyhow::bail!("fetch rule static headers cannot be empty");
    }

    let mut normalized = HashMap::with_capacity(headers.len());
    let mut seen_names = HashMap::with_capacity(headers.len());

    for (header_name, header_value) in headers {
        let normalized_name = require_non_empty("header", header_name)?;
        let normalized_value = require_non_empty("value", header_value)?;
        let normalized_name_key = normalized_name.to_ascii_lowercase();

        if let Some(existing_name) = seen_names.insert(normalized_name_key, normalized_name.clone()) {
            anyhow::bail!(
                "fetch rule duplicate static header name '{}' conflicts with '{}'",
                normalized_name,
                existing_name
            );
        }

        normalized.insert(normalized_name, normalized_value);
    }

    Ok(normalized)
}

fn normalize_oauth_config(config: OAuthClientCredentialsConfig) -> Result<OAuthClientCredentialsConfig> {
    Ok(OAuthClientCredentialsConfig {
        header_name: require_non_empty("header", config.header_name)?,
        token_url: require_non_empty("token_url", config.token_url)?,
        client_id: require_non_empty("client_id", config.client_id)?,
        client_secret: require_non_empty("client_secret", config.client_secret)?,
        scope: config.scope.map(|scope| scope.trim().to_string()).filter(|scope| !scope.is_empty()),
        refresh_buffer_secs: config.refresh_buffer_secs,
    })
}

fn build_fetch_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to create fetch HTTP client")
}

fn build_dynamic_auth_source(
    injection: &HeaderInjection,
) -> Option<Arc<OAuthClientCredentialsTokenSource>> {
    match injection {
        HeaderInjection::Static { .. } => None,
        HeaderInjection::OAuthClientCredentials(config) => Some(Arc::new(
            OAuthClientCredentialsTokenSource::new(
                build_fetch_http_client(),
                OAuthTokenSourceConfig {
                    header: config.header_name.clone(),
                    token_url: config.token_url.clone(),
                    client_id: config.client_id.clone(),
                    client_secret: config.client_secret.clone(),
                    scope: config.scope.clone(),
                    refresh_buffer_secs: config.refresh_buffer_secs,
                },
            ),
        )),
    }
}

fn sanitized_dynamic_auth_error(header_name: &str, stage: &str) -> String {
    format!(
        "dynamic credential injection failed for header '{}' during {}",
        header_name, stage
    )
}

fn normalize_methods(methods: Vec<String>) -> Vec<String> {
    methods
        .into_iter()
        .map(|method| method.trim().to_uppercase())
        .filter(|method| !method.is_empty())
        .collect()
}

/// Apply header injection rules. User-provided headers take precedence.
pub async fn apply_header_rules(
    rules: &[HeaderRule],
    host: &str,
    method: &str,
    headers: &mut HashMap<String, String>,
) -> Result<(), String> {
    for rule in rules {
        if !rule.matches(host, method) {
            continue;
        }

        match &rule.injection {
            HeaderInjection::Static { headers: rule_headers } => {
                for (k, v) in rule_headers {
                    let key = k.to_ascii_lowercase();
                    headers.entry(key).or_insert_with(|| v.clone());
                }
            }
            HeaderInjection::OAuthClientCredentials(config) => {
                let key = config.header_name.to_ascii_lowercase();
                if headers.contains_key(&key) {
                    continue;
                }

                let source = rule.dynamic_auth_source.as_ref().ok_or_else(|| {
                    sanitized_dynamic_auth_error(&config.header_name, "source initialization")
                })?;
                let value = source
                    .authorization_header_value()
                    .await
                    .map_err(|_| sanitized_dynamic_auth_error(&config.header_name, "token acquisition"))?;
                headers.insert(key, value);
            }
        }
    }

    Ok(())
}

// ── OPA policy input ─────────────────────────────────────────────────────

#[derive(Serialize)]
struct FetchPolicyInput {
    operation: &'static str,
    url: String,
    method: String,
    headers: HashMap<String, String>,
    url_parsed: UrlParsed,
}

#[derive(Serialize)]
struct UrlParsed {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
    query: String,
}

// ── Async deno_core op ──────────────────────────────────────────────────

/// Async op: performs an OPA-gated HTTP fetch. Called from JS via
/// `Deno.core.ops.op_fetch(url, method, headersJson, body)`.
/// Returns a JSON string with {status, statusText, url, headers, body}.
#[op2(async)]
#[string]
async fn op_fetch(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
    #[string] method: String,
    #[string] headers_json: String,
    #[string] body: String,
) -> Result<String, JsErrorBox> {
    // Clone config from OpState before any .await (Rc is !Send).
    let (policy_chain, http_client, header_rules) = {
        let state = state.borrow();
        let config = state.try_borrow::<FetchConfig>()
            .ok_or_else(|| JsErrorBox::generic("fetch: internal error — no fetch config available"))?;
        (
            config.policy_chain.clone(),
            config.http_client.clone(),
            config.header_rules.clone(),
        )
    };

    // Convert empty string (from JS null body) to None.
    let body = if body.is_empty() { None } else { Some(body) };

    // Spawn on a separate tokio task so deno_core's op driver only sees a
    // simple JoinHandle future. Without this, the deeply nested async state
    // machine from PolicyChain → PolicyEvaluatorKind → reqwest triggers a
    // RefCell re-entrancy panic in deno_core's FuturesUnorderedDriver on
    // some Rust toolchains (observed with stable, not nightly).
    tokio::spawn(async move {
        do_fetch(url, method, headers_json, body, policy_chain, http_client, header_rules).await
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fetch task join error: {}", e)))?
    .map_err(|e| JsErrorBox::generic(e))
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    fetch_ext,
    ops = [op_fetch],
);

/// Create the fetch extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    fetch_ext::init()
}

// ── Inject fetch() JS wrapper into the global scope ─────────────────────

/// Inject the `globalThis.fetch` JS wrapper. Must be called after the
/// runtime is created (with the fetch extension) but before user code runs.
pub fn inject_fetch(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<fetch-setup>", FETCH_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install fetch wrapper: {}", e))?;
    Ok(())
}

/// JavaScript wrapper that provides the web-standard Fetch API.
/// The async op `Deno.core.ops.op_fetch(url, method, headersJson, body)`
/// returns a Promise<string> with JSON {status, statusText, url, headers, body}.
/// This wrapper parses it into a proper Response-like object.
const FETCH_JS_WRAPPER: &str = r#"
(function() {
    function Headers(init) {
        this._map = {};
        if (init) {
            for (const key of Object.keys(init)) {
                this._map[key.toLowerCase()] = init[key];
            }
        }
    }
    Headers.prototype.get = function(name) {
        return this._map[name.toLowerCase()] || null;
    };
    Headers.prototype.has = function(name) {
        return name.toLowerCase() in this._map;
    };
    Headers.prototype.set = function(name, value) {
        this._map[name.toLowerCase()] = String(value);
    };
    Headers.prototype.append = function(name, value) {
        const key = name.toLowerCase();
        if (key in this._map) {
            this._map[key] += ', ' + String(value);
        } else {
            this._map[key] = String(value);
        }
    };
    Headers.prototype.delete = function(name) {
        delete this._map[name.toLowerCase()];
    };
    Headers.prototype.entries = function() {
        return Object.entries(this._map);
    };
    Headers.prototype.keys = function() {
        return Object.keys(this._map);
    };
    Headers.prototype.values = function() {
        return Object.values(this._map);
    };
    Headers.prototype.forEach = function(cb) {
        for (const [k, v] of Object.entries(this._map)) {
            cb(v, k, this);
        }
    };

    globalThis.fetch = async function fetch(resource, init) {
        if (typeof resource !== 'string') {
            throw new TypeError('fetch: first argument must be a URL string');
        }

        const opts = init || {};
        const method = (opts.method || 'GET').toUpperCase();
        const headers = opts.headers || {};

        // Normalize headers to plain object with lowercase keys
        const normalizedHeaders = {};
        if (headers && typeof headers === 'object') {
            if (typeof headers.entries === 'function' && headers instanceof Headers) {
                for (const [k, v] of headers.entries()) {
                    normalizedHeaders[k] = v;
                }
            } else {
                for (const key of Object.keys(headers)) {
                    normalizedHeaders[key.toLowerCase()] = String(headers[key]);
                }
            }
        }

        // Request/response bodies cross the op boundary base64-encoded so binary
        // payloads (e.g. git smart-HTTP packfiles) are preserved.
        function _b64FromBytes(bytes) {
            let bin = '';
            const CHUNK = 0x8000;
            for (let i = 0; i < bytes.length; i += CHUNK) {
                bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
            }
            return btoa(bin);
        }
        function _bytesFromB64(b64) {
            const bin = atob(b64 || '');
            const out = new Uint8Array(bin.length);
            for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i) & 0xff;
            return out;
        }

        let body = "";
        if (opts.body !== undefined && opts.body !== null) {
            let bytes;
            if (typeof FormData !== 'undefined' && opts.body instanceof FormData) {
                const serialized = opts.body._serialize();
                bytes = new TextEncoder().encode(serialized.body);
                if (!('content-type' in normalizedHeaders)) {
                    normalizedHeaders['content-type'] = 'multipart/form-data; boundary=' + serialized.boundary;
                }
            } else if (opts.body instanceof Uint8Array) {
                bytes = opts.body;
            } else if (ArrayBuffer.isView(opts.body)) {
                bytes = new Uint8Array(opts.body.buffer, opts.body.byteOffset, opts.body.byteLength);
            } else if (opts.body instanceof ArrayBuffer) {
                bytes = new Uint8Array(opts.body);
            } else {
                bytes = new TextEncoder().encode(String(opts.body));
            }
            body = _b64FromBytes(bytes);
        }

        const headersJson = JSON.stringify(normalizedHeaders);

        // Async op — truly non-blocking, returns a Promise
        const rawResult = await Deno.core.ops.op_fetch(resource, method, headersJson, body);
        const result = JSON.parse(rawResult);

        // result.body is base64 (bodyEncoding: "base64"); decode once to bytes.
        const responseBytes = _bytesFromB64(result.body);
        const responseHeaders = new Headers(result.headers);
        const decodeText = function() { return new TextDecoder().decode(responseBytes); };
        const toArrayBuffer = function() {
            return responseBytes.buffer.slice(
                responseBytes.byteOffset,
                responseBytes.byteOffset + responseBytes.byteLength
            );
        };
        const mkResponse = function() {
            return {
                ok: result.status >= 200 && result.status < 300,
                status: result.status,
                statusText: result.statusText,
                url: result.url,
                headers: responseHeaders,
                redirected: result.redirected || false,
                type: 'basic',
                bodyUsed: false,
                text: function() { return Promise.resolve(decodeText()); },
                json: function() { return Promise.resolve(JSON.parse(decodeText())); },
                arrayBuffer: function() { return Promise.resolve(toArrayBuffer()); },
                bytes: function() { return Promise.resolve(new Uint8Array(responseBytes)); },
                blob: function() { return Promise.resolve(new Blob([decodeText()], { type: responseHeaders.get('content-type') || '' })); },
                clone: function() { return mkResponse(); },
            };
        };
        return mkResponse();
    };
})();
"#;

// ── base64 (binary-safe body transfer over the op boundary) ─────────────
//
// Request and response bodies cross the JS↔Rust op boundary as base64 so that
// binary payloads (e.g. git's smart-HTTP packfiles) survive intact — a plain
// String body would be mangled by UTF-8 (de)coding. Standard RFC 4648 alphabet
// with padding; the JS side uses btoa/atob, which match.

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_TABLE[((n >> 18) & 63) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { B64_TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64_TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut buf = 0u32;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for &c in s.as_bytes() {
        match c {
            b'=' => break,
            b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => {}
        }
        let v = val(c).ok_or_else(|| "fetch: invalid base64 in body".to_string())?;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

// ── Pure-Rust fetch implementation (no V8 types) ─────────────────────────

/// Execute a policy-gated HTTP fetch. All V8 interaction happens in the caller.
async fn do_fetch(
    url_str: String,
    method: String,
    headers_json: String,
    body: Option<String>,
    policy_chain: Arc<PolicyChain>,
    http_client: reqwest::Client,
    header_rules: Vec<HeaderRule>,
) -> Result<String, String> {
    let mut headers: HashMap<String, String> = serde_json::from_str(&headers_json)
        .map_err(|e| format!("fetch: invalid headers JSON: {}", e))?;

    // Parse URL into components for policy input
    let parsed_url = url::Url::parse(&url_str)
        .map_err(|e| format!("fetch: invalid URL '{}': {}", url_str, e))?;

    let url_host = parsed_url.host_str().unwrap_or("").to_string();

    // Apply header injection rules. User-provided headers take precedence.
    apply_header_rules(&header_rules, &url_host, &method, &mut headers)
        .await
        .map_err(|e| format!("fetch: credential injection failed for host '{}': {}", url_host, e))?;

    let url_parsed = UrlParsed {
        scheme: parsed_url.scheme().to_string(),
        host: url_host,
        port: parsed_url.port(),
        path: parsed_url.path().to_string(),
        query: parsed_url.query().unwrap_or("").to_string(),
    };

    let policy_input = FetchPolicyInput {
        operation: "fetch",
        url: url_str.clone(),
        method: method.clone(),
        headers: headers.clone(),
        url_parsed,
    };

    // Evaluate policy chain.
    let input_value = serde_json::to_value(&policy_input)
        .map_err(|e| format!("fetch: failed to serialize policy input: {}", e))?;
    let allowed = policy_chain.evaluate(&input_value).await?;
    if !allowed {
        return Err(format!(
            "fetch denied by policy: {} {} is not allowed",
            method, url_str
        ));
    }

    // Execute the HTTP request
    let mut req_builder = http_client.request(
        method
            .parse::<reqwest::Method>()
            .map_err(|e| format!("fetch: invalid method '{}': {}", method, e))?,
        &url_str,
    );

    for (k, v) in &headers {
        req_builder = req_builder.header(k.as_str(), v.as_str());
    }

    if let Some(body) = body {
        // The JS wrapper sends the request body base64-encoded so binary
        // payloads (git packfiles, etc.) are preserved.
        let body_bytes = b64_decode(&body)?;
        req_builder = req_builder.body(body_bytes);
    }

    let resp = req_builder
        .send()
        .await
        .map_err(|e| format!("fetch: request failed: {}", e))?;

    let status = resp.status().as_u16();
    let status_text = resp
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_string();
    let final_url = resp.url().to_string();

    let resp_headers: HashMap<String, String> = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                v.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    // Read the response body as raw bytes and hand it back base64-encoded, so
    // binary responses (git smart-HTTP, images, …) survive the op boundary.
    let resp_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("fetch: failed to read response body: {}", e))?;
    let resp_body = b64_encode(&resp_bytes);

    let result = serde_json::json!({
        "status": status,
        "statusText": status_text,
        "url": final_url,
        "headers": resp_headers,
        "body": resp_body,
        "bodyEncoding": "base64",
        "redirected": final_url != url_str,
    });

    Ok(result.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;

    use axum::extract::{Form, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use crate::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyEvaluatorKind};

    #[derive(Clone)]
    struct TestTokenServer {
        base_url: String,
        state: TestTokenServerState,
    }

    impl TestTokenServer {
        fn token_url(&self) -> String {
            format!("{}/token", self.base_url)
        }

        async fn requests(&self) -> Vec<TestTokenRequest> {
            self.state.requests.lock().await.clone()
        }
    }

    #[derive(Clone)]
    struct TestTokenServerState {
        responses: Arc<tokio::sync::Mutex<VecDeque<TestTokenResponse>>>,
        requests: Arc<tokio::sync::Mutex<Vec<TestTokenRequest>>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TestTokenResponse {
        status: StatusCode,
        body: Value,
    }

    impl TestTokenResponse {
        fn success(body: Value) -> Self {
            Self {
                status: StatusCode::OK,
                body,
            }
        }

        fn failure(status: StatusCode, body: Value) -> Self {
            Self { status, body }
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct TestTokenRequest {
        grant_type: String,
    }

    async fn start_token_server(responses: Vec<TestTokenResponse>) -> TestTokenServer {
        async fn token_handler(
            State(state): State<TestTokenServerState>,
            Form(form): Form<HashMap<String, String>>,
        ) -> Response {
            state.requests.lock().await.push(TestTokenRequest {
                grant_type: form.get("grant_type").cloned().unwrap_or_default(),
            });

            let response = state
                .responses
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| {
                    TestTokenResponse::failure(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({"error":"no_more_responses"}),
                    )
                });

            (response.status, Json(response.body)).into_response()
        }

        let state = TestTokenServerState {
            responses: Arc::new(tokio::sync::Mutex::new(VecDeque::from(responses))),
            requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
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

    #[derive(Clone)]
    struct EchoServer {
        url: String,
        state: EchoServerState,
    }

    impl EchoServer {
        async fn requests(&self) -> Vec<EchoRequestRecord> {
            self.state.requests.lock().await.clone()
        }
    }

    #[derive(Clone)]
    struct EchoServerState {
        requests: Arc<tokio::sync::Mutex<Vec<EchoRequestRecord>>>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct EchoRequestRecord {
        authorization: Option<String>,
    }

    async fn start_echo_server() -> EchoServer {
        async fn echo_handler(
            State(state): State<EchoServerState>,
            headers: HeaderMap,
        ) -> impl IntoResponse {
            state.requests.lock().await.push(EchoRequestRecord {
                authorization: headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned),
            });

            (StatusCode::OK, Json(json!({"ok": true})))
        }

        let state = EchoServerState {
            requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        };

        let app = Router::new()
            .route("/resource", get(echo_handler))
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        EchoServer {
            url: format!("http://{}/resource", address),
            state,
        }
    }

    fn oauth_rule_for_host(host: &str, token_url: String) -> HeaderRule {
        HeaderRule::oauth_client_credentials(
            host.to_string(),
            vec![],
            OAuthClientCredentialsConfig {
                header_name: "Authorization".to_string(),
                token_url,
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                scope: Some("read:all".to_string()),
                refresh_buffer_secs: 0,
            },
        )
        .expect("rule should be valid")
    }

    fn allow_when_authorization_matches(expected: &str) -> Arc<PolicyChain> {
        let tempdir = tempfile::tempdir().expect("tempdir should be created");
        let policy_path = tempdir.path().join("fetch.rego");
        std::fs::write(
            &policy_path,
            format!(
                r#"
package mcp.fetch

default allow := false

allow if {{
  input.headers.authorization == "{expected}"
}}
"#
            ),
        )
        .expect("rego policy should be written");

        let evaluator = LocalPolicyEvaluator::from_file(&policy_path, "data.mcp.fetch.allow".to_string())
            .expect("local policy evaluator should be created");

        Arc::new(PolicyChain::new(
            vec![PolicyEvaluatorKind::Local(evaluator)],
            EvalMode::All,
        ))
    }

    #[derive(Debug, Deserialize)]
    struct FetchResponseBody {
        ok: bool,
    }

    #[test]
    fn test_header_rule_new_rejects_empty_host() {
        let err = HeaderRule::new(
            "   ".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer tok".into())]),
            },
        )
        .expect_err("blank host should fail");

        assert!(err.to_string().contains("'host' cannot be empty"));
    }

    #[test]
    fn test_header_rule_static_header_rejects_empty_name() {
        let err = HeaderRule::static_header(
            "example.com".to_string(),
            vec![],
            "   ".to_string(),
            "Bearer tok".to_string(),
        )
        .expect_err("blank header name should fail");

        assert!(err.to_string().contains("'header' cannot be empty"));
    }

    #[test]
    fn test_header_rule_oauth_client_credentials_rejects_blank_required_fields() {
        let err = HeaderRule::oauth_client_credentials(
            "example.com".to_string(),
            vec![],
            OAuthClientCredentialsConfig {
                header_name: "Authorization".to_string(),
                token_url: " ".to_string(),
                client_id: "abc".to_string(),
                client_secret: "xyz".to_string(),
                scope: None,
                refresh_buffer_secs: 30,
            },
        )
        .expect_err("blank token_url should fail");

        assert!(err.to_string().contains("'token_url' cannot be empty"));
    }

    #[test]
    fn test_header_rule_new_rejects_empty_static_headers_map() {
        let err = HeaderRule::new(
            "example.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::new(),
            },
        )
        .expect_err("empty static headers map should fail");

        assert!(err.to_string().contains("static headers cannot be empty"));
    }

    #[test]
    fn test_header_rule_new_rejects_blank_static_header_value() {
        let err = HeaderRule::new(
            "example.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "   ".into())]),
            },
        )
        .expect_err("blank static header value should fail");

        assert!(err.to_string().contains("'value' cannot be empty"));
    }

    #[test]
    fn test_header_rule_new_rejects_case_variant_duplicate_static_headers() {
        let err = HeaderRule::new(
            "example.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([
                    ("Authorization".into(), "Bearer one".into()),
                    ("authorization".into(), "Bearer two".into()),
                ]),
            },
        )
        .expect_err("case-variant duplicate headers should fail");

        assert!(err.to_string().contains("duplicate static header name"));
    }

    #[test]
    fn test_header_rule_oauth_client_credentials_trims_required_fields() {
        let rule = HeaderRule::oauth_client_credentials(
            " example.com ".to_string(),
            vec![" get ".to_string()],
            OAuthClientCredentialsConfig {
                header_name: " Authorization ".to_string(),
                token_url: " https://issuer/token ".to_string(),
                client_id: " abc ".to_string(),
                client_secret: " xyz ".to_string(),
                scope: Some("read:all".to_string()),
                refresh_buffer_secs: 30,
            },
        )
        .expect("required auth fields should be trimmed");

        let auth = rule.dynamic_auth().expect("dynamic auth expected");
        assert_eq!(rule.host, "example.com");
        assert_eq!(rule.methods(), &["GET".to_string()]);
        assert_eq!(auth.header_name, "Authorization");
        assert_eq!(auth.token_url, "https://issuer/token");
        assert_eq!(auth.client_id, "abc");
        assert_eq!(auth.client_secret, "xyz");
    }

    #[test]
    fn test_header_rule_exact_host_match() {
        let rule = HeaderRule::new(
            "api.github.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer tok".into())]),
            },
        )
        .expect("rule should be valid");
        assert!(rule.matches("api.github.com", "GET"));
        assert!(rule.matches("api.github.com", "POST"));
        assert!(!rule.matches("other.github.com", "GET"));
        assert!(!rule.matches("github.com", "GET"));
    }

    #[test]
    fn test_header_rule_wildcard_host_match() {
        let rule = HeaderRule::new(
            "*.github.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer tok".into())]),
            },
        )
        .expect("rule should be valid");
        assert!(rule.matches("api.github.com", "GET"));
        assert!(rule.matches("github.com", "GET"));
        assert!(rule.matches("sub.api.github.com", "GET"));
        assert!(!rule.matches("github.org", "GET"));
    }

    #[test]
    fn test_header_rule_case_insensitive_host() {
        let rule = HeaderRule::new(
            "API.GitHub.COM".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer tok".into())]),
            },
        )
        .expect("rule should be valid");
        assert!(rule.matches("api.github.com", "GET"));
        assert!(rule.matches("API.GITHUB.COM", "GET"));
    }

    #[test]
    fn test_header_rule_method_filter() {
        let rule = HeaderRule::new(
            "example.com".to_string(),
            vec!["GET".to_string(), "POST".to_string()],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer tok".into())]),
            },
        )
        .expect("rule should be valid");
        assert!(rule.matches("example.com", "GET"));
        assert!(rule.matches("example.com", "post"));
        assert!(!rule.matches("example.com", "DELETE"));
    }

    #[test]
    fn test_header_rule_empty_methods_matches_all() {
        let rule = HeaderRule::new(
            "example.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer tok".into())]),
            },
        )
        .expect("rule should be valid");
        assert!(rule.matches("example.com", "GET"));
        assert!(rule.matches("example.com", "POST"));
        assert!(rule.matches("example.com", "DELETE"));
        assert!(rule.matches("example.com", "PATCH"));
    }

    #[tokio::test]
    async fn test_apply_header_rules_injects_when_absent() {
        let rules = vec![HeaderRule::new(
            "example.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer injected".into())]),
            },
        )
        .expect("rule should be valid")];
        let mut headers = HashMap::new();
        apply_header_rules(&rules, "example.com", "GET", &mut headers)
            .await
            .expect("rule application should succeed");
        assert_eq!(headers["authorization"], "Bearer injected");
    }

    #[tokio::test]
    async fn test_apply_header_rules_user_headers_take_precedence() {
        let rules = vec![HeaderRule::new(
            "example.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer injected".into())]),
            },
        )
        .expect("rule should be valid")];
        let mut headers = HashMap::new();
        headers.insert("authorization".to_string(), "Bearer user-provided".to_string());
        apply_header_rules(&rules, "example.com", "GET", &mut headers)
            .await
            .expect("rule application should succeed");
        assert_eq!(headers["authorization"], "Bearer user-provided");
    }

    #[tokio::test]
    async fn test_apply_header_rules_no_match() {
        let rules = vec![HeaderRule::new(
            "example.com".to_string(),
            vec!["POST".to_string()],
            HeaderInjection::Static {
                headers: HashMap::from([("authorization".into(), "Bearer tok".into())]),
            },
        )
        .expect("rule should be valid")];
        let mut headers = HashMap::new();

        // Host mismatch
        apply_header_rules(&rules, "other.com", "POST", &mut headers)
            .await
            .expect("host mismatch should not fail");
        assert!(headers.is_empty());

        // Method mismatch
        apply_header_rules(&rules, "example.com", "GET", &mut headers)
            .await
            .expect("method mismatch should not fail");
        assert!(headers.is_empty());
    }

    #[tokio::test]
    async fn test_apply_header_rules_multiple_headers() {
        let rules = vec![HeaderRule::new(
            "example.com".to_string(),
            vec![],
            HeaderInjection::Static {
                headers: HashMap::from([
                    ("authorization".into(), "Bearer tok".into()),
                    ("x-custom".into(), "custom-value".into()),
                ]),
            },
        )
        .expect("rule should be valid")];
        let mut headers = HashMap::new();
        apply_header_rules(&rules, "example.com", "GET", &mut headers)
            .await
            .expect("rule application should succeed");
        assert_eq!(headers["authorization"], "Bearer tok");
        assert_eq!(headers["x-custom"], "custom-value");
    }

    #[tokio::test]
    async fn test_apply_header_rules_injects_dynamic_auth_when_absent() {
        let token_server = start_token_server(vec![TestTokenResponse::success(json!({
            "access_token": "dynamic-token",
            "token_type": "Bearer",
            "expires_in": 3600
        }))])
        .await;
        let rules = vec![oauth_rule_for_host("example.com", token_server.token_url())];
        let mut headers = HashMap::new();

        apply_header_rules(&rules, "example.com", "GET", &mut headers)
            .await
            .expect("dynamic auth should be injected");

        assert_eq!(headers["authorization"], "Bearer dynamic-token");
        assert_eq!(
            token_server.requests().await,
            vec![TestTokenRequest {
                grant_type: "client_credentials".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn test_apply_header_rules_skips_dynamic_injection_when_user_header_exists() {
        let token_server = start_token_server(vec![TestTokenResponse::success(json!({
            "access_token": "dynamic-token",
            "token_type": "Bearer",
            "expires_in": 3600
        }))])
        .await;
        let rules = vec![oauth_rule_for_host("example.com", token_server.token_url())];
        let mut headers = HashMap::from([(
            "authorization".to_string(),
            "Bearer user-provided".to_string(),
        )]);

        apply_header_rules(&rules, "example.com", "GET", &mut headers)
            .await
            .expect("user-provided authorization should win");

        assert_eq!(headers["authorization"], "Bearer user-provided");
        assert!(token_server.requests().await.is_empty());
    }

    #[tokio::test]
    async fn test_do_fetch_applies_dynamic_auth_before_policy_evaluation() {
        let token_server = start_token_server(vec![TestTokenResponse::success(json!({
            "access_token": "policy-token",
            "token_type": "Bearer",
            "expires_in": 3600
        }))])
        .await;
        let echo_server = start_echo_server().await;
        let policy_chain = allow_when_authorization_matches("Bearer policy-token");

        let response = do_fetch(
            echo_server.url.clone(),
            "GET".to_string(),
            "{}".to_string(),
            None,
            policy_chain,
            reqwest::Client::new(),
            vec![oauth_rule_for_host("127.0.0.1", token_server.token_url())],
        )
        .await
        .expect("fetch should succeed when policy sees injected auth");

        // The response body is base64-encoded (binary-safe op boundary); decode
        // it before parsing the echo server's JSON.
        let response_value: serde_json::Value =
            serde_json::from_str(&response).expect("response JSON should parse");
        let body_b64 = response_value
            .get("body")
            .and_then(Value::as_str)
            .expect("response body should exist");
        let body_bytes = super::b64_decode(body_b64).expect("response body should base64-decode");
        let payload: FetchResponseBody =
            serde_json::from_slice(&body_bytes).expect("response body JSON should parse");

        assert!(payload.ok);
        assert_eq!(
            echo_server.requests().await,
            vec![EchoRequestRecord {
                authorization: Some("Bearer policy-token".to_string()),
            }]
        );
    }

    #[tokio::test]
    async fn test_do_fetch_reports_host_when_dynamic_auth_fails() {
        let token_server = start_token_server(vec![TestTokenResponse::failure(
            StatusCode::BAD_REQUEST,
            json!({
                "error":"invalid_client",
                "access_token":"server-access-token",
                "refresh_token":"server-refresh-token",
                "detail":"client-secret should never surface"
            }),
        )])
        .await;

        let err = do_fetch(
            "http://example.com/resource".to_string(),
            "GET".to_string(),
            "{}".to_string(),
            None,
            Arc::new(PolicyChain::new(vec![], EvalMode::All)),
            reqwest::Client::new(),
            vec![oauth_rule_for_host("example.com", token_server.token_url())],
        )
        .await
        .expect_err("dynamic auth failure should bubble up");

        assert!(err.contains("host 'example.com'"), "unexpected error: {err}");
        assert!(err.contains("credential injection failed"), "unexpected error: {err}");
        assert!(err.contains("header 'Authorization'"), "unexpected error: {err}");
        assert!(err.contains("token acquisition"), "unexpected error: {err}");
        assert!(!err.contains("client-secret"), "secret leaked in error: {err}");
        assert!(!err.contains("invalid_client"), "endpoint body leaked in error: {err}");
        assert!(
            !err.contains("server-access-token"),
            "access token leaked in error: {err}"
        );
        assert!(
            !err.contains("server-refresh-token"),
            "refresh token leaked in error: {err}"
        );
    }

    #[tokio::test]
    async fn test_do_fetch_sends_multipart_body() {
        #[derive(Clone)]
        struct MultipartState {
            requests: Arc<tokio::sync::Mutex<Vec<(String, String)>>>,
        }

        async fn multipart_handler(
            State(state): State<MultipartState>,
            headers: HeaderMap,
            body: axum::body::Bytes,
        ) -> impl IntoResponse {
            let ct = headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let body_str = String::from_utf8_lossy(&body).to_string();
            state.requests.lock().await.push((ct, body_str));
            (StatusCode::OK, Json(json!({"received": true})))
        }

        let state = MultipartState {
            requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        };

        let app = Router::new()
            .route("/upload", post(multipart_handler))
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let url = format!("http://{}/upload", addr);
        let boundary = "----TestBoundary123";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"f\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nhello world\r\n--{boundary}--\r\n"
        );
        let ct = format!("multipart/form-data; boundary={boundary}");
        let headers_json = serde_json::json!({"content-type": ct}).to_string();

        let resp = do_fetch(
            url,
            "POST".to_string(),
            headers_json,
            // do_fetch expects the request body base64-encoded (binary-safe).
            Some(super::b64_encode(body.as_bytes())),
            Arc::new(PolicyChain::new(vec![], EvalMode::All)),
            reqwest::Client::new(),
            vec![],
        )
        .await
        .expect("multipart fetch should succeed");

        let parsed: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed["status"], 200);

        let reqs = state.requests.lock().await;
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].0.starts_with("multipart/form-data; boundary="));
        assert!(reqs[0].1.contains("hello world"));
        assert!(reqs[0].1.contains("name=\"f\""));
        assert!(reqs[0].1.contains("filename=\"test.txt\""));
    }

    #[test]
    fn test_b64_round_trips_binary() {
        // Every byte value must survive the base64 round-trip used to carry
        // fetch request/response bodies across the op boundary.
        let all_bytes: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        assert_eq!(
            super::b64_decode(&super::b64_encode(&all_bytes)).expect("decode"),
            all_bytes
        );
        // Exercise every padding case (len % 3 == 0/1/2) and empty input.
        for n in [0usize, 1, 2, 3, 4, 5, 100, 255] {
            let d: Vec<u8> = (0..n).map(|i| (i * 31 + 7) as u8).collect();
            assert_eq!(
                super::b64_decode(&super::b64_encode(&d)).unwrap(),
                d,
                "round-trip failed for len {n}"
            );
        }
    }
}
