//! Reproduction: heap persistence + a large session working set crashes the
//! whole server process at snapshot time, killing every session.
//!
//! Symptom (reported from the pi-irc deployment, image built from `main`):
//! with `--heap-store dir` (heap persistence on) alongside `--fs-store dir`, a
//! `run_js` that performs a large `isomorphic-git` clone into the session
//! filesystem makes the engine abort when it serializes the post-run V8 heap
//! snapshot. The abort is a V8 `SnapshotCreator` fatal, printed as:
//!
//! ```text
//! Unknown external reference 0x....
//! <unresolved>
//! ```
//!
//! Because this is a C++ `abort()` inside `runtime.snapshot()`
//! (`server/src/engine/mod.rs`, `let snapshot_data = runtime.snapshot();`), it
//! is NOT caught by the surrounding `catch_unwind`: the process dies, so every
//! other session on the server dies with it. A small clone snapshots fine, so
//! the trigger is the size/shape of the live heap reachable at snapshot time,
//! not the clone itself.
//!
//! Expected (what this test asserts): a session that overruns snapshotting must
//! surface a per-execution error (e.g. an OOM or "snapshot failed" result) and
//! leave the server process and OTHER sessions alive — never take the process
//! down. Confirmed workaround today: run with heap persistence off
//! (`--heap-store none`) and filesystem snapshots on, which turns the crash
//! into a graceful `Out of memory: V8 heap limit exceeded` result.
//!
//! This test is red on `main` today: the server process exits during the clone.
//!
//! It needs network access (imports `isomorphic-git` from esm.sh and clones a
//! public GitHub repo) and takes ~1 minute, so it is gated behind an env var.
//! Run it with:
//!
//! ```bash
//! RUN_SNAPSHOT_CRASH_REPRO=1 cargo test --test heap_snapshot_large_session_crash -- --nocapture
//! ```
//!
//! Override the repo with `SNAPSHOT_CRASH_REPO_URL` (default: the confirmed
//! trigger `https://github.com/trycua/cua`).

use reqwest::Client;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};

