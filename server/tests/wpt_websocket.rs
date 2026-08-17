/// Web Platform Tests gate for the WebSocket implementation.
///
/// Runs the vendored `tests/wpt/websockets/*.any.js` files (see
/// `tests/wpt/README.md` for provenance) inside the real Engine against a
/// local echo server, and compares each file's outcome to
/// `tests/wpt/websocket_expectations.json`. A newly failing file fails this
/// test; a newly passing file also fails it, so the manifest stays honest —
/// update the manifest in the same change that fixes the implementation.
///
/// These live outside `tests/wpt/vendor/` on purpose: `wpt_harness.rs` walks
/// that tree and runs every file serverlessly, which websockets tests cannot
/// be. This runner supplies the echo server they need, so it keeps its own
/// manifest while sharing the vendored `resources/testharness.js`.
///
/// The WPT files are sloppy-mode scripts (undeclared-variable assignments),
/// so they are evaluated via indirect eval in global scope rather than being
/// concatenated into the (strict) ES module the engine executes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Once};

use axum::{
    Router,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
    routing::any,
};
use server::engine::execution::ExecutionRegistry;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::websocket::WebSocketConfig;
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

async fn start_echo_server() -> String {
    async fn ws_handler(headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
        let ws = match headers
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok())
            .and_then(|list| list.split(',').next().map(|p| p.trim().to_string()))
        {
            Some(protocol) => ws.protocols([protocol]),
            None => ws,
        };
        ws.on_upgrade(echo)
    }

    async fn echo(mut socket: WebSocket) {
        while let Some(Ok(message)) = socket.recv().await {
            match message {
                Message::Text(text) => {
                    if socket.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Message::Binary(bytes) => {
                    if socket.send(Message::Binary(bytes)).await.is_err() {
                        break;
                    }
                }
                // Keep polling after a Close so tungstenite's auto-queued
                // close reply is flushed before the socket drops.
                Message::Close(_) => continue,
                _ => {}
            }
        }
    }

    let app = Router::new().route("/echo", any(ws_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr.to_string()
}

fn build_engine() -> Engine {
    // WPT runs against an allow-all policy: the suite tests API conformance,
    // not the policy layer (websocket_e2e.rs covers that).
    let chain = Arc::new(PolicyChain::new(vec![], EvalMode::All));
    let tmp = std::env::temp_dir().join(format!(
        "mcp-wpt-ws-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");

    Engine::new_stateless(128 * 1024 * 1024, 15, 4)
        .with_websocket_config(WebSocketConfig::new_with_chain(chain))
        .with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: String) -> Result<String, String> {
    let exec_id = engine
        .run_js(code)
        .execute()
        .await
        .map_err(|error| format!("submit should succeed: {error}"))?;

    for _ in 0..400 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return Ok(info.result.unwrap_or_default()),
                "failed" => return Err(info.error.unwrap_or_default()),
                "timed_out" => return Err("execution timed out".to_string()),
                _ => continue,
            }
        }
    }

    Err("timeout waiting for execution".to_string())
}

