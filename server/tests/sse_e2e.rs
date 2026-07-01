//! End-to-end test for the legacy HTTP+SSE transport (`--sse-port`), which is
//! served by the vendored rmcp 0.1.5 SSE server. Verifies that a run_js call
//! placed via `POST /message` produces its result on the `GET /sse` stream.
//!
//! The SSE transport does NOT support MCP tasks (that is the Streamable HTTP
//! transport, covered by tasks_e2e.rs).

use reqwest::Client;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};

struct SseServer {
    child: Option<tokio::process::Child>,
    base_url: String,
}

impl SseServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();
        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(["--sse-port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let base_url = format!("http://127.0.0.1:{}", port);

        // Readiness: the REST sidecar shares the port.
        let client = Client::new();
        for _ in 0..150 {
            if client
                .get(format!("{}/api/executions", base_url))
                .timeout(Duration::from_millis(100))
                .send()
                .await
                .is_ok()
            {
                return Ok(Self { child: Some(child), base_url });
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err("SSE server did not become ready within 15s".into())
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

impl Drop for SseServer {
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

/// Drive the SSE transport end to end and assert the run_js output appears on
/// the SSE stream.
#[tokio::test]
async fn sse_run_js_returns_output_on_stream() {
    let mut server = SseServer::start().await.expect("server start");

    // 1. Open the SSE stream and read the `endpoint` event giving the POST URL.
    let stream_client = Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let mut resp = stream_client
        .get(format!("{}/sse", server.base_url))
        .send()
        .await
        .expect("open sse stream");

    let mut buffer = String::new();
    let mut post_path = String::new();
    while let Ok(Some(chunk)) = timeout(Duration::from_secs(5), resp.chunk()).await.expect("sse chunk") {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        if let Some(line) = buffer.lines().find(|l| l.starts_with("data: ") && l.contains("sessionId=")) {
            post_path = line.trim_start_matches("data: ").to_string();
            break;
        }
        if buffer.len() > 4096 {
            break;
        }
    }
    assert!(!post_path.is_empty(), "should receive an endpoint event with sessionId");
    let post_url = format!("{}{}", server.base_url, post_path);

    // 2. Post initialize + initialized + a run_js tool call.
    let poster = Client::new();
    let send = |msg: Value| {
        let poster = poster.clone();
        let url = post_url.clone();
        async move {
            poster
                .post(&url)
                .json(&msg)
                .send()
                .await
                .expect("post message")
                .status()
        }
    };

    let status = send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": { "name": "sse-e2e", "version": "1.0.0" } }
    })).await;
    assert!(status.is_success(), "initialize POST status: {status}");

    let _ = send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;

    let status = send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "run_js", "arguments": { "code": "console.log(6 * 7)" } }
    })).await;
    assert!(status.is_success(), "tools/call POST status: {status}");

    // 3. Read the SSE stream until the run_js result (containing 42) arrives.
    let mut found = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(5), resp.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                if buffer.contains("\"id\":2") && buffer.contains("42") {
                    found = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(found, "run_js result (id=2, output 42) should arrive on the SSE stream; got:\n{buffer}");

    server.stop().await;
}
