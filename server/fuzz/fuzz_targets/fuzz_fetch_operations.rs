
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::ExecutionConfig;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
                deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
                std::panic::set_hook(Box::new(|_| {}));
    });
}


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

fuzz_target!(|input: FetchOperationInput| {
    ensure_v8();

        let mut headers = input.headers_data;
    headers.truncate(20);

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

        let max_bytes = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateless(&js_code, ExecutionConfig::new(max_bytes)
        .isolate_handle(handle));
});

/// Escape a string for safe inclusion in JavaScript code
fn escape_js_string(s: &str) -> String {
        let s = if s.len() > 10000 {
        let mut end = 10000;
        while !s.is_char_boundary(end) { end -= 1; }
        &s[..end]
    } else {
        s
    };

    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\0', "\\0")
}
