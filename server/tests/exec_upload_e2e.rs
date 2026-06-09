//! Integration tests for POST /api/exec accepting file uploads.
//!
//! `/api/exec` accepts two encodings:
//!   - `application/json` (existing behaviour) — an ExecRequest body
//!   - `multipart/form-data` — the code is uploaded as a `file` (or `code`) part
//!
//! These tests start the real server binary in stateless mode and exercise the
//! multipart path end-to-end, plus a JSON regression check and the error case.

use reqwest::Client;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

// ── Server helper ────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<tokio::process::Child>,
    base_url: String,
}

impl HttpServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();

        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(["--stateless", "--http-port", &port.to_string()])
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
        Err("Server did not become ready within 15s".into())
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
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

/// Poll an execution until it reaches a terminal state, then return its
/// collected console output.
async fn run_to_output(base_url: &str, client: &Client, execution_id: &str) -> String {
    let mut status = String::new();
    for _ in 0..100 {
        let info: serde_json::Value = client
            .get(format!("{}/api/executions/{}", base_url, execution_id))
            .send()
            .await
            .expect("GET execution")
            .json()
            .await
            .expect("parse execution JSON");
        status = info["status"].as_str().unwrap_or("").to_string();
        if matches!(status.as_str(), "completed" | "failed" | "timed_out" | "cancelled") {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(status, "completed", "execution did not complete: status={status}");

    let page: serde_json::Value = client
        .get(format!("{}/api/executions/{}/output", base_url, execution_id))
        .send()
        .await
        .expect("GET output")
        .json()
        .await
        .expect("parse output JSON");
    page["data"].as_str().unwrap_or("").to_string()
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Uploading the code as a `file` part runs it and produces console output.
#[tokio::test]
async fn test_exec_multipart_file_upload_runs_code() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::text("console.log(6 * 7);")
            .file_name("script.js")
            .mime_str("application/javascript")
            .unwrap(),
    );

    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .multipart(form)
        .send()
        .await
        .expect("POST multipart /api/exec");

    assert_eq!(resp.status(), 202, "expected 202 Accepted");
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let exec_id = body["execution_id"].as_str().expect("execution_id missing");

    let output = run_to_output(&server.base_url, &client, exec_id).await;
    assert!(output.contains("42"), "expected output to contain 42, got: {output:?}");

    server.stop().await;
}

/// The optional `code` text part is an alias for `file` and also carries
/// extra fields (here, an execution timeout).
#[tokio::test]
async fn test_exec_multipart_code_part_with_fields() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let form = reqwest::multipart::Form::new()
        .text("code", "console.log('hello from multipart');")
        .text("execution_timeout_secs", "60");

    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .multipart(form)
        .send()
        .await
        .expect("POST multipart /api/exec");

    assert_eq!(resp.status(), 202);
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let exec_id = body["execution_id"].as_str().expect("execution_id missing");

    let output = run_to_output(&server.base_url, &client, exec_id).await;
    assert!(
        output.contains("hello from multipart"),
        "expected greeting in output, got: {output:?}"
    );

    server.stop().await;
}

/// A multipart request without a `file` or `code` part is a 400.
#[tokio::test]
async fn test_exec_multipart_missing_code_is_400() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let form = reqwest::multipart::Form::new().text("session", "abc");

    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .multipart(form)
        .send()
        .await
        .expect("POST multipart /api/exec");

    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let error = body["error"].as_str().expect("error field");
    assert!(error.contains("file") || error.contains("code"), "got: {error}");

    server.stop().await;
}

/// The JSON encoding still works (regression).
#[tokio::test]
async fn test_exec_json_still_works() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .json(&serde_json::json!({ "code": "console.log(1 + 1);" }))
        .send()
        .await
        .expect("POST json /api/exec");

    assert_eq!(resp.status(), 202);
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let exec_id = body["execution_id"].as_str().expect("execution_id missing");

    let output = run_to_output(&server.base_url, &client, exec_id).await;
    assert!(output.contains("2"), "expected output to contain 2, got: {output:?}");

    server.stop().await;
}
