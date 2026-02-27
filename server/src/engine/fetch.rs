//! OPA-gated `fetch()` implementation for the V8 JavaScript runtime.
//!
//! Injects a synchronous, web-standards-like `fetch(url, opts?)` global
//! function into the V8 runtime. Each request is evaluated against an OPA
//! policy before execution.
//!
//! The Response object mirrors the web Fetch API shape (synchronous variant):
//! ```js
//! const resp = fetch("https://example.com");
//! resp.ok           // boolean
//! resp.status       // number
//! resp.statusText   // string
//! resp.url          // string
//! resp.headers      // Headers object with .get(name)
//! resp.text()       // body as string
//! resp.json()       // parsed JSON
//! ```

use std::cell::RefCell;
use std::collections::HashMap;

use deno_core::v8;
use deno_core::JsRuntime;
use serde::Serialize;

use super::opa::OpaClient;

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for the fetch() function, including OPA policy settings.
#[derive(Clone, Debug)]
pub struct FetchConfig {
    pub opa_client: OpaClient,
    pub opa_policy_path: String,
    pub http_client: reqwest::blocking::Client,
}

impl FetchConfig {
    pub fn new(opa_url: String, opa_policy_path: String) -> Self {
        let http_client = reqwest::blocking::Client::builder()
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

// ── Thread-local for V8 callback context ─────────────────────────────────
//
// V8 function callbacks (`extern "C" fn`) can't capture closures. Since
// each V8 execution runs on a single thread inside `spawn_blocking`, a
// thread-local carries the config into the callback.

thread_local! {
    static FETCH_CONFIG: RefCell<Option<FetchConfig>> = const { RefCell::new(None) };
}

pub fn set_thread_fetch_config(config: FetchConfig) {
    FETCH_CONFIG.with(|c| *c.borrow_mut() = Some(config));
}

pub fn clear_thread_fetch_config() {
    FETCH_CONFIG.with(|c| *c.borrow_mut() = None);
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

// ── Inject fetch() into the V8 global scope ──────────────────────────────

/// Inject a global `fetch` function into the V8 runtime.
/// Must be called after the runtime is created but before user code runs.
pub fn inject_fetch(runtime: &mut JsRuntime) -> Result<(), String> {
    // First, inject the native __fetch_native callback
    {
        deno_core::scope!(scope, runtime);
        let global = scope.get_current_context().global(scope);

        let fetch_fn = v8::Function::new(scope, fetch_callback)
            .ok_or("Failed to create fetch function")?;

        let key = v8::String::new(scope, "__fetch_native")
            .ok_or("Failed to create __fetch_native string")?;
        global.set(scope, key.into(), fetch_fn.into());
    }

    // Then, define the JS wrapper that builds the Response object
    runtime
        .execute_script(
            "<fetch-setup>",
            FETCH_JS_WRAPPER.to_string(),
        )
        .map_err(|e| format!("Failed to install fetch wrapper: {}", e))?;

    Ok(())
}

/// JavaScript wrapper that provides the web-standards-like Response API.
/// The native `__fetch_native(url, method, headersJson, body)` returns a
/// JSON string with {status, statusText, url, headers, body}. This wrapper
/// parses it into a proper Response-like object.
const FETCH_JS_WRAPPER: &str = r#"
(function() {
    const nativeFetch = globalThis.__fetch_native;
    delete globalThis.__fetch_native;

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

    globalThis.fetch = function fetch(resource, init) {
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
        const rawResult = nativeFetch(resource, method, headersJson, body);
        const result = JSON.parse(rawResult);

        if (result.error) {
            throw new Error(result.error);
        }

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
            text: function() { return responseBody; },
            json: function() { return JSON.parse(responseBody); },
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
                    text: function() { return responseBody; },
                    json: function() { return JSON.parse(responseBody); },
                };
            }
        };
    };
})();
"#;

// ── V8 native callback ──────────────────────────────────────────────────

/// V8 native function: `__fetch_native(url, method, headersJson, body) -> jsonString`
///
/// Extracts string arguments from JS, delegates to `do_fetch` for the
/// OPA policy check + HTTP request (pure Rust, no V8 types), then converts
/// the result back to a V8 string.
fn fetch_callback(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // Extract all JS arguments up-front so the rest is pure Rust.
    let url_str = arg_to_string(scope, &args, 0);
    let method = arg_to_string(scope, &args, 1);
    let headers_json = arg_to_string(scope, &args, 2);
    let body = arg_to_string(scope, &args, 3);

    let result = do_fetch(url_str, method, headers_json, body);

    let json_str = match result {
        Ok(json) => json,
        Err(err) => serde_json::json!({ "error": err }).to_string(),
    };

    if let Some(v8_str) = v8::String::new(scope, &json_str) {
        rv.set(v8_str.into());
    }
}

/// Extract a JS argument as a Rust String.
fn arg_to_string(
    scope: &v8::PinScope<'_, '_>,
    args: &v8::FunctionCallbackArguments,
    index: i32,
) -> Option<String> {
    let val = args.get(index);
    if val.is_undefined() || val.is_null() {
        return None;
    }
    val.to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
}

// ── Pure-Rust fetch implementation (no V8 types) ─────────────────────────

/// Execute an OPA-gated HTTP fetch. All V8 interaction happens in the caller.
fn do_fetch(
    url_str: Option<String>,
    method: Option<String>,
    headers_json: Option<String>,
    body: Option<String>,
) -> Result<String, String> {
    let url_str = url_str
        .ok_or_else(|| "fetch: missing or invalid URL argument".to_string())?;
    let method = method.unwrap_or_else(|| "GET".to_string());
    let headers_json = headers_json.unwrap_or_else(|| "{}".to_string());

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

    // Evaluate OPA policy and execute request using thread-local config
    FETCH_CONFIG.with(|cell| {
        let borrow = cell.borrow();
        let config = borrow
            .as_ref()
            .ok_or_else(|| "fetch: internal error — no fetch config on this thread".to_string())?;

        let allowed = config
            .opa_client
            .evaluate(&config.opa_policy_path, &policy_input)?;

        if !allowed {
            return Err(format!(
                "fetch denied by policy: {} {} is not allowed",
                method, url_str
            ));
        }

        // Execute the HTTP request
        let mut req_builder = config.http_client.request(
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
    })
}
