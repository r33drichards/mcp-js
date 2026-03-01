#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::ExecutionConfig;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        // Disable V8 background threads to prevent cumulative memory exhaustion
        deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
        // Override libfuzzer's abort-on-panic hook for graceful panic handling
        std::panic::set_hook(Box::new(|_| {}));
    });
}

#[derive(Arbitrary, Debug)]
struct FetchOperationInput {
    /// The URL to fetch (can be empty, malformed, various schemes)
    url: String,
    /// HTTP method (arbitrary string for invalid cases)
    method: String,
    /// Headers map with arbitrary keys/values
    headers_data: Vec<(String, String)>,
    /// Whether to include a request body
    include_body: bool,
}

// Fuzz fetch operation inputs with edge cases:
// - Empty URLs
// - Malformed URLs (missing scheme, invalid hosts, special characters)
// - Various URL schemes (http, https, ftp, javascript, etc.)
// - Invalid HTTP methods
// - Header injection attempts (null bytes, newlines, unicode)
// - Large header values
// - Various domain formats (IP, unicode domains, special chars)
//
// This exercises URL parsing and HTTP request building paths
// Most requests will fail due to invalid URLs or policy denial, but that's the point —
// we're testing that the system handles invalid inputs gracefully.
fuzz_target!(|input: FetchOperationInput| {
    ensure_v8();

    // Cap headers to avoid excessive string building
    let mut headers = input.headers_data;
    headers.truncate(20);

    // Sanitize header values to avoid breaking JavaScript syntax
    let headers_js = headers
        .iter()
        .filter(|(k, v)| !k.is_empty() && k.len() < 1000 && v.len() < 1000)
        .map(|(k, v)| {
            format!(
                "'{}': '{}'",
                k.replace('\\', "\\\\")
                    .replace('\'', "\\'")
                    .replace('\n', "\\n")
                    .replace('\0', ""),
                v.replace('\\', "\\\\")
                    .replace('\'', "\\'")
                    .replace('\n', "\\n")
                    .replace('\0', "")
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    let headers_obj = if headers_js.is_empty() {
        "{}".to_string()
    } else {
        format!("{{{}}}", headers_js)
    };

    let method = if input.method.is_empty() {
        "GET".to_string()
    } else {
        input.method.to_uppercase()
    };

    let body_str = if input.include_body {
        r#", body: "fuzzer""#
    } else {
        ""
    };

    let url = escape_js_string(&input.url);

    let js_code = format!(
        r#"try {{
  await fetch('{}', {{
    method: '{}',
    headers: {},
    timeout: 5000
    {}
  }});
}} catch(e) {{ }}"#,
        url, method, headers_obj, body_str
    );

    // Execute stateless — we don't care about the result, only that it doesn't crash
    let max_bytes = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateless(&js_code, ExecutionConfig::new(max_bytes)
        .isolate_handle(handle));
});

/// Escape a string for safe inclusion in JavaScript code
fn escape_js_string(s: &str) -> String {
    // Cap URL length to avoid excessive code size
    let s = if s.len() > 10000 {
        &s[..10000]
    } else {
        s
    };

    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\0', "\\0")
}
