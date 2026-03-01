/// End-to-end integration tests for the async execution model with console.log
/// capture and streaming output via sled.
///
/// These tests start the actual server in stateless mode with an HTTP port,
/// then exercise the REST API:
///   POST /api/exec           → submit code, get execution_id
///   GET  /api/executions/:id → poll status/result
///   GET  /api/executions/:id/output → retrieve console output
///   GET  /api/executions     → list executions
///   POST /api/executions/:id/cancel → cancel a running execution

use reqwest::Client;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};

mod common;

// ── Server helper ────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<tokio::process::Child>,
    base_url: String,
}

impl HttpServer {
    /// Start the server in stateless mode on a random port with the REST API.
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();

        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(&["--stateless", "--http-port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        let base_url = format!("http://127.0.0.1:{}", port);

        // Poll until the server is ready (up to 15s).
        let client = Client::new();
        let health_url = format!("{}/api/executions", base_url);
        for _ in 0..150 {
            if client
                .get(&health_url)
                .timeout(Duration::from_millis(100))
                .send()
                .await
                .is_ok()
            {
                return Ok(HttpServer {
                    child: Some(child),
                    base_url,
                });
            }
            sleep(Duration::from_millis(100)).await;
        }

        Err("Server did not become ready within 15s".into())
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
            sleep(Duration::from_millis(500)).await;
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

// ── Helpers ──────────────────────────────────────────────────────────────

/// Submit code for execution and return the execution_id.
async fn submit_code(client: &Client, base_url: &str, code: &str) -> String {
    let resp = client
        .post(format!("{}/api/exec", base_url))
        .json(&json!({ "code": code }))
        .send()
        .await
        .expect("POST /api/exec failed");
    assert_eq!(resp.status(), 202, "Expected 202 Accepted");
    let body: Value = resp.json().await.expect("Invalid JSON from /api/exec");
    body["execution_id"]
        .as_str()
        .expect("Missing execution_id")
        .to_string()
}

/// Poll until an execution reaches a terminal state (completed/failed/timed_out/cancelled).
/// Returns the final JSON body from GET /api/executions/:id.
async fn poll_until_done(client: &Client, base_url: &str, id: &str) -> Value {
    let url = format!("{}/api/executions/{}", base_url, id);
    for _ in 0..100 {
        let resp = client
            .get(&url)
            .send()
            .await
            .expect("GET execution failed");
        let body: Value = resp.json().await.expect("Invalid JSON");
        let status = body["status"].as_str().unwrap_or("");
        if status != "running" {
            return body;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("Execution {} did not finish within 5s", id);
}

/// Fetch console output for an execution (line-offset mode by default).
async fn get_output(
    client: &Client,
    base_url: &str,
    id: &str,
    query: &str,
) -> Value {
    let url = format!("{}/api/executions/{}/output?{}", base_url, id, query);
    let resp = client.get(&url).send().await.expect("GET output failed");
    resp.json().await.expect("Invalid JSON from output endpoint")
}

// ── Tests ────────────────────────────────────────────────────────────────

/// Test that console.log output is captured and retrievable.
#[tokio::test]
async fn test_console_log_basic() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    let id = submit_code(
        &client,
        &server.base_url,
        r#"console.log("hello world"); 42;"#,
    )
    .await;

    let result = timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id)).await?;
    assert_eq!(result["status"], "completed", "Execution should complete");
    assert_eq!(result["result"], "42", "Return value should be 42");

    // Fetch console output
    let output = get_output(&client, &server.base_url, &id, "").await;
    let data = output["data"].as_str().expect("output.data should be a string");
    assert!(
        data.contains("hello world"),
        "Console output should contain 'hello world', got: {}",
        data
    );
    assert_eq!(output["total_lines"], 1, "Should have exactly 1 line of output");

    server.stop().await;
    Ok(())
}

/// Test all console methods: log, info, warn, error, debug, trace.
#[tokio::test]
async fn test_console_methods() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    let code = r#"
        console.log("LOG line");
        console.info("INFO line");
        console.warn("WARN line");
        console.error("ERROR line");
        console.debug("DEBUG line");
        console.trace("TRACE line");
        "done";
    "#;

    let id = submit_code(&client, &server.base_url, code).await;
    let result = timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id)).await?;
    assert_eq!(result["status"], "completed");

    let output = get_output(&client, &server.base_url, &id, "").await;
    let data = output["data"].as_str().unwrap();

    // console.log → plain text
    assert!(data.contains("LOG line"), "Should contain LOG line");
    // console.info → [INFO] prefix
    assert!(data.contains("[INFO] INFO line"), "Should contain [INFO] prefix");
    // console.warn → [WARN] prefix
    assert!(data.contains("[WARN] WARN line"), "Should contain [WARN] prefix");
    // console.error → [ERROR] prefix
    assert!(data.contains("[ERROR] ERROR line"), "Should contain [ERROR] prefix");
    // console.debug → plain text (same as log)
    assert!(data.contains("DEBUG line"), "Should contain DEBUG line");
    // console.trace → plain text (same as log)
    assert!(data.contains("TRACE line"), "Should contain TRACE line");

    assert_eq!(output["total_lines"], 6, "Should have 6 lines of output");

    server.stop().await;
    Ok(())
}

