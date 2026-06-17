//! CLI end-to-end integration tests
//!
//! These tests start the mcp-v8 server in stateless HTTP mode, then exercise
//! every mcp-v8-cli subcommand via `std::process::Command`, asserting correct
//! exit codes and output.
//!
//! # Prerequisites
//!
//! Both binaries must be built before running these tests:
//!
//! ```bash
//! # From the workspace root
//! cargo build --release --bin server   -p server
//! cargo build --release --bin mcp-v8-cli -p mcp-v8-client
//! ```
//!
//! # Running
//!
//! ```bash
//! # From the workspace root (or mcp-v8-client/)
//! cargo test --test cli_e2e -p mcp-v8-client -- --nocapture
//! ```
//!
//! To point the tests at a custom server binary set `SERVER_BIN`:
//!
//! ```bash
//! SERVER_BIN=/path/to/server cargo test --test cli_e2e -p mcp-v8-client
//! ```

use reqwest::blocking::Client;
use serde_json::Value;
use std::{
    env,
    net::TcpListener,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::Duration,
};

// ── Binary paths ─────────────────────────────────────────────────────────────

/// Path to the CLI binary under test (auto-resolved by Cargo test runner).
fn cli_bin() -> String {
    // CARGO_BIN_EXE_mcp-v8-cli is injected by Cargo for binaries in the same
    // package (mcp-v8-client).
    env!("CARGO_BIN_EXE_mcp-v8-cli").to_string()
}

/// Path to the server binary.
///
/// Resolved in priority order:
///   1. `SERVER_BIN` environment variable (explicit override)
///   2. `target/release/server` relative to the workspace root
fn server_bin() -> String {
    if let Ok(bin) = env::var("SERVER_BIN") {
        return bin;
    }
    // Walk up from the manifest dir to find the workspace root.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = std::path::Path::new(manifest_dir)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let candidate = workspace_root
        .join("target")
        .join("release")
        .join("server");
    if candidate.exists() {
        return candidate.to_string_lossy().to_string();
    }
    // Fallback: assume tests run from workspace root
    "target/release/server".to_string()
}

// ── Server lifecycle ──────────────────────────────────────────────────────────

struct TestServer {
    child: Child,
    pub base_url: String,
}

impl TestServer {
    /// Start the server in stateless HTTP mode on a random free port.
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = free_port();
        let bin = server_bin();

        let child = Command::new(&bin)
            .args(["--http-port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn server '{}': {}", bin, e))?;

        let base_url = format!("http://127.0.0.1:{}", port);

        // Poll until the server accepts requests (up to 15 s).
        let client = Client::builder()
            .timeout(Duration::from_millis(200))
            .build()?;
        let health = format!("{}/api/executions", base_url);
        for _ in 0..150 {
            if client.get(&health).send().is_ok() {
                return Ok(TestServer { child, base_url });
            }
            sleep(Duration::from_millis(100));
        }

        Err("Server did not become ready within 15 s".into())
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Return a currently free TCP port on 127.0.0.1.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("Failed to bind ephemeral port")
        .local_addr()
        .expect("No local addr")
        .port()
}

// ── CLI helpers ───────────────────────────────────────────────────────────────

/// Run the CLI with the given arguments and return (exit_code, stdout, stderr).
fn run_cli(base_url: &str, args: &[&str]) -> (i32, String, String) {
    let bin = cli_bin();
    let output = Command::new(&bin)
        .arg("--url")
        .arg(base_url)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("Failed to run CLI '{}': {}", bin, e));

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    (code, stdout, stderr)
}

/// Extract the first UUID-shaped token from `text`.
fn extract_uuid(text: &str) -> Option<String> {
    let re = regex_simple(text);
    re
}

fn regex_simple(text: &str) -> Option<String> {
    // Simple hand-rolled UUID extractor — avoids adding the `regex` dep.
    for word in text.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
        let parts: Vec<&str> = w.split('-').collect();
        if parts.len() == 5
            && parts[0].len() == 8
            && parts[1].len() == 4
            && parts[2].len() == 4
            && parts[3].len() == 4
            && parts[4].len() == 12
            && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return Some(w.to_string());
        }
    }
    None
}

