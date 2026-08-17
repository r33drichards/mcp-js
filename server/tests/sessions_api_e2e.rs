//! End-to-end tests for the named-session REST endpoints:
//! `GET /api/sessions` and `GET /api/sessions/{session}/history`.
//!
//! Named sessions are created per-call via the `session` field of
//! `POST /api/exec`, and per-session heap/fs state is resumed from the
//! session's latest history entry. These endpoints make those sessions
//! discoverable — before them, a REST caller could create named sessions
//! (persisting heaps and files server-side) but had no way to enumerate
//! them or inspect what state each one holds.

use reqwest::Client;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};

// ── Server helper ────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<Child>,
    base_url: String,
    // Owns the on-disk state (session db, heap dir) for the server's lifetime.
    _dir: tempfile::TempDir,
}

impl HttpServer {
    /// Spawn the real server binary with isolated state directories, plus any
    /// extra CLI args, and wait for the HTTP surface to come up.
    async fn start(extra_args: &[&str]) -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();
        let dir = tempfile::Builder::new()
            .prefix("mcp-sessions-e2e-")
            .tempdir()?;
        let db_path = dir.path().join("sessions").to_string_lossy().to_string();

        let mut args = vec![
            "--http-port".to_string(),
            port.to_string(),
            "--session-db-path".to_string(),
            db_path,
        ];
        args.extend(extra_args.iter().map(|s| s.to_string()));

        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;

        let base_url = format!("http://127.0.0.1:{}", port);
        let client = Client::new();
        let health = format!("{}/api/version", base_url);