/// Test console.log with multiple arguments and object formatting.
#[tokio::test]
async fn test_console_log_formatting() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    let code = r#"
        console.log("hello", "world");
        console.log("num:", 42);
        console.log("obj:", {a: 1, b: 2});
        console.log("arr:", [1, 2, 3]);
        "ok";
    "#;

    let id = submit_code(&client, &server.base_url, code).await;
    let result = timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id)).await?;
    assert_eq!(result["status"], "completed");

    let output = get_output(&client, &server.base_url, &id, "").await;
    let data = output["data"].as_str().unwrap();

    assert!(data.contains("hello world"), "Multiple string args should be space-joined");
    assert!(data.contains("num: 42"), "Number arg should be stringified");
    assert!(data.contains(r#""a":1"#), "Object should be JSON-serialized");
    assert!(data.contains("[1,2,3]"), "Array should be JSON-serialized");

    server.stop().await;
    Ok(())
}

/// Test the async execution lifecycle: submit → running → completed.
#[tokio::test]
async fn test_execution_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    // Submit code — response should be 202 with an execution_id
    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .json(&json!({ "code": "1 + 1" }))
        .send()
        .await?;
    assert_eq!(resp.status(), 202);
    let body: Value = resp.json().await?;
    let id = body["execution_id"].as_str().expect("Should have execution_id");
    assert!(!id.is_empty());

    // Poll until done
    let result = timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, id)).await?;
    assert_eq!(result["status"], "completed");
    assert_eq!(result["result"], "2");
    assert!(result["started_at"].is_string());
    assert!(result["completed_at"].is_string());

    server.stop().await;
    Ok(())
}

/// Test listing executions.
#[tokio::test]
async fn test_list_executions() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    // Submit two executions
    let id1 = submit_code(&client, &server.base_url, "1").await;
    let id2 = submit_code(&client, &server.base_url, "2").await;

    // Wait for both to complete
    timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id1)).await?;
    timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id2)).await?;

    // List executions
    let resp = client
        .get(format!("{}/api/executions", server.base_url))
        .send()
        .await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    let executions = body["executions"].as_array().expect("Should have executions array");
    assert!(
        executions.len() >= 2,
        "Should list at least 2 executions, got {}",
        executions.len()
    );

    // Both IDs should appear
    let ids: Vec<&str> = executions
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert!(ids.contains(&id1.as_str()), "Should contain first execution");
    assert!(ids.contains(&id2.as_str()), "Should contain second execution");

    server.stop().await;
    Ok(())
}