/// Poll `executions get <id>` until status is Completed/Failed/TimedOut (or timeout).
/// Returns the final JSON response on success; panics on timeout.
fn poll_until_terminal(base_url: &str, id: &str, max_secs: u64) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(max_secs);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for execution {} to reach terminal state",
            id
        );
        let (code, stdout, _) = run_cli(base_url, &["--json", "executions", "get", id]);
        assert_eq!(code, 0, "executions get should exit 0");
        if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
            let status = v["status"].as_str().unwrap_or("").to_lowercase();
            match status.as_str() {
                "completed" | "failed" | "timedout" | "timed_out" => return v,
                _ => {}
            }
        }
        sleep(Duration::from_millis(500));
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Test 1 — `exec`: submits code, captures execution_id.
#[test]
fn test_exec_captures_execution_id() {
    let server = TestServer::start().expect("Failed to start server");

    let (code, stdout, _stderr) = run_cli(&server.base_url, &["exec", "console.log(42)"]);
    assert_eq!(code, 0, "exec should exit 0");

    let id = extract_uuid(&stdout).expect("Should find an execution_id UUID in exec output");
    assert!(!id.is_empty(), "execution_id should be non-empty");
}

/// Test 2 — `executions list`: exits 0 and produces output.
#[test]
fn test_executions_list() {
    let server = TestServer::start().expect("Failed to start server");

    // Submit something first so the list is never empty.
    run_cli(&server.base_url, &["exec", "1+1"]);

    let (code, stdout, _) = run_cli(&server.base_url, &["executions", "list"]);
    assert_eq!(code, 0, "executions list should exit 0");
    assert!(!stdout.trim().is_empty(), "executions list should produce output");
}

/// Test 3 — `executions get`: status field is present in the output.
#[test]
fn test_executions_get_has_status() {
    let server = TestServer::start().expect("Failed to start server");

    let (_, submit_out, _) = run_cli(&server.base_url, &["exec", "console.log(99)"]);
    let id = extract_uuid(&submit_out).expect("Should get execution_id from exec");

    let (code, stdout, _) = run_cli(&server.base_url, &["executions", "get", &id]);
    assert_eq!(code, 0, "executions get should exit 0");
    assert!(
        stdout.to_lowercase().contains("status"),
        "executions get output should contain 'status', got: {}",
        stdout
    );
}

/// Test 4 — `executions output`: output of `console.log(42)` contains "42".
#[test]
fn test_executions_output_contains_expected_value() {
    let server = TestServer::start().expect("Failed to start server");

    let (_, submit_out, _) = run_cli(&server.base_url, &["exec", "console.log(42)"]);
    let id = extract_uuid(&submit_out).expect("Should get execution_id from exec");

    // Wait for completion before reading output.
    poll_until_terminal(&server.base_url, &id, 30);

    let (code, stdout, _) = run_cli(&server.base_url, &["executions", "output", &id]);
    assert_eq!(code, 0, "executions output should exit 0");
    assert!(
        stdout.contains("42"),
        "Output should contain '42', got: {}",
        stdout
    );
}

/// Test 5 — exec + poll loop: `2+2` completes and output contains "4".
#[test]
fn test_exec_poll_until_completed() {
    let server = TestServer::start().expect("Failed to start server");

    let (_, submit_out, _) = run_cli(&server.base_url, &["exec", "console.log(2+2)"]);
    let id = extract_uuid(&submit_out).expect("Should get execution_id from exec");

    let final_resp = poll_until_terminal(&server.base_url, &id, 30);
    let status = final_resp["status"].as_str().unwrap_or("").to_lowercase();
    assert_eq!(status, "completed", "Execution should complete successfully");

    let (code, stdout, _) = run_cli(&server.base_url, &["executions", "output", &id]);
    assert_eq!(code, 0, "executions output should exit 0");
    assert!(
        stdout.contains('4'),
        "Output of 2+2 should contain '4', got: {}",
        stdout
    );
}

/// Test 6 — `--json` flag: `exec --json` produces valid JSON.
#[test]
fn test_exec_json_flag_produces_valid_json() {
    let server = TestServer::start().expect("Failed to start server");

    let (code, stdout, _) =
        run_cli(&server.base_url, &["--json", "exec", r#"console.log("json-test")"#]);
    assert_eq!(code, 0, "exec --json should exit 0");

    let parsed: Result<Value, _> = serde_json::from_str(&stdout);
    assert!(
        parsed.is_ok(),
        "exec --json output should be valid JSON, got: {}",
        stdout
    );
}

/// Test 7 — `executions cancel`: submit infinite loop, cancel it, assert exit 0.
#[test]
fn test_executions_cancel() {
    let server = TestServer::start().expect("Failed to start server");

    let (_, submit_out, _) = run_cli(&server.base_url, &["exec", "while(true){}"]);
    let id = extract_uuid(&submit_out).expect("Should get execution_id from exec");

    // Cancel immediately — don't wait for the execution to finish.
    let (cancel_code, _stdout, _stderr) =
        run_cli(&server.base_url, &["executions", "cancel", &id]);
    assert_eq!(cancel_code, 0, "executions cancel should exit 0");
}
