//! End-to-end tests for session-less, per-request-negotiated MCP over the
//! Streamable HTTP transport (protocol `2026-07-28`, SEP-2567/SEP-2575).
//!
//! rmcp serves these POSTs directly alongside legacy stateful sessions on the
//! same `/mcp` endpoint (`legacy_session_mode` default). Each request carries
//! its own negotiation metadata (`_meta.io.modelcontextprotocol/*`) and the
//! SEP-2243 standard headers (`MCP-Protocol-Version`, `Mcp-Method`,
//! `Mcp-Name`); no `initialize` handshake and no `Mcp-Session-Id` are used.
//!
//! This is the wire shape session-less clients (e.g. ChatGPT connectors on
//! the 2026-07-28 revision) speak, so these tests pin the dual-mode endpoint
//! behavior the Streamable HTTP transport is configured for.

use reqwest::Client;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, sleep};

const TASKS_EXTENSION_ID: &str = "io.modelcontextprotocol/tasks";
const PROTOCOL_VERSION: &str = "2026-07-28";

// ── Server harness ─────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<tokio::process::Child>,
    base_url: String,
}

impl HttpServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();
        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(["--http-port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let base_url = format!("http://127.0.0.1:{}", port);
        let client = Client::new();
        let health = format!("{}/api/executions", base_url);
        for _ in 0..150 {
            if client
                .get(&health)
                .timeout(Duration::from_millis(100))
                .send()
                .await
                .is_ok()
            {
                return Ok(Self {
                    child: Some(child),
                    base_url,
                });
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err("server did not become ready within 15s".into())
    }

    fn mcp_url(&self) -> String {
        format!("{}/mcp", self.base_url)
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }
}

fn find_available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// ── Session-less request helpers ───────────────────────────────────────────

const ACCEPT: &str = "application/json, text/event-stream";

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client")
}

/// Extract the JSON-RPC object from a Streamable HTTP POST response body,
/// which may be SSE-framed (`data:` lines) or a single JSON object.
fn parse_rpc(body: &str) -> Value {
    if body.contains("data:") {
        let mut data = String::new();
        for line in body.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        serde_json::from_str(&data).unwrap_or(Value::Null)
    } else {
        serde_json::from_str(body).unwrap_or(Value::Null)
    }
}

/// The SEP-2575 per-request client context every session-less request carries.
fn request_meta(tasks: bool) -> Value {
    let capabilities = if tasks {
        json!({ "extensions": { TASKS_EXTENSION_ID: {} } })
    } else {
        json!({})
    };
    json!({
        "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientInfo": { "name": "sessionless-e2e", "version": "1.0.0" },
        "io.modelcontextprotocol/clientCapabilities": capabilities,
    })
}

/// POST one self-contained session-less JSON-RPC request: no session header,
/// SEP-2243 standard headers (`Mcp-Method`, and `Mcp-Name` naming the tool or
/// task), per-request `_meta` negotiation.
async fn sessionless_rpc(
    client: &Client,
    url: &str,
    method: &str,
    name: &str,
    message: Value,
) -> Value {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT)
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .header("Mcp-Method", method)
        .header("Mcp-Name", name)
        .json(&message)
        .send()
        .await
        .expect("sessionless rpc request");
    assert!(
        resp.status().is_success(),
        "sessionless rpc status: {}",
        resp.status()
    );
    let body = resp.text().await.expect("rpc body");
    parse_rpc(&body)
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// A single session-less POST (no initialize, no session id) negotiates
/// per-request and returns the tool result synchronously.
#[tokio::test]
async fn sessionless_tool_call_returns_result() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();

    let resp = sessionless_rpc(
        &c,
        &url,
        "tools/call",
        "run_js",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": { "code": "console.log(6 * 7)" },
                "_meta": request_meta(false)
            }
        }),
    )
    .await;

    assert!(
        resp["error"].is_null(),
        "sessionless tools/call should succeed: {resp}"
    );
    assert_ne!(
        resp["result"]["resultType"], "task",
        "without the tasks extension the call must be synchronous: {resp}"
    );
    let text = serde_json::to_string(&resp).unwrap_or_default();
    assert!(text.contains("42"), "should carry run_js output 42: {resp}");

    server.stop().await;
}

