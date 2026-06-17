//! End-to-end tests for native MCP **tasks** support (rmcp 1.x, SEP-1319) over
//! the Streamable HTTP transport.
//!
//! These spawn the real server binary with `--http-port` (stateless mode) and
//! drive the `/mcp` endpoint with raw JSON-RPC, exercising the native task
//! flow: capability advertisement on `initialize`, a task-augmented `run_js`
//! `tools/call` (which returns a `CreateTaskResult`), and
//! `tasks/get` / `tasks/result` / `tasks/list` / `tasks/cancel`.

use reqwest::Client;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

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
                return Ok(Self { child: Some(child), base_url });
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

/// Initialize an MCP session; return (session_id, initialize_result_json).
async fn initialize(client: &Client, url: &str) -> (String, Value) {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "tasks-e2e", "version": "1.0.0" }
            }
        }))
        .send()
        .await
        .expect("initialize request");
    assert!(resp.status().is_success(), "initialize status: {}", resp.status());
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

// ── Tests ──────────────────────────────────────────────────────────────────

/// `initialize` advertises the `tasks` server capability.
#[tokio::test]
async fn advertises_tasks_capability() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();

    let (_session, init) = initialize(&c, &server.mcp_url()).await;
    assert!(
        !init["result"]["capabilities"]["tasks"].is_null(),
        "capabilities.tasks should be advertised: {init}"
    );

    server.stop().await;
}

/// Full happy path: a task-augmented run_js returns a working task that
/// progresses to completion; `tasks/result` yields the output and `tasks/list`
/// includes the task.
#[tokio::test]
async fn task_augmented_call_completes_and_returns_result() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();
    let (session, _) = initialize(&c, &url).await;

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
                "arguments": { "code": "console.log(6 * 7)" },
                "task": {}
            }
        }),
    )
    .await;

    let task = &create["result"]["task"];
    assert!(task.is_object(), "expected result.task (CreateTaskResult), got {create}");
    let task_id = task["taskId"].as_str().expect("taskId").to_string();

    // tasks/list includes the freshly-created task (checked before tasks/result,
    // which retrieves and consumes the task's payload).
    let list = rpc(
        &c,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tasks/list" }),
    )
    .await;
    let listed = list["result"]["tasks"]
        .as_array()
        .map(|arr| arr.iter().any(|t| t["taskId"] == task_id.as_str()))
        .unwrap_or(false);
    assert!(listed, "tasks/list should include the task: {list}");

    // Poll tasks/get until terminal.
    let mut final_status = String::new();
    for _ in 0..200 {
        let got = rpc(
            &c,
            &url,
            &session,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tasks/get",
                    "params": { "taskId": task_id } }),
        )
        .await;
        let status = got["result"]["status"].as_str().unwrap_or("").to_string();
        if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            final_status = status;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(final_status, "completed", "task should complete");

    // tasks/result returns the run_js tool result (output contains 42).
    let result = rpc(
        &c,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "tasks/result",
                "params": { "taskId": task_id } }),
    )
    .await;
    let text = serde_json::to_string(&result).unwrap_or_default();
    assert!(text.contains("42"), "tasks/result should carry run_js output 42: {result}");

    server.stop().await;
}

/// A normal (non-augmented) run_js call still returns its result directly and
/// does not create a task.
#[tokio::test]
async fn plain_tool_call_creates_no_task() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();
    let (session, _) = initialize(&c, &url).await;

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
    assert!(resp["result"]["task"].is_null(), "plain call must not be a task: {resp}");
    let text = serde_json::to_string(&resp).unwrap_or_default();
    assert!(text.contains("123"), "plain call should return output 123: {resp}");

    let list = rpc(
        &c,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tasks/list" }),
    )
    .await;
    assert_eq!(
        list["result"]["tasks"].as_array().map(Vec::len),
        Some(0),
        "no tasks should have been created: {list}"
    );

    server.stop().await;
}

/// `tasks/cancel` transitions a still-running task to `cancelled`.
#[tokio::test]
async fn cancel_transitions_task_to_cancelled() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();
    let (session, _) = initialize(&c, &url).await;

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
                "arguments": { "code": "while (true) {}", "execution_timeout_secs": 5 },
                "task": {}
            }
        }),
    )
    .await;
    let task_id = create["result"]["task"]["taskId"]
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
    // CancelTaskResult carries the (now cancelled) task state.
    assert_eq!(cancel["result"]["status"], "cancelled", "cancel result: {cancel}");

    server.stop().await;
}