/// Build the per-file runner script: bootstrap globals, load testharness,
/// register the completion hook, load the substituted constants helper and
/// the test file, then await completion and throw on any failing subtest.
fn build_runner_script(addr: &str, testharness: &str, constants: &str, test_file: &str) -> String {
    let host = addr.split(':').next().unwrap();
    let port = addr.split(':').nth(1).unwrap();

    let constants = constants
        .replace("{{host}}", host)
        .replace("{{ports[ws][0]}}", port)
        .replace("{{ports[wss][0]}}", port)
        .replace("{{ports[h2][0]}}", port)
        .replace("{{hosts[alt][www]}}", "alt.example");

    // `location` must stringify to its href: tests do `new URL(x, location)`.
    let bootstrap = format!(
        r#"globalThis.location = {{
            protocol: 'http:',
            search: '?default',
            href: 'http://{addr}/',
            host: '{addr}',
            hostname: '{host}',
            port: '{port}',
            toString: function () {{ return this.href; }},
        }};"#
    );

    format!(
        r#"
        const __geval = eval; // indirect eval: global scope, sloppy mode
        __geval({bootstrap});
        __geval({testharness});
        const __completion = new Promise((resolve) => {{
            add_completion_callback((tests, harnessStatus) => {{
                resolve({{
                    harness: {{ status: harnessStatus.status, message: harnessStatus.message }},
                    tests: tests.map((t) => ({{ name: t.name, status: t.status, message: t.message }})),
                }});
            }});
        }});
        let __result;
        try {{
            __geval({constants});
            __geval({test_file});
            __result = await __completion;
        }} finally {{
            // Always drop open sockets — a test that leaves one open (or a
            // load-time throw) must not hang the execution's event loop.
            __mcpV8WebSocketCloseAll();
        }}
        const __failures = __result.tests.filter((t) => t.status !== 0);
        if (__result.harness.status !== 0) {{
            __failures.push({{ name: "<harness>", status: __result.harness.status, message: __result.harness.message }});
        }}
        if (__result.tests.length === 0) {{
            __failures.push({{ name: "<harness>", status: -1, message: "no tests ran" }});
        }}
        if (__failures.length > 0) {{
            throw new Error("WPT-FAIL " + JSON.stringify(__failures));
        }}
        "#,
        bootstrap = serde_json::to_string(&bootstrap).unwrap(),
        testharness = serde_json::to_string(testharness).unwrap(),
        constants = serde_json::to_string(&constants).unwrap(),
        test_file = serde_json::to_string(test_file).unwrap(),
    )
}

#[tokio::test]
async fn wpt_websockets_match_expectations() {
    ensure_v8();

    let wpt_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt");
    let testharness = std::fs::read_to_string(wpt_dir.join("vendor/resources/testharness.js"))
        .expect("vendored testharness.js");
    let constants = std::fs::read_to_string(wpt_dir.join("websockets/constants.sub.js"))
        .expect("constants.sub.js");
    let expectations: BTreeMap<String, String> = serde_json::from_str(
        &std::fs::read_to_string(wpt_dir.join("websocket_expectations.json"))
            .expect("websocket_expectations.json"),
    )
    .expect("websocket_expectations.json should be a {file: \"pass\"|\"fail\"} map");

    let mut test_files: Vec<String> = std::fs::read_dir(wpt_dir.join("websockets"))
        .expect("websockets dir")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.ends_with(".any.js").then_some(name)
        })
        .collect();
    test_files.sort();
    assert!(!test_files.is_empty(), "no vendored WPT files found");

    let addr = start_echo_server().await;
    let engine = build_engine();

    let mut mismatches: Vec<String> = Vec::new();
    for file in &test_files {
        let expected = expectations
            .get(file)
            .map(String::as_str)
            .unwrap_or("pass");
        let source = std::fs::read_to_string(wpt_dir.join("websockets").join(file)).unwrap();
        let script = build_runner_script(&addr, &testharness, &constants, &source);
        let outcome = run_js(&engine, script).await;
        let actual = if outcome.is_ok() { "pass" } else { "fail" };

        if actual != expected {
            let detail = match &outcome {
                Ok(_) => "unexpected PASS — remove it from the expected-fail list".to_string(),
                Err(e) => format!("unexpected FAIL — {}", e.lines().next().unwrap_or("")),
            };
            mismatches.push(format!("{file}: expected {expected}, got {actual}: {detail}"));
        }
    }

    // Every entry in the manifest must correspond to a vendored file, so the
    // expected-fail list can't silently rot.
    for file in expectations.keys() {
        if !test_files.contains(file) {
            mismatches.push(format!(
                "{file}: in websocket_expectations.json but not vendored"
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "WPT websockets results diverged from websocket_expectations.json:\n  {}",
        mismatches.join("\n  ")
    );
}