/// Session-less and legacy stateful clients are served by the same `/mcp`
/// endpoint concurrently.
#[tokio::test]
async fn sessionless_and_legacy_share_one_endpoint() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();

    // Legacy stateful client: initialize handshake issues a session id.
    let init = c
        .post(&url)
        .header("Accept", ACCEPT)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "legacy-e2e", "version": "1.0.0" }
            }
        }))
        .send()
        .await
        .expect("initialize request");
    assert!(init.status().is_success(), "initialize: {}", init.status());
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .expect("legacy initialize should issue mcp-session-id")
        .to_str()
        .unwrap()
        .to_string();
    let init_rpc = parse_rpc(&init.text().await.expect("initialize body"));
    assert_eq!(
        init_rpc["result"]["protocolVersion"], "2025-06-18",
        "legacy client negotiates its requested version: {init_rpc}"
    );
    c.post(&url)
        .header("Accept", ACCEPT)
        .header("mcp-session-id", &session_id)
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await
        .expect("initialized notification");

    // Session-less client on the same endpoint, between legacy requests.
    let sessionless = sessionless_rpc(
        &c,
        &url,
        "tools/call",
        "run_js",
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": { "code": "console.log('sessionless')" },
                "_meta": request_meta(false)
            }
        }),
    )
    .await;
    assert!(
        serde_json::to_string(&sessionless)
            .unwrap_or_default()
            .contains("sessionless"),
        "sessionless call should succeed on the shared endpoint: {sessionless}"
    );

    // The legacy session still works after the session-less request.
    let legacy = c
        .post(&url)
        .header("Accept", ACCEPT)
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "run_js", "arguments": { "code": "console.log('legacy')" } }
        }))
        .send()
        .await
        .expect("legacy tools/call");
    assert!(legacy.status().is_success(), "legacy: {}", legacy.status());
    let legacy_rpc = parse_rpc(&legacy.text().await.expect("legacy body"));
    assert!(
        serde_json::to_string(&legacy_rpc)
            .unwrap_or_default()
            .contains("legacy"),
        "legacy session call should still succeed: {legacy_rpc}"
    );

    server.stop().await;
}

/// A task created by one session-less POST is observable from later
/// session-less POSTs (the task store outlives the per-request service).
#[tokio::test]
async fn sessionless_task_spans_requests() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();

    let create = sessionless_rpc(
        &c,
        &url,
        "tools/call",
        "run_js",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": { "code": "console.log(1234)" },
                "_meta": request_meta(true)
            }
        }),
    )
    .await;
    assert_eq!(
        create["result"]["resultType"], "task",
        "tasks-capable sessionless call should create a task: {create}"
    );
    let task_id = create["result"]["taskId"]
        .as_str()
        .expect("taskId")
        .to_string();

    // Poll from fresh session-less requests until terminal.
    for _ in 0..200 {
        let got = sessionless_rpc(
            &c,
            &url,
            "tasks/get",
            &task_id,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tasks/get",
                "params": { "taskId": task_id, "_meta": request_meta(true) }
            }),
        )
        .await;
        match got["result"]["status"].as_str() {
            Some("completed") => {
                let text = serde_json::to_string(&got["result"]["result"]).unwrap_or_default();
                assert!(
                    text.contains("1234"),
                    "completed task should carry run_js output: {got}"
                );
                server.stop().await;
                return;
            }
            Some("failed") | Some("cancelled") => {
                panic!("task ended in unexpected status: {got}");
            }
            _ => sleep(Duration::from_millis(50)).await,
        }
    }
    panic!("task {task_id} did not complete within 10s");
}