fn find_available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A temp dir that cleans itself up, and a helper to write policy files into it.
struct Workspace {
    dir: std::path::PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mcp-snapshot-crash-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Workspace { dir }
    }

    fn write(&self, name: &str, contents: &str) -> std::path::PathBuf {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Policies that let the sandbox import isomorphic-git from esm.sh, fetch from
/// GitHub, and read/write its session filesystem — the pi-irc clone setup.
fn write_policies(ws: &Workspace) -> std::path::PathBuf {
    let fetch = ws.write(
        "policies/fetch.rego",
        "package mcp.fetch\ndefault allow = true\n",
    );
    let modules = ws.write(
        "policies/modules.rego",
        "package mcp.modules\ndefault allow = true\n",
    );
    let fs = ws.write(
        "policies/filesystem.rego",
        "package mcp.filesystem\ndefault allow = true\n",
    );
    let config = json!({
        "policies": {
            "fetch": { "policies": [{ "url": format!("file://{}", fetch.display()) }] },
            "modules": { "policies": [{ "url": format!("file://{}", modules.display()) }] },
            "filesystem": { "policies": [{ "url": format!("file://{}", fs.display()) }] },
        }
    });
    ws.write(
        "config.json",
        &serde_json::to_string_pretty(&config).unwrap(),
    )
}

async fn spawn_server(port: u16, ws: &Workspace, config: &std::path::Path) -> Child {
    let child = Command::new(env!("CARGO_BIN_EXE_server"))
        .args([
            "--http-port",
            &port.to_string(),
            "--heap-store",
            "dir",
            "--heap-dir",
            ws.path("heaps").to_str().unwrap(),
            "--fs-store",
            "dir",
            "--session-db-path",
            ws.path("sessions").to_str().unwrap(),
            "--heap-memory-max",
            "2048",
            "--execution-timeout",
            "300",
            "--allow-external-modules",
            "--config",
            config.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn server");

    let client = Client::new();
    let health = format!("http://127.0.0.1:{port}/api/executions");
    for _ in 0..150 {
        if client
            .get(&health)
            .timeout(Duration::from_millis(200))
            .send()
            .await
            .is_ok()
        {
            return child;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not become ready");
}

/// Submit code for a session; return the execution id.
async fn submit(client: &Client, base: &str, session: &str, code: &str) -> String {
    let resp = client
        .post(format!("{base}/api/exec"))
        .json(&json!({ "code": code, "session": session }))
        .send()
        .await
        .expect("POST /api/exec");
    let body: Value = resp.json().await.expect("exec json");
    body["execution_id"]
        .as_str()
        .expect("execution_id")
        .to_string()
}

/// True while the server answers HTTP (i.e. the process is alive).
async fn server_alive(client: &Client, base: &str) -> bool {
    client
        .get(format!("{base}/api/executions"))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .is_ok()
}

#[tokio::test]
async fn large_clone_with_heap_persistence_must_not_crash_the_process() {
    if std::env::var("RUN_SNAPSHOT_CRASH_REPRO").is_err() {
        eprintln!(
            "skipping: set RUN_SNAPSHOT_CRASH_REPRO=1 to run (network + ~1 min). \
             See the module docs."
        );
        return;
    }

    let repo = std::env::var("SNAPSHOT_CRASH_REPO_URL")
        .unwrap_or_else(|_| "https://github.com/trycua/cua".to_string());

    let ws = Workspace::new();
    let config = write_policies(&ws);
    let port = find_available_port();
    let base = format!("http://127.0.0.1:{port}");
    let mut server = spawn_server(port, &ws, &config).await;
    let client = Client::new();

    // A second, tiny session proves the blast radius: it exists before the
    // crash and must still be serviceable afterwards.
    let bystander = submit(
        &client,
        &base,
        "bystander",
        "globalThis.n = (globalThis.n ?? 0) + 1; console.log('bystander', globalThis.n);",
    )
    .await;

    // The trigger: clone a large repo into the session filesystem, holding the
    // isomorphic-git http machinery live, then let the run end so the engine
    // serializes the heap snapshot.
    let clone_code = format!(
        r#"
        const git = (await import("https://esm.sh/isomorphic-git@1.27.1")).default;
        const http = (await import("https://esm.sh/isomorphic-git@1.27.1/http/web/index.js")).default;
        await git.clone({{ fs, http, dir: "/repo", url: {repo}, depth: 1, singleBranch: true }});
        console.log("cloned", (await fs.readdir("/repo")).length, "entries");
        "#,
        repo = serde_json::to_string(&repo).unwrap()
    );
    let clone_id = submit(&client, &base, "cloner", &clone_code).await;

    // Poll the clone execution to a terminal state. If the process crashes, the
    // HTTP endpoint stops answering — detect that and fail with the diagnosis.
    let mut terminal: Option<Value> = None;
    for _ in 0..1200 {
        sleep(Duration::from_millis(250)).await;
        let resp = match client
            .get(format!("{base}/api/executions/{clone_id}"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(_) => {
                // The server stopped answering mid-clone. Confirm the process
                // is actually gone, then fail with the reproduction verdict.
                sleep(Duration::from_millis(500)).await;
                let still_up = server_alive(&client, &base).await;
                let exited = server.try_wait().ok().flatten();
                assert!(
                    still_up,
                    "REPRODUCED: the server process died while snapshotting the large \
                     session (clone of {repo}). Heap persistence (--heap-store dir) makes \
                     runtime.snapshot() abort the whole process on a large heap \
                     (V8 SnapshotCreator: 'Unknown external reference / <unresolved>'), \
                     taking every session down. Child exit: {exited:?}. Expected a \
                     per-execution error with the process staying alive (as \
                     --heap-store none does, returning a graceful OOM)."
                );
                unreachable!("server_alive returned false above");
            }
        };
        let body: Value = resp.json().await.expect("execution json");
        match body["status"].as_str().unwrap_or("") {
            "running" | "queued" => continue,
            _ => {
                terminal = Some(body);
                break;
            }
        }
    }

    let clone_result = terminal.expect("clone execution never reached a terminal state");
    let status = clone_result["status"].as_str().unwrap_or("");

    // The server must still be alive and the bystander session still usable.
    assert!(
        server_alive(&client, &base).await,
        "server must stay alive after the large clone; clone status was {status:?}, \
         result {clone_result}"
    );

    // The clone execution must be a clean terminal state: either it completed,
    // or it failed gracefully (e.g. OOM). What must never happen is a process
    // crash, which the branch above already asserts against.
    assert!(
        matches!(status, "completed" | "failed" | "timed_out" | "cancelled"),
        "unexpected clone status {status:?}: {clone_result}"
    );

    // The bystander session, created before the clone, must still resolve and
    // its next run must still work — proving the crash did not wipe sessions.
    let _ = bystander;
    let bystander2 = submit(
        &client,
        &base,
        "bystander",
        "globalThis.n = (globalThis.n ?? 0) + 1; console.log('bystander', globalThis.n);",
    )
    .await;
    let mut ok = false;
    for _ in 0..120 {
        sleep(Duration::from_millis(250)).await;
        let body: Value = client
            .get(format!("{base}/api/executions/{bystander2}"))
            .send()
            .await
            .expect("bystander poll")
            .json()
            .await
            .expect("bystander json");
        if body["status"].as_str().unwrap_or("") != "running"
            && body["status"].as_str().unwrap_or("") != "queued"
        {
            ok = body["status"] == "completed";
            break;
        }
    }
    assert!(
        ok,
        "bystander session should still run after the large clone"
    );

    let _ = server.kill().await;
}