        for _ in 0..150 {
            if client
                .get(&health)
                .timeout(Duration::from_millis(100))
                .send()
                .await
                .is_ok()
            {
                return Ok(Self { child: Some(child), base_url, _dir: dir });
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err("Server did not become ready within 15s".into())
    }

    /// Start with per-session heap persistence (session log enabled).
    async fn start_stateful(heap_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start(&["--heap-store", "dir", "--heap-dir", heap_dir]).await
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

/// Submit code under a session name and wait for a terminal status, which is
/// returned. The session log entry is written before the execution is marked
/// terminal, so a completed run is guaranteed to be visible in the history.
async fn exec_in_session(
    base_url: &str,
    client: &Client,
    code: &str,
    session: &str,
) -> String {
    let resp = client
        .post(format!("{}/api/exec", base_url))
        .json(&serde_json::json!({ "code": code, "session": session }))
        .send()
        .await
        .expect("POST /api/exec");
    assert_eq!(resp.status(), 202, "expected 202 Accepted");
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let exec_id = body["execution_id"].as_str().expect("execution_id").to_string();

    for _ in 0..100 {
        let info: serde_json::Value = client
            .get(format!("{}/api/executions/{}", base_url, exec_id))
            .send()
            .await
            .expect("GET execution")
            .json()
            .await
            .expect("parse execution JSON");
        let status = info["status"].as_str().unwrap_or("").to_string();
        if matches!(status.as_str(), "completed" | "failed" | "timed_out" | "cancelled") {
            return status;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("execution {exec_id} did not reach a terminal state");
}

async fn list_sessions(base_url: &str, client: &Client) -> Vec<String> {
    let body: serde_json::Value = client
        .get(format!("{}/api/sessions", base_url))
        .send()
        .await
        .expect("GET /api/sessions")
        .json()
        .await
        .expect("parse sessions JSON");
    body["sessions"]
        .as_array()
        .expect("sessions array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Named sessions created via POST /api/exec appear in GET /api/sessions,
/// and their history is browsable per session.
#[tokio::test]
async fn test_sessions_are_listed_and_history_browsable() {
    let heap_dir = tempfile::Builder::new()
        .prefix("mcp-sessions-e2e-heaps-")
        .tempdir()
        .expect("heap dir");
    let mut server = HttpServer::start_stateful(heap_dir.path().to_str().unwrap())
        .await
        .expect("server start");
    let client = Client::new();

    // No sessions before anything ran.
    assert!(list_sessions(&server.base_url, &client).await.is_empty());

    let status = exec_in_session(&server.base_url, &client, "console.log('one');", "alpha").await;
    assert_eq!(status, "completed");
    let status = exec_in_session(&server.base_url, &client, "console.log('two');", "alpha").await;
    assert_eq!(status, "completed");
    let status = exec_in_session(&server.base_url, &client, "console.log('own');", "beta").await;
    assert_eq!(status, "completed");

    let mut sessions = list_sessions(&server.base_url, &client).await;
    sessions.sort();
    assert_eq!(sessions, vec!["alpha", "beta"]);

    // Full history for one session, oldest first.
    let resp = client
        .get(format!("{}/api/sessions/alpha/history", server.base_url))
        .send()
        .await
        .expect("GET history");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("parse history JSON");
    assert_eq!(body["session"], "alpha");
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2);
    // The logged code is the post-type-strip source, which may carry a
    // trailing newline.
    assert_eq!(entries[0]["code"].as_str().map(str::trim_end), Some("console.log('one');"));
    assert_eq!(entries[1]["code"].as_str().map(str::trim_end), Some("console.log('two');"));
    // The second run resumed the session's heap from the first.
    let first_output = entries[0]["output_heap"].as_str().expect("output_heap");
    assert_eq!(first_output.len(), 64, "output_heap should be a SHA-256 hex hash");
    assert_eq!(entries[1]["input_heap"], *first_output);
    // Entries expose the fs snapshot field (null — no fs store configured).
    assert!(entries[0].as_object().unwrap().contains_key("output_fs"));

    // Field selection narrows each entry.
    let body: serde_json::Value = client
        .get(format!(
            "{}/api/sessions/alpha/history?fields=index,code",
            server.base_url
        ))
        .send()
        .await
        .expect("GET filtered history")
        .json()
        .await
        .expect("parse filtered JSON");
    let entries = body["entries"].as_array().expect("entries array");
    let obj = entries[0].as_object().unwrap();
    assert!(obj.contains_key("index"));
    assert!(obj.contains_key("code"));
    assert!(!obj.contains_key("output_heap"));

    // Unknown session names are a 404, not an empty 200.
    let resp = client
        .get(format!("{}/api/sessions/no-such-session/history", server.base_url))
        .send()
        .await
        .expect("GET unknown history");
    assert_eq!(resp.status(), 404);

    server.stop().await;
}

/// A run that fails before producing a snapshot must not register its
/// session name — only completed executions create history.
#[tokio::test]
async fn test_failed_runs_do_not_register_sessions() {
    let heap_dir = tempfile::Builder::new()
        .prefix("mcp-sessions-e2e-heaps-")
        .tempdir()
        .expect("heap dir");
    let mut server = HttpServer::start_stateful(heap_dir.path().to_str().unwrap())
        .await
        .expect("server start");
    let client = Client::new();

    let status =
        exec_in_session(&server.base_url, &client, "throw new Error('boom');", "gamma").await;
    assert_eq!(status, "failed");

    let sessions = list_sessions(&server.base_url, &client).await;
    assert!(
        !sessions.contains(&"gamma".to_string()),
        "failed run registered a phantom session: {:?}",
        sessions
    );

    server.stop().await;
}

/// Without heap or fs persistence there is no session log; the endpoint says
/// so instead of returning an empty list.
#[tokio::test]
async fn test_sessions_endpoint_without_session_log() {
    let mut server = HttpServer::start(&[]).await.expect("server start");
    let client = Client::new();

    let resp = client
        .get(format!("{}/api/sessions", server.base_url))
        .send()
        .await
        .expect("GET /api/sessions");
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    assert!(body["error"].as_str().unwrap_or("").contains("not configured"));

    server.stop().await;
}
