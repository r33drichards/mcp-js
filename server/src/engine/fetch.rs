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

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;

use super::opa::OpaClient;

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for the fetch() function, including OPA policy settings.
/// Stored in deno_core's `OpState` for access from async ops.
#[derive(Clone, Debug)]
pub struct FetchConfig {
    pub opa_client: OpaClient,
    pub opa_policy_path: String,
    pub http_client: reqwest::Client,
}

impl FetchConfig {
    pub fn new(opa_url: String, opa_policy_path: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create fetch HTTP client");
        Self {
            opa_client: OpaClient::new(opa_url),
            opa_policy_path,
            http_client,
        }
    }
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
    #[string] body: Option<String>,
) -> Result<String, JsErrorBox> {
    // Clone config from OpState before any .await (Rc is !Send).
    let (opa_client, opa_policy_path, http_client) = {
        let state = state.borrow();
        let config = state.try_borrow::<FetchConfig>()
            .ok_or_else(|| JsErrorBox::generic("fetch: internal error — no fetch config available"))?;
        (
            config.opa_client.clone(),
            config.opa_policy_path.clone(),
            config.http_client.clone(),
        )
    };

    do_fetch(url, method, headers_json, body, opa_client, opa_policy_path, http_client)
        .await
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
        const body = opts.body !== undefined ? String(opts.body) : null;

        // Normalize headers to plain object with lowercase keys
        const normalizedHeaders = {};
        if (headers && typeof headers === 'object') {
            for (const key of Object.keys(headers)) {
                normalizedHeaders[key.toLowerCase()] = String(headers[key]);
            }
        }

        const headersJson = JSON.stringify(normalizedHeaders);

        // Async op — truly non-blocking, returns a Promise
        const rawResult = await Deno.core.ops.op_fetch(resource, method, headersJson, body);
        const result = JSON.parse(rawResult);

        const responseBody = result.body;
        const responseHeaders = new Headers(result.headers);

        return {
            ok: result.status >= 200 && result.status < 300,
            status: result.status,
            statusText: result.statusText,
            url: result.url,
            headers: responseHeaders,
            redirected: result.redirected || false,
            type: 'basic',
            bodyUsed: false,
            text: function() { return Promise.resolve(responseBody); },
            json: function() { return Promise.resolve(JSON.parse(responseBody)); },
            clone: function() {
                return {
                    ok: this.ok,
                    status: this.status,
                    statusText: this.statusText,
                    url: this.url,
                    headers: this.headers,
                    redirected: this.redirected,
                    type: this.type,
                    bodyUsed: false,
                    text: function() { return Promise.resolve(responseBody); },
                    json: function() { return Promise.resolve(JSON.parse(responseBody)); },
                };
            }
        };
    };
})();
"#;

// ── Pure-Rust fetch implementation (no V8 types) ─────────────────────────

/// Execute an OPA-gated HTTP fetch. All V8 interaction happens in the caller.
async fn do_fetch(
    url_str: String,
    method: String,
    headers_json: String,
    body: Option<String>,
    opa_client: OpaClient,
    opa_policy_path: String,
    http_client: reqwest::Client,
) -> Result<String, String> {
    let headers: HashMap<String, String> = serde_json::from_str(&headers_json)
        .map_err(|e| format!("fetch: invalid headers JSON: {}", e))?;

    // Parse URL into components for OPA policy input
    let parsed_url = url::Url::parse(&url_str)
        .map_err(|e| format!("fetch: invalid URL '{}': {}", url_str, e))?;

    let url_parsed = UrlParsed {
        scheme: parsed_url.scheme().to_string(),
        host: parsed_url.host_str().unwrap_or("").to_string(),
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

    // Evaluate OPA policy
    let allowed = opa_client
        .evaluate(&opa_policy_path, &policy_input)
        .await?;

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
        req_builder = req_builder.body(body);
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

    let resp_body = resp
        .text()
        .await
        .map_err(|e| format!("fetch: failed to read response body: {}", e))?;

    let result = serde_json::json!({
        "status": status,
        "statusText": status_text,
        "url": final_url,
        "headers": resp_headers,
        "body": resp_body,
        "redirected": final_url != url_str,
    });

    Ok(result.to_string())
}
