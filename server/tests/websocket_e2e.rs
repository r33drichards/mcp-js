/// Integration tests for the policy-gated WebSocket client.
///
/// Uses a local axum WebSocket echo server plus local Rego policies, runs JS
/// through the real Engine, and asserts against both the JS-observable
/// behavior (thrown errors fail the execution) and the server-side handshake
/// records (header injection, secret non-leakage).

use std::sync::{Arc, Once};

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::Response,
    routing::any,
};
use server::engine::execution::ExecutionRegistry;
use server::engine::fetch::HeaderRule;
use server::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};
use server::engine::websocket::WebSocketConfig;
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

#[derive(Clone, Debug, Default)]
struct HandshakeRecord {
    x_api_key: Option<String>,
    authorization: Option<String>,
    protocols: Option<String>,
}

#[derive(Clone)]
struct EchoState {
    handshakes: Arc<tokio::sync::Mutex<Vec<HandshakeRecord>>>,
}

#[derive(Clone)]
struct EchoServer {
    addr: String,
    state: EchoState,
}

impl EchoServer {
    fn url(&self) -> String {
        format!("ws://{}/echo", self.addr)
    }

    async fn handshakes(&self) -> Vec<HandshakeRecord> {
        self.state.handshakes.lock().await.clone()
    }
}

async fn start_echo_server() -> EchoServer {
    async fn ws_handler(
        State(state): State<EchoState>,
        headers: HeaderMap,
        ws: WebSocketUpgrade,
    ) -> Response {
        let header_str = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned)
        };
        state.handshakes.lock().await.push(HandshakeRecord {
            x_api_key: header_str("x-api-key"),
            authorization: header_str("authorization"),
            protocols: header_str("sec-websocket-protocol"),
        });

        // Accept the first offered subprotocol, if any.
        let ws = match header_str("sec-websocket-protocol")
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
                Message::Close(_) => {
                    // tungstenite auto-queues the close reply (echoing the
                    // code); keep polling until the stream ends so the reply
                    // is flushed before the socket drops. Breaking here
                    // would drop the reply and the client would see a TCP
                    // reset instead of a clean close.
                    continue;
                }
                _ => {}
            }
        }
    }

    let state = EchoState {
        handshakes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/echo", any(ws_handler))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    EchoServer {
        addr: addr.to_string(),
        state,
    }
}

fn chain_from_rego(rego: &str) -> Arc<PolicyChain> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("websocket.rego");
    std::fs::write(&path, rego).unwrap();
    std::mem::forget(dir);

    let evaluator =
        LocalPolicyEvaluator::from_file(&path, "data.mcp.websocket.allow".to_string()).unwrap();
    Arc::new(PolicyChain::new(
        vec![PolicyEvaluatorKind::Local(evaluator)],
        EvalMode::All,
    ))
}

fn allow_all_chain() -> Arc<PolicyChain> {
    chain_from_rego("package mcp.websocket\ndefault allow = true\n")
}

