//! End-to-end tests for MCP **tasks** support (spec 2025-11-25) over the
//! Streamable HTTP transport.
//!
//! These spawn the real server binary with `--http-port` (stateless mode) and
//! drive the `/mcp` endpoint with raw JSON-RPC, exercising the task shim:
//! capability advertisement on `initialize`, task-augmented `tools/call`,
//! and `tasks/get` / `tasks/result` / `tasks/list` / `tasks/cancel`.

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

/// Initialize an MCP session and return (session_id, initialize_result).
async fn initialize(client: &Client, url: &str) -> (String, Value) {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
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
    let body: Value = resp.json().await.expect("initialize body is JSON");

    // Complete the handshake.
    client
        .post(url)
        .header("Accept", ACCEPT)
        .header("mcp-session-id", &session_id)
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await
        .expect("initialized notification");

    (session_id, body)
}

/// POST a JSON-RPC request and parse the (JSON) response. Used for the shim's
/// own methods, which always answer with `application/json`.
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
    resp.json().await.expect("rpc response is JSON")
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// `initialize` advertises the `tasks` server capability.
#[tokio::test]
async fn advertises_tasks_capability() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let (_session, init) = initialize(&client, &server.mcp_url()).await;
    let tasks = &init["result"]["capabilities"]["tasks"];
    assert!(tasks.is_object(), "capabilities.tasks missing: {init}");
    assert!(tasks["list"].is_object(), "tasks.list missing");
    assert!(tasks["cancel"].is_object(), "tasks.cancel missing");
    assert!(
        tasks["requests"]["tools"]["call"].is_object(),
        "tasks.requests.tools.call missing"
    );

    server.stop().await;
}

/// Full happy path: a task-augmented `tools/call` returns a working task, which
/// progresses to `completed`; `tasks/result` then yields the run_js output and
/// `tasks/list` includes the task.
#[tokio::test]
async fn task_augmented_call_completes_and_returns_result() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();
    let url = server.mcp_url();

    let (session, _) = initialize(&client, &url).await;

    // Task-augmented tools/call → CreateTaskResult.
    let create = rpc(
        &client,
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
    assert!(task.is_object(), "expected result.task, got {create}");
    let task_id = task["taskId"].as_str().expect("taskId").to_string();
    assert_eq!(task["status"], "working", "task must start working");
    assert!(task["createdAt"].is_string());
    assert!(task["ttl"].is_number());

    // Poll tasks/get until terminal.
    let mut final_status = String::new();
    for _ in 0..100 {
        let got = rpc(
            &client,
            &url,
            &session,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tasks/get",
                    "params": { "taskId": task_id } }),
        )
        .await;
        let status = got["result"]["status"].as_str().unwrap_or("").to_string();
        assert_eq!(got["result"]["taskId"], task_id.as_str());
        if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            final_status = status;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(final_status, "completed", "task should complete");

    // tasks/result returns exactly what run_js would have returned.
    let result = rpc(
        &client,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tasks/result",
                "params": { "taskId": task_id } }),
    )
    .await;
    let content_text = result["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        content_text.contains("42"),
        "run_js output should contain 42, got: {result}"
    );

    // tasks/list includes the task.
    let list = rpc(
        &client,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "tasks/list" }),
    )
    .await;
    let listed = list["result"]["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .any(|t| t["taskId"] == task_id.as_str());
    assert!(listed, "tasks/list should include the task: {list}");

    server.stop().await;
}

/// An unknown taskId yields a JSON-RPC -32602 (Invalid params) error.
#[tokio::test]
async fn unknown_task_id_is_invalid_params() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();
    let url = server.mcp_url();
    let (session, _) = initialize(&client, &url).await;

    let got = rpc(
        &client,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 9, "method": "tasks/get",
                "params": { "taskId": "does-not-exist" } }),
    )
    .await;
    assert_eq!(got["error"]["code"], -32602, "expected invalid params: {got}");

    server.stop().await;
}

/// `tasks/cancel` transitions a still-running task to `cancelled`, and
/// `tasks/result` then replays the cancellation as a JSON-RPC error.
#[tokio::test]
async fn cancel_transitions_task_to_cancelled() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();
    let url = server.mcp_url();
    let (session, _) = initialize(&client, &url).await;

    // Long-running JS keeps the task in `working` until we cancel it.
    let create = rpc(
        &client,
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
        &client,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tasks/cancel",
                "params": { "taskId": task_id } }),
    )
    .await;
    assert_eq!(cancel["result"]["status"], "cancelled", "cancel result: {cancel}");

    let got = rpc(
        &client,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tasks/get",
                "params": { "taskId": task_id } }),
    )
    .await;
    assert_eq!(got["result"]["status"], "cancelled");

    // tasks/result returns the run_js outcome flagged as an error (the
    // execution did not complete successfully).
    let result = rpc(
        &client,
        &url,
        &session,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "tasks/result",
                "params": { "taskId": task_id } }),
    )
    .await;
    assert_eq!(
        result["result"]["isError"], true,
        "cancelled task result should be flagged isError: {result}"
    );

    server.stop().await;
}

/// A normal (non-augmented) `tools/call` still works and is NOT turned into a
/// task — the shim only intercepts calls carrying `params.task`.
#[tokio::test]
async fn plain_tool_call_is_not_a_task() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();
    let url = server.mcp_url();
    let (session, _) = initialize(&client, &url).await;

    // No `task` field → forwarded to rmcp, returns the tool result directly
    // (over SSE). We just assert it isn't a CreateTaskResult and lists empty.
    let resp = client
        .post(&url)
        .header("Accept", ACCEPT)
        .header("mcp-session-id", &session)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "run_js", "arguments": { "code": "console.log(1)" } }
        }))
        .send()
        .await
        .expect("plain tool call");
    assert!(resp.status().is_success());

    let list = rpc(
        &client,
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
