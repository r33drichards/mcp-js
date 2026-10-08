//! End-to-end tests for native MCP **tasks** support (SEP-2663, the
//! `io.modelcontextprotocol/tasks` extension) over the Streamable HTTP
//! transport.
//!
//! These spawn the real server binary with `--http-port` (stateless mode) and
//! drive the `/mcp` endpoint with raw JSON-RPC, exercising the native task
//! flow: extension advertisement on `initialize`, task execution of `run_js`
//! for clients that declare the extension (`tools/call` returns a
//! `CreateTaskResult`), and `tasks/get` / `tasks/cancel`.

use reqwest::Client;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, sleep};

/// Extension id clients and servers declare for SEP-2663 tasks.
const TASKS_EXTENSION_ID: &str = "io.modelcontextprotocol/tasks";

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

// ── MCP client helpers ─────────────────────────────────────────────────────

const ACCEPT: &str = "application/json, text/event-stream";

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client")
}

/// Extract the JSON-RPC object from a Streamable HTTP POST response body, which
/// may be SSE-framed (`data:` lines) or a single JSON object.
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

/// Client capabilities declaring the SEP-2663 tasks extension.
fn tasks_capabilities() -> Value {
    json!({ "extensions": { TASKS_EXTENSION_ID: {} } })
}

/// Initialize an MCP session with the given client capabilities; return
/// (session_id, initialize_result_json).
async fn initialize_with(client: &Client, url: &str, capabilities: Value) -> (String, Value) {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": capabilities,
                "clientInfo": { "name": "tasks-e2e", "version": "1.0.0" }
            }
        }))
        .send()
        .await
        .expect("initialize request");
    assert!(
        resp.status().is_success(),
        "initialize status: {}",
        resp.status()
    );
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .expect("mcp-session-id header on initialize")
        .to_str()
        .unwrap()
        .to_string();
    let body = resp.text().await.expect("initialize body");
    let rpc = parse_rpc(&body);

    // Complete the handshake.
    client
        .post(url)
        .header("Accept", ACCEPT)
        .header("mcp-session-id", &session_id)
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await
        .expect("initialized notification");

    (session_id, rpc)
}

/// POST a JSON-RPC request and parse the response.
async fn rpc(client: &Client, url: &str, session: &str, message: Value) -> Value {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT)
        .header("mcp-session-id", session)
        .json(&message)
        .send()
        .await
        .expect("rpc request");
    assert!(resp.status().is_success(), "rpc status: {}", resp.status());
    let body = resp.text().await.expect("rpc body");
    parse_rpc(&body)
}

/// Poll `tasks/get` until the task reaches a terminal status; return the final
/// `tasks/get` response.
async fn poll_until_terminal(client: &Client, url: &str, session: &str, task_id: &str) -> Value {
    for _ in 0..200 {
        let got = rpc(
            client,
            url,
            session,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tasks/get",
                    "params": { "taskId": task_id } }),
        )
        .await;
        let status = got["result"]["status"].as_str().unwrap_or("");
        if matches!(status, "completed" | "failed" | "cancelled") {
            return got;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("task {task_id} did not reach a terminal status within 10s");
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// `initialize` advertises the tasks extension in `capabilities.extensions`.
#[tokio::test]
async fn advertises_tasks_capability() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();

    let (_session, init) = initialize_with(&c, &server.mcp_url(), tasks_capabilities()).await;
    assert!(
        init["result"]["capabilities"]["extensions"][TASKS_EXTENSION_ID].is_object(),
        "capabilities.extensions should advertise {TASKS_EXTENSION_ID}: {init}"
    );

    server.stop().await;
}

/// Full happy path: for a tasks-capable client, run_js returns a
/// `CreateTaskResult` that progresses to completion; the completed `tasks/get`
/// carries the tool result inline.
#[tokio::test]
async fn task_call_completes_and_returns_result() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();
    let (session, _) = initialize_with(&c, &url, tasks_capabilities()).await;

    let create = rpc(
        &c,
        &url,
        &session,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": { "code": "console.log(6 * 7)" }
            }
        }),
    )
    .await;

    // CreateTaskResult: resultType "task" with the seed Task flattened in.
    assert_eq!(
        create["result"]["resultType"], "task",
        "expected a CreateTaskResult, got {create}"
    );
    let task_id = create["result"]["taskId"]
        .as_str()
        .expect("taskId")
        .to_string();
    assert_eq!(create["result"]["status"], "working", "seed status: {create}");

    let done = poll_until_terminal(&c, &url, &session, &task_id).await;
    assert_eq!(
        done["result"]["status"], "completed",
        "task should complete: {done}"
    );

    // The completed task carries the run_js tool result inline (output 42).
    let text = serde_json::to_string(&done["result"]["result"]).unwrap_or_default();
    assert!(
        text.contains("42"),
        "completed tasks/get should carry run_js output 42: {done}"
    );

    server.stop().await;
}

/// A client that does not declare the tasks extension gets a synchronous
/// result and no task.
#[tokio::test]
async fn plain_tool_call_creates_no_task() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();
    let (session, _) = initialize_with(&c, &url, json!({})).await;

    let resp = rpc(
        &c,
        &url,
        &session,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "run_js", "arguments": { "code": "console.log(123)" } }
        }),
    )
    .await;
    // A normal call returns a CallToolResult (content), not a CreateTaskResult.
    assert_ne!(
        resp["result"]["resultType"], "task",
        "plain call must not be a task: {resp}"
    );
    assert!(
        resp["result"]["taskId"].is_null(),
        "plain call must not create a task: {resp}"
    );
    let text = serde_json::to_string(&resp).unwrap_or_default();
    assert!(
        text.contains("123"),
        "plain call should return output 123: {resp}"
    );

    server.stop().await;
}

/// `tasks/cancel` transitions a still-running task to `cancelled`.
#[tokio::test]
async fn cancel_transitions_task_to_cancelled() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();
    let (session, _) = initialize_with(&c, &url, tasks_capabilities()).await;

    let create = rpc(
        &c,
        &url,
        &session,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": { "code": "while (true) {}", "execution_timeout_secs": 5 }
            }
        }),
    )
    .await;
    let task_id = create["result"]["taskId"]
        .as_str()
        .expect("taskId")
        .to_string();

    let cancel = rpc(
        &c,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tasks/cancel",
                "params": { "taskId": task_id } }),
    )
    .await;
    assert!(
        cancel["error"].is_null(),
        "tasks/cancel should succeed: {cancel}"
    );

    // Cancellation is cooperative: the task settles as cancelled.
    let done = poll_until_terminal(&c, &url, &session, &task_id).await;
    assert_eq!(
        done["result"]["status"], "cancelled",
        "task should settle as cancelled: {done}"
    );

    server.stop().await;
}
