/// Integration tests for fetch header injection.
///
/// Spins up mock HTTP servers (OPA + echo target), configures an Engine with
/// header rules, runs JS `fetch()` calls, and asserts that injected headers
/// arrive (or don't) on the outbound request.

use std::collections::HashMap;
use std::sync::{Arc, Once};

use axum::{
    Router,
    extract::Request,
    response::Json,
    routing::{any, post},
};
use serde_json::Value;
use server::engine::{initialize_v8, Engine};
use server::engine::fetch::{FetchConfig, HeaderRule};
use server::engine::execution::ExecutionRegistry;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

// ── Mock servers ────────────────────────────────────────────────────────

/// Start a mock OPA server that always allows requests.
/// Returns the base URL, e.g. "http://127.0.0.1:12345".
async fn start_opa_mock() -> String {
    let app = Router::new().route(
        "/v1/data/mcp/fetch",
        post(|| async {
            Json(serde_json::json!({ "result": { "allow": true } }))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://127.0.0.1:{}", port)
}

/// Start a mock target server that echoes back all received request headers
/// as a JSON object. Returns the base URL.
async fn start_echo_mock() -> String {
    let app = Router::new().route(
        "/",
        any(|req: Request| async move {
            let headers: HashMap<String, String> = req
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        v.to_str().unwrap_or("").to_string(),
                    )
                })
                .collect();
            Json(serde_json::json!(headers))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://127.0.0.1:{}", port)
}

// ── Helper to build an Engine with header rules ─────────────────────────

fn build_engine(opa_url: &str, header_rules: Vec<HeaderRule>) -> Engine {
    let fetch_config = FetchConfig::new(opa_url.to_string(), "mcp/fetch".to_string())
        .with_header_rules(header_rules);

    let tmp = std::env::temp_dir().join(format!("mcp-fetch-test-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_fetch_config(fetch_config)
        .with_execution_registry(Arc::new(registry))
}

/// Run JS code that fetches the echo endpoint and parse the echoed headers.
async fn fetch_and_get_echoed_headers(
    engine: &Engine,
    echo_url: &str,
    js_extra: &str,
) -> HashMap<String, String> {
    // JS code: call fetch, parse the echoed JSON, re-stringify so V8
    // returns a proper JSON string (not "[object Object]").
    // Wrapped in an async IIFE because fetch() is async and top-level
    // await is not available in script (non-module) context.
    let code = format!(
        r#"
        (async () => {{
            const resp = await fetch("{echo_url}", {js_extra});
            return JSON.stringify(await resp.json());
        }})()
        "#,
        echo_url = echo_url,
        js_extra = js_extra,
    );

    let exec_id = engine
        .run_js(code, None, None, None, None, None)
        .await
        .expect("submit should succeed");

    // Poll for completion
    let mut output = String::new();
    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => { output = info.result.expect("should have result"); break; }
                "failed" => panic!("Execution failed: {}", info.error.unwrap_or_default()),
                "timed_out" => panic!("Execution timed out"),
                _ => continue,
            }
        }
    }

    let parsed: Value = serde_json::from_str(&output)
        .expect("output should be valid JSON");

    parsed
        .as_object()
        .expect("output should be a JSON object")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_injected_header_arrives_on_request() {
    ensure_v8();

    let opa_url = start_opa_mock().await;
    let echo_url = start_echo_mock().await;

    let rules = vec![HeaderRule {
        host: "127.0.0.1".to_string(),
        methods: vec![],
        headers: HashMap::from([
            ("authorization".to_string(), "Bearer test-token".to_string()),
        ]),
    }];

    let engine = build_engine(&opa_url, rules);
    let echoed = fetch_and_get_echoed_headers(&engine, &echo_url, "{}").await;

    assert_eq!(
        echoed.get("authorization").map(|s| s.as_str()),
        Some("Bearer test-token"),
        "Injected authorization header should be present. Got headers: {:?}",
        echoed
    );
}

#[tokio::test]
async fn test_user_header_overrides_injected() {
    ensure_v8();

    let opa_url = start_opa_mock().await;
    let echo_url = start_echo_mock().await;

    let rules = vec![HeaderRule {
        host: "127.0.0.1".to_string(),
        methods: vec![],
        headers: HashMap::from([
            ("authorization".to_string(), "Bearer injected".to_string()),
        ]),
    }];

    let engine = build_engine(&opa_url, rules);

    let echoed = fetch_and_get_echoed_headers(
        &engine,
        &echo_url,
        r#"{ headers: { "Authorization": "Bearer user-provided" } }"#,
    )
    .await;

    assert_eq!(
        echoed.get("authorization").map(|s| s.as_str()),
        Some("Bearer user-provided"),
        "User-provided header should override injected one. Got headers: {:?}",
        echoed
    );
}

#[tokio::test]
async fn test_no_injection_on_host_mismatch() {
    ensure_v8();

    let opa_url = start_opa_mock().await;
    let echo_url = start_echo_mock().await;

    // Rule targets a different host than the echo server (127.0.0.1)
    let rules = vec![HeaderRule {
        host: "other.example.com".to_string(),
        methods: vec![],
        headers: HashMap::from([
            ("x-injected".to_string(), "should-not-appear".to_string()),
        ]),
    }];

    let engine = build_engine(&opa_url, rules);
    let echoed = fetch_and_get_echoed_headers(&engine, &echo_url, "{}").await;

    assert!(
        !echoed.contains_key("x-injected"),
        "Header should NOT be injected when host doesn't match. Got headers: {:?}",
        echoed
    );
}

#[tokio::test]
async fn test_no_injection_on_method_mismatch() {
    ensure_v8();

    let opa_url = start_opa_mock().await;
    let echo_url = start_echo_mock().await;

    // Rule targets only POST, but JS will call GET (the default)
    let rules = vec![HeaderRule {
        host: "127.0.0.1".to_string(),
        methods: vec!["POST".to_string()],
        headers: HashMap::from([
            ("x-post-only".to_string(), "should-not-appear".to_string()),
        ]),
    }];

    let engine = build_engine(&opa_url, rules);
    let echoed = fetch_and_get_echoed_headers(&engine, &echo_url, "{}").await;

    assert!(
        !echoed.contains_key("x-post-only"),
        "Header should NOT be injected when method doesn't match. Got headers: {:?}",
        echoed
    );
}
