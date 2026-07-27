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

/// Write a nono capability manifest granting `readwrite` and `read` paths
/// (skipping ones that don't exist — grants attach to real inodes) plus the
/// system paths a confined server needs, with all network access blocked.
fn write_manifest(dest: &Path, readwrite: &[&Path], read: &[&Path]) -> String {
    let mut grants = Vec::new();
    for path in readwrite {
        grants.push(json!({"path": path.to_str().unwrap(), "access": "readwrite"}));
    }
    let system: &[&str] = &[
        "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc", "/opt", "/nix",
        "/proc/self", "/dev/null", "/dev/urandom", "/dev/random",
        "/System", "/Library", "/private/etc",
    ];
    for path in read.iter().map(|p| p.to_str().unwrap()).chain(system.iter().copied()) {
        if Path::new(path).exists() {
            grants.push(json!({"path": path, "access": "read"}));
        }
    }
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
    std::fs::create_dir_all(&sessions).unwrap();
    let manifest = write_manifest(tmp.path(), &[&sessions], &[]);
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

    /// Call run_js with the given arguments and poll to a terminal state.
    async fn run_js_to_terminal(&mut self, id: u64, arguments: Value) -> Value {
        let response = self
            .send(json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": "run_js", "arguments": arguments}
            }))
            .await;
        let exec_id = common::extract_execution_id(&response)
            .unwrap_or_else(|| panic!("run_js response without execution_id: {response}"));

        for i in 0..200u64 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let poll = self
                .send(json!({
                    "jsonrpc": "2.0", "id": 100_000 + id * 1_000 + i,
                    "method": "tools/call",
                    "params": {"name": "get_execution", "arguments": {"execution_id": exec_id}}
                }))
                .await;
            if let Some(mut info) = common::extract_execution_info(&poll) {
                if matches!(
                    info["status"].as_str(),
                    Some("completed") | Some("failed") | Some("timed_out") | Some("cancelled")
                ) {
                    info["execution_id"] = json!(exec_id);
                    return info;
                }
            }
        }
        panic!("execution {exec_id} did not reach a terminal state");
    }

    async fn console_output(&mut self, id: u64, exec_id: &str) -> String {
        let response = self
            .send(json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": "get_execution_output",
                           "arguments": {"execution_id": exec_id, "line_limit": 10_000}}
            }))
            .await;
        common::extract_execution_info(&response)
            .and_then(|info| info["data"].as_str().map(str::to_string))
            .unwrap_or_default()
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
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::write(allowed.join("ok.js"), "console.log('granted-file-ran');").unwrap();
    std::fs::write(blocked.join("secret.js"), "console.log('SECRET-CONTENT');").unwrap();

    let manifest = write_manifest(tmp.path(), &[&sessions], &[&allowed]);
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
    let info = server.run_js_to_terminal(2, json!({"code": "console.log(6 * 7);"})).await;
    assert_eq!(info["status"], "completed", "sandboxed run_js failed: {info}");
    let exec_id = info["execution_id"].as_str().unwrap().to_string();
    let output = server.console_output(3, &exec_id).await;
    assert!(output.contains("42"), "expected console output, got: {output:?}");

    // A script inside the granted read path loads and runs.
    let info = server
        .run_js_to_terminal(4, json!({"file": allowed.join("ok.js").to_str().unwrap()}))
        .await;
    assert_eq!(info["status"], "completed", "granted-path run_js failed: {info}");
    let exec_id = info["execution_id"].as_str().unwrap().to_string();
    let output = server.console_output(5, &exec_id).await;
    assert!(output.contains("granted-file-ran"), "granted file did not run: {output:?}");

    // A script outside every grant is denied by the KERNEL, not by policy:
    // --allow-run-js-file makes the server itself willing to read any path,
    // so the only thing standing between run_js and the file is the sandbox.
    let response = server
        .send(json!({
            "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": {"name": "run_js",
                       "arguments": {"file": blocked.join("secret.js").to_str().unwrap()}}
        }))
        .await;
    let rendered = response.to_string();
    assert!(
        !rendered.contains("SECRET-CONTENT"),
        "ungranted file content leaked through the sandbox: {rendered}"
    );
    // The read fails before an execution is ever created (the file loader's
    // canonicalize/read gets EACCES from the kernel), so the call surfaces a
    // tool error rather than an execution id.
    assert!(
        common::extract_execution_id(&response).is_none(),
        "blocked script must not start an execution: {rendered}"
    );
    assert!(
        rendered.contains("run_js file"),
        "expected the file-read error to surface: {rendered}"
    );

    server.stop().await;
}