fn build_engine(config: WebSocketConfig) -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-ws-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");

    Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_websocket_config(config)
        .with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: String) -> Result<String, String> {
    let exec_id = engine
        .run_js(code)
        .execute()
        .await
        .map_err(|error| format!("submit should succeed: {error}"))?;

    for _ in 0..600 {
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

#[tokio::test]
async fn websocket_echo_round_trip_with_subprotocol_and_clean_close() {
    ensure_v8();
    let server = start_echo_server().await;
    let engine = build_engine(WebSocketConfig::new_with_chain(allow_all_chain()));

    let code = format!(
        r#"
        const ws = new WebSocket("{url}", ["chat", "fallback"]);
        ws.binaryType = "arraybuffer";
        const received = [];
        const closeEvent = await new Promise((resolve, reject) => {{
            ws.onopen = () => {{
                if (ws.readyState !== WebSocket.OPEN) {{
                    reject(new Error("readyState should be OPEN in onopen"));
                    return;
                }}
                ws.send("hello");
                ws.send(new Uint8Array([1, 2, 250]).buffer);
            }};
            ws.onmessage = (event) => {{
                received.push(event.data);
                if (received.length === 2) ws.close(1000, "done");
            }};
            ws.onclose = resolve;
            ws.onerror = (event) => reject(new Error("unexpected websocket error: " + (event.message || "")));
        }});

        if (ws.protocol !== "chat") throw new Error("bad negotiated protocol: " + ws.protocol);
        if (received[0] !== "hello") throw new Error("bad text echo: " + received[0]);
        if (!(received[1] instanceof ArrayBuffer)) throw new Error("binary echo should be ArrayBuffer");
        const bytes = new Uint8Array(received[1]);
        if (bytes.length !== 3 || bytes[0] !== 1 || bytes[2] !== 250) {{
            throw new Error("bad binary echo: " + Array.from(bytes).join(","));
        }}
        if (closeEvent.code !== 1000) throw new Error("bad close code: " + closeEvent.code);
        if (!closeEvent.wasClean) throw new Error("close should be clean");
        if (ws.readyState !== WebSocket.CLOSED) throw new Error("readyState should be CLOSED");
        "#,
        url = server.url(),
    );

    run_js(&engine, code).await.expect("echo round trip should succeed");

    let handshakes = server.handshakes().await;
    assert_eq!(handshakes.len(), 1);
    assert_eq!(handshakes[0].protocols.as_deref(), Some("chat, fallback"));
}

#[tokio::test]
async fn websocket_connect_denied_by_policy_fires_error_and_unclean_close() {
    ensure_v8();
    let server = start_echo_server().await;
    // Policy allows only a host the test never connects to.
    let engine = build_engine(WebSocketConfig::new_with_chain(chain_from_rego(
        r#"
package mcp.websocket

default allow = false

allow if {
    input.url_parsed.host == "allowed.example"
}
"#,
    )));

    let code = format!(
        r#"
        const ws = new WebSocket("{url}");
        let sawError = false;
        const closeEvent = await new Promise((resolve, reject) => {{
            ws.onopen = () => reject(new Error("connect should have been denied"));
            ws.onerror = () => {{ sawError = true; }};
            ws.onclose = resolve;
        }});
        if (!sawError) throw new Error("error event should fire before close");
        if (closeEvent.wasClean) throw new Error("denied connect must not be a clean close");
        if (closeEvent.code !== 1006) throw new Error("bad close code: " + closeEvent.code);
        "#,
        url = server.url(),
    );

    run_js(&engine, code).await.expect("denied connect should surface as events, not a crash");

    // The policy check happens before any network I/O.
    assert!(server.handshakes().await.is_empty());
}

#[tokio::test]
async fn websocket_handshake_receives_injected_header_for_matching_host() {
    ensure_v8();
    let server = start_echo_server().await;
    let rule = HeaderRule::static_header(
        "127.0.0.1".to_string(),
        vec![],
        "x-api-key".to_string(),
        "secret-handshake-token".to_string(),
    )
    .expect("rule should be valid");
    let engine = build_engine(
        WebSocketConfig::new_with_chain(allow_all_chain()).with_header_rules(vec![rule]),
    );

    let code = format!(
        r#"
        const ws = new WebSocket("{url}");
        await new Promise((resolve, reject) => {{
            ws.onopen = () => {{ ws.close(1000); }};
            ws.onclose = resolve;
            ws.onerror = (event) => reject(new Error("unexpected websocket error: " + (event.message || "")));
        }});
        "#,
        url = server.url(),
    );

    run_js(&engine, code).await.expect("connect should succeed");

    let handshakes = server.handshakes().await;
    assert_eq!(handshakes.len(), 1);
    assert_eq!(
        handshakes[0].x_api_key.as_deref(),
        Some("secret-handshake-token"),
        "matching host should receive the injected header"
    );
}

#[tokio::test]
async fn websocket_secret_never_sent_to_non_matching_host() {
    ensure_v8();
    let server = start_echo_server().await;
    // Rule scoped to a different host: the local echo server must never see it.
    let rule = HeaderRule::static_header(
        "api.example.com".to_string(),
        vec![],
        "authorization".to_string(),
        "Bearer super-secret".to_string(),
    )
    .expect("rule should be valid");
    let engine = build_engine(
        WebSocketConfig::new_with_chain(allow_all_chain()).with_header_rules(vec![rule]),
    );

    let code = format!(
        r#"
        const ws = new WebSocket("{url}");
        await new Promise((resolve, reject) => {{
            ws.onopen = () => {{ ws.close(1000); }};
            ws.onclose = resolve;
            ws.onerror = (event) => reject(new Error("unexpected websocket error: " + (event.message || "")));
        }});
        "#,
        url = server.url(),
    );

    run_js(&engine, code).await.expect("connect should succeed");

    let handshakes = server.handshakes().await;
    assert_eq!(handshakes.len(), 1);
    assert_eq!(
        handshakes[0].authorization, None,
        "secret scoped to another host must not leak to this one"
    );
}

#[tokio::test]
async fn websocket_send_blob_with_nul_bytes_round_trips() {
    ensure_v8();
    let server = start_echo_server().await;
    let engine = build_engine(WebSocketConfig::new_with_chain(allow_all_chain()));

    // Mirrors WPT Send-binary-blob.any.js: a Blob of NUL bytes must survive
    // the round trip with its size intact.
    let code = format!(
        r#"
        const ws = new WebSocket("{url}");
        ws.binaryType = "blob";
        let data = "";
        for (let i = 0; i < 100; i++) data += String.fromCharCode(0);
        const blob = new Blob([data]);
        const received = await new Promise((resolve, reject) => {{
            ws.onopen = () => {{
                if (blob.size !== 100) {{
                    reject(new Error("sent blob size wrong: " + blob.size + " _data len " + (blob._data || "").length));
                    return;
                }}
                ws.send(blob);
            }};
            ws.onmessage = (event) => {{ resolve(event.data); ws.close(1000); }};
            ws.onerror = (event) => reject(new Error("ws error: " + (event.message || "")));
        }});
        if (!(received instanceof Blob)) throw new Error("expected Blob, got " + received);
        if (received.size !== 100) throw new Error("received blob size " + received.size);
        const bytes = new Uint8Array(await received.arrayBuffer());
        if (bytes.some((b) => b !== 0)) throw new Error("blob bytes corrupted");
        "#,
        url = server.url(),
    );

    run_js(&engine, code).await.expect("NUL-byte blob should round-trip");
}

#[tokio::test]
async fn websocket_global_absent_without_policy_config() {
    ensure_v8();
    // Engine with no websocket config: the class must not exist at all.
    let tmp = std::env::temp_dir().join(format!(
        "mcp-ws-absent-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    let engine =
        Engine::new_stateless(64 * 1024 * 1024, 30, 4).with_execution_registry(Arc::new(registry));

    let code = r#"
        if (typeof WebSocket !== "undefined") {
            throw new Error("WebSocket should be absent without a websocket policy");
        }
    "#
    .to_string();

    run_js(&engine, code).await.expect("check should succeed");
}

#[tokio::test]
async fn websocket_constructor_validation_matches_spec() {
    ensure_v8();
    let engine = build_engine(WebSocketConfig::new_with_chain(allow_all_chain()));

    let code = r#"
        function assertThrowsDom(fn, name, label) {
            try {
                fn();
            } catch (e) {
                if (e instanceof DOMException && e.name === name) return;
                throw new Error(label + ": expected DOMException " + name + ", got " + e);
            }
            throw new Error(label + ": expected an exception");
        }

        assertThrowsDom(() => new WebSocket("ftp://example.com/"), "SyntaxError", "bad scheme");
        assertThrowsDom(() => new WebSocket("ws://example.com/#frag"), "SyntaxError", "fragment");
        assertThrowsDom(() => new WebSocket("ws://example.com/", ["a", "a"]), "SyntaxError", "duplicate protocol");
        assertThrowsDom(() => new WebSocket("ws://example.com/", ["bad protocol"]), "SyntaxError", "invalid protocol token");

        // http(s) map onto ws(s) without connecting anywhere real.
        const ws = new WebSocket("http://255.255.255.255:9/");
        if (ws.url !== "ws://255.255.255.255:9/") throw new Error("http should map to ws: " + ws.url);
        if (ws.readyState !== WebSocket.CONNECTING) throw new Error("should start CONNECTING");
        assertThrowsDom(() => ws.send("x"), "InvalidStateError", "send while CONNECTING");
        try { ws.close(1005); throw new Error("close(1005) should throw"); }
        catch (e) { if (!(e instanceof DOMException) || e.name !== "InvalidAccessError") throw e; }
        ws.close();
        await new Promise((resolve) => { ws.onclose = resolve; });
    "#
    .to_string();

    run_js(&engine, code).await.expect("constructor validation should pass");
}
