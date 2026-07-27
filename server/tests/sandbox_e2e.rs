//! End-to-end tests for the OS sandbox layer (`--sandbox-manifest`, nono:
//! Landlock/Seatbelt).
//!
//! The sandbox needs kernel support (Landlock on Linux 5.13+, Seatbelt on
//! macOS), which CI containers may lack. Each test probes support first:
//!
//! - unsupported host → assert the fail-closed contract: `--sandbox-manifest`
//!   must abort startup with a clear error instead of running unconfined.
//! - supported host → assert the server still executes JS while confined, and
//!   that the kernel actually denies reads outside the granted capability set
//!   (via `run_js`'s `file` parameter pointed at an ungranted path).

use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::{Duration, timeout};

mod common;

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn sandbox_supported() -> bool {
    nono::Sandbox::support_info().is_supported
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sandbox_supported() -> bool {
    false
}

/// Write a nono capability manifest that grants only the given extra read
/// paths, with all network access blocked. Everything the server itself
/// needs (storage dirs, system paths) comes from the composed baseline, so
/// the manifest stays this small on purpose — the tests double as proof
/// that composition works.
fn write_manifest(dest: &Path, read: &[&Path]) -> String {
    let grants: Vec<_> = read
        .iter()
        .map(|path| json!({"path": path.to_str().unwrap(), "access": "read"}))
        .collect();
    let manifest = json!({
        "version": "0.1.0",
        "filesystem": {"grants": grants},
        "network": {"mode": "blocked"}
    });
    let file = dest.join("sandbox-manifest.json");
    std::fs::write(&file, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    file.to_str().unwrap().to_string()
}

/// On hosts that cannot enforce the sandbox, `--sandbox-manifest` must fail
/// closed: startup aborts with an explanatory error rather than serving
/// unconfined.
#[tokio::test]
async fn sandbox_fails_closed_when_unsupported() {
    if sandbox_supported() {
        eprintln!("skipping: this host supports the OS sandbox");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let sessions = tmp.path().join("sessions");
    let manifest = write_manifest(tmp.path(), &[]);
    let output = Command::new(env!("CARGO_BIN_EXE_server"))
        .args([
            "--sandbox-manifest",
            &manifest,
            "--session-db-path",
            sessions.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .expect("spawn server");

    assert!(
        !output.status.success(),
        "--sandbox-manifest must abort startup on a host that cannot enforce it"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot enforce"),
        "error should explain the fail-closed contract, got: {stderr}"
    );
}

/// Manifest options that only work under nono's CLI supervisor must abort
/// startup on every host — never be silently ignored.
#[tokio::test]
async fn supervisor_only_manifest_options_abort_startup() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("proxy-manifest.json");
    std::fs::write(
        &file,
        r#"{"version":"0.1.0","network":{"mode":"proxy","allow_domains":[".example.com"]}}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_server"))
        .args([
            "--sandbox-manifest",
            file.to_str().unwrap(),
            "--session-db-path",
            tmp.path().join("sessions").to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .expect("spawn server");

    assert!(!output.status.success(), "proxy-mode manifest must abort startup");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("supervisor") && stderr.contains("proxy"),
        "error should name the unsupported options, got: {stderr}"
    );
}

struct StdioServer {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl StdioServer {
    async fn start(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        tokio::time::sleep(Duration::from_millis(500)).await;
        StdioServer { child, stdin, stdout }
    }

    async fn send(&mut self, message: Value) -> Value {
        let line = format!("{}\n", serde_json::to_string(&message).unwrap());
        self.stdin.write_all(line.as_bytes()).await.expect("write");
        self.stdin.flush().await.expect("flush");
        let mut response = String::new();
        timeout(Duration::from_secs(10), self.stdout.read_line(&mut response))
            .await
            .expect("response timeout")
            .expect("read");
        serde_json::from_str(&response).expect("parse response")
    }

    async fn notify(&mut self, message: Value) {
        let line = format!("{}\n", serde_json::to_string(&message).unwrap());
        self.stdin.write_all(line.as_bytes()).await.expect("write");
        self.stdin.flush().await.expect("flush");
    }

    async fn initialize(&mut self) {
        let response = self
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "sandbox-e2e", "version": "1.0.0"}
                }
            }))
            .await;
        assert!(response["result"].is_object(), "initialize failed: {response}");
        self.notify(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
    }

    /// Call run_js and return `(is_error, text)`. Handles both response
    /// shapes: a synchronous inline result (`{"output":"..."}` in the tool
    /// content) and an asynchronous `execution_id`, which is polled to a
    /// terminal state and resolved to its console output.
    async fn run_js_text(&mut self, id: u64, arguments: Value) -> (bool, String) {
        let response = self
            .send(json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": "run_js", "arguments": arguments}
            }))
            .await;

        let Some(exec_id) = common::extract_execution_id(&response) else {
            let is_error = response["result"]["isError"].as_bool().unwrap_or(false)
                || response.get("error").is_some();
            let text = response["result"]["content"][0]["text"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| response.to_string());
            return (is_error, text);
        };

        for i in 0..200u64 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let poll = self
                .send(json!({
                    "jsonrpc": "2.0", "id": 100_000 + id * 1_000 + i,
                    "method": "tools/call",
                    "params": {"name": "get_execution", "arguments": {"execution_id": exec_id}}
                }))
                .await;
            if let Some(info) = common::extract_execution_info(&poll) {
                if matches!(
                    info["status"].as_str(),
                    Some("completed") | Some("failed") | Some("timed_out") | Some("cancelled")
                ) {
                    let is_error = info["status"] != "completed";
                    let output = self
                        .send(json!({
                            "jsonrpc": "2.0", "id": 500_000 + id, "method": "tools/call",
                            "params": {"name": "get_execution_output",
                                       "arguments": {"execution_id": exec_id, "line_limit": 10_000}}
                        }))
                        .await;
                    let console = common::extract_execution_info(&output)
                        .and_then(|o| o["data"].as_str().map(str::to_string))
                        .unwrap_or_default();
                    return (is_error, format!("{info} {console}"));
                }
            }
        }
        panic!("execution {exec_id} did not reach a terminal state");
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

/// While confined, the server must keep executing JS normally, read `run_js`
/// scripts from granted paths, and be *kernel-denied* on ungranted paths.
#[tokio::test]
async fn sandboxed_server_executes_js_and_kernel_denies_ungranted_reads() {
    if !sandbox_supported() {
        eprintln!("skipping: this host cannot enforce the OS sandbox");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let sessions = tmp.path().join("sessions");
    let allowed = tmp.path().join("allowed");
    let blocked = tmp.path().join("blocked");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::write(allowed.join("ok.js"), "console.log('granted-file-ran');").unwrap();
    std::fs::write(blocked.join("secret.js"), "console.log('SECRET-CONTENT');").unwrap();

    // The manifest grants only the script root; the session db and system
    // paths come from the composed server baseline.
    let manifest = write_manifest(tmp.path(), &[&allowed]);
    let mut server = StdioServer::start(&[
        "--sandbox-manifest",
        &manifest,
        "--session-db-path",
        sessions.to_str().unwrap(),
        "--allow-run-js-file",
    ])
    .await;
    server.initialize().await;

    // Plain execution still works under confinement (V8, sled, transports).
    let (is_error, text) = server.run_js_text(2, json!({"code": "console.log(6 * 7);"})).await;
    assert!(!is_error, "sandboxed run_js failed: {text}");
    assert!(text.contains("42"), "expected console output, got: {text:?}");

    // A script inside the granted read path loads and runs.
    let (is_error, text) = server
        .run_js_text(4, json!({"file": allowed.join("ok.js").to_str().unwrap()}))
        .await;
    assert!(!is_error, "granted-path run_js failed: {text}");
    assert!(text.contains("granted-file-ran"), "granted file did not run: {text:?}");

    // A script outside every grant is denied by the KERNEL, not by policy:
    // --allow-run-js-file makes the server itself willing to read any path,
    // so the only thing standing between run_js and the file is the sandbox.
    let (is_error, text) = server
        .run_js_text(6, json!({"file": blocked.join("secret.js").to_str().unwrap()}))
        .await;
    assert!(
        !text.contains("SECRET-CONTENT"),
        "ungranted file content leaked through the sandbox: {text}"
    );
    assert!(is_error, "reading an ungranted path must fail: {text}");

    server.stop().await;
}
