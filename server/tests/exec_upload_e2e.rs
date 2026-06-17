//! Integration tests for POST /api/exec accepting script uploads.
//!
//! `/api/exec` accepts two encodings, selected by `Content-Type`:
//!   - `application/json` (existing behaviour) — an ExecRequest body
//!   - any other type (e.g. `application/javascript`) — the raw request body is
//!     the script source (a file upload, `curl --data-binary @script.js`), with
//!     optional params passed as query-string parameters.
//!
//! These tests start the real server binary in stateless mode and exercise the
//! raw-upload path end-to-end, a JSON regression, and the multipart-rejection.

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

/// A raw-body upload with a non-JSON Content-Type runs the body as the script.
#[tokio::test]
async fn test_exec_raw_upload_runs_code() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .header(reqwest::header::CONTENT_TYPE, "application/javascript")
        .body("console.log(6 * 7);")
        .send()
        .await
        .expect("POST raw /api/exec");

    assert_eq!(resp.status(), 202, "expected 202 Accepted");
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let exec_id = body["execution_id"].as_str().expect("execution_id missing");

    let output = run_to_output(&server.base_url, &client, exec_id).await;
    assert!(output.contains("42"), "expected output to contain 42, got: {output:?}");

    server.stop().await;
}

/// Optional parameters ride along a raw upload as query-string params.
#[tokio::test]
async fn test_exec_raw_upload_with_query_params() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let resp = client
        .post(format!(
            "{}/api/exec?execution_timeout_secs=60&session=upload-test",
            server.base_url
        ))
        .header(reqwest::header::CONTENT_TYPE, "text/javascript")
        .body("console.log('hello from upload');")
        .send()
        .await
        .expect("POST raw /api/exec with query params");

    assert_eq!(resp.status(), 202);
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let exec_id = body["execution_id"].as_str().expect("execution_id missing");

    let output = run_to_output(&server.base_url, &client, exec_id).await;
    assert!(
        output.contains("hello from upload"),
        "expected greeting in output, got: {output:?}"
    );

    server.stop().await;
}

/// multipart/form-data is explicitly unsupported and returns 415 with guidance.
#[tokio::test]
async fn test_exec_multipart_is_415() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .header(
            reqwest::header::CONTENT_TYPE,
            "multipart/form-data; boundary=xyz",
        )
        .body("--xyz\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\nconsole.log(1)\r\n--xyz--\r\n")
        .send()
        .await
        .expect("POST multipart /api/exec");

    assert_eq!(resp.status(), 415);
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let error = body["error"].as_str().expect("error field");
    assert!(error.contains("multipart"), "got: {error}");

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