/// Test console output pagination (line-offset mode).
#[tokio::test]
async fn test_console_output_line_pagination() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    // Generate 10 lines of output
    let code = r#"
        for (let i = 1; i <= 10; i++) {
            console.log("line " + i);
        }
        "done";
    "#;

    let id = submit_code(&client, &server.base_url, code).await;
    timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id)).await?;

    // Fetch first 3 lines
    let page1 = get_output(&client, &server.base_url, &id, "line_offset=1&line_limit=3").await;
    assert_eq!(page1["total_lines"], 10, "Should have 10 total lines");
    assert_eq!(page1["start_line"], 1);
    assert_eq!(page1["end_line"], 3);
    assert_eq!(page1["has_more"], true);
    let data1 = page1["data"].as_str().unwrap();
    assert!(data1.contains("line 1"), "First page should start with line 1");
    assert!(data1.contains("line 3"), "First page should include line 3");
    assert!(!data1.contains("line 4"), "First page should not include line 4");

    // Fetch next page using next_line_offset
    let next_offset = page1["next_line_offset"].as_u64().unwrap();
    let page2 = get_output(
        &client,
        &server.base_url,
        &id,
        &format!("line_offset={}&line_limit=3", next_offset),
    )
    .await;
    assert_eq!(page2["start_line"], 4);
    let data2 = page2["data"].as_str().unwrap();
    assert!(data2.contains("line 4"), "Second page should start with line 4");

    server.stop().await;
    Ok(())
}

/// Test console output byte-offset pagination.
#[tokio::test]
async fn test_console_output_byte_pagination() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    let code = r#"
        console.log("aaaa");
        console.log("bbbb");
        "ok";
    "#;

    let id = submit_code(&client, &server.base_url, code).await;
    timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id)).await?;

    // Fetch all output first to know total bytes
    let full = get_output(&client, &server.base_url, &id, "").await;
    let total_bytes = full["total_bytes"].as_u64().unwrap();
    assert!(total_bytes > 0, "Should have some bytes of output");

    // Fetch first 5 bytes via byte-offset mode
    let page = get_output(&client, &server.base_url, &id, "byte_offset=0&byte_limit=5").await;
    assert_eq!(page["start_byte"], 0);
    assert_eq!(page["end_byte"], 5);
    let data = page["data"].as_str().unwrap();
    assert_eq!(data.len(), 5, "Should return exactly 5 bytes");
    assert_eq!(data, "aaaa\n", "First 5 bytes should be 'aaaa\\n'");

    server.stop().await;
    Ok(())
}

/// Test that a failed execution reports the error correctly.
#[tokio::test]
async fn test_execution_failure() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    let id = submit_code(
        &client,
        &server.base_url,
        "throw new Error('boom');",
    )
    .await;

    let result = timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id)).await?;
    assert_eq!(result["status"], "failed", "Should be marked as failed");
    let error = result["error"].as_str().unwrap_or("");
    assert!(
        error.contains("boom"),
        "Error should mention 'boom', got: {}",
        error
    );

    server.stop().await;
    Ok(())
}

/// Test console output interleaved with computation and a return value.
#[tokio::test]
async fn test_console_log_with_return_value() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    let code = r#"
        console.log("starting computation");
        const result = Array.from({length: 5}, (_, i) => i + 1).reduce((a, b) => a + b, 0);
        console.log("result is", result);
        result;
    "#;

    let id = submit_code(&client, &server.base_url, code).await;
    let result = timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id)).await?;
    assert_eq!(result["status"], "completed");
    assert_eq!(result["result"], "15", "1+2+3+4+5 should equal 15");

    let output = get_output(&client, &server.base_url, &id, "").await;
    let data = output["data"].as_str().unwrap();
    assert!(data.contains("starting computation"));
    assert!(data.contains("result is 15"));
    assert_eq!(output["total_lines"], 2);

    server.stop().await;
    Ok(())
}

/// Test that console output for an execution with no console calls returns empty data.
#[tokio::test]
async fn test_no_console_output() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    let id = submit_code(&client, &server.base_url, "1 + 1").await;
    let result = timeout(Duration::from_secs(10), poll_until_done(&client, &server.base_url, &id)).await?;
    assert_eq!(result["status"], "completed");
    assert_eq!(result["result"], "2");

    let output = get_output(&client, &server.base_url, &id, "").await;
    assert_eq!(output["total_bytes"], 0, "Should have no console output bytes");
    assert_eq!(output["total_lines"], 0, "Should have no console output lines");

    server.stop().await;
    Ok(())
}
