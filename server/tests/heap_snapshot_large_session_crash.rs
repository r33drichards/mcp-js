//! Reproduction: with heap persistence on, an execution that runs out of heap
//! after it has suspended at an `await` crashes the whole server process,
//! killing every session.
//!
//! Originally reported from the pi-irc deployment as "a large isomorphic-git
//! clone crashes the engine at snapshot time" (`--heap-store dir` plus
//! `--fs-store dir`). Bisecting that report against a stateful server showed
//! the clone, the filesystem store, the network, and the data size are all
//! incidental; the trigger is:
//!
//!   heap persistence ON  +  the run hits the V8 heap limit  +  the OOM happens
//!   after the module's top level has suspended at an `await` (a resolved
//!   promise is enough; a timer or a completed `fetch` behave the same).
//!
//! Observed matrix (all on `main` at 4b7daaff, 64 MiB heap cap):
//!
//! | run                                            | `--heap-store none` | `--heap-store dir`          |
//! |------------------------------------------------|---------------------|-----------------------------|
//! | OOM at top level, no `await`                   | graceful OOM error  | graceful OOM error          |
//! | `await Promise.resolve()` then OOM             | graceful OOM error  | **process abort**           |
//! | `await` timer then OOM                         | graceful OOM error  | **process abort**           |
//! | 4 KB `fetch` then OOM                          | graceful OOM error  | **process abort**           |
//! | 60 MiB `fetch` body (OOMs decoding base64)     | graceful OOM error  | **process abort**           |
//! | 56 MiB `fetch` body (fits)                     | ok                  | ok                          |
//! | large clone of trycua/cua (OOMs at 2 GiB cap)  | graceful OOM error  | **process abort**           |
//!
//! Crash signature, printed by V8 right before the process exits (wait status
//! 5 / `Trace/BPT trap`):
//!
//! ```text
//! Unknown external reference 0x....
//! <unresolved>
//! ```
//!
//! Why (see `server/src/engine/mod.rs`): `near_heap_limit_callback` sets the
//! OOM flag, calls `isolate.terminate_execution()`, and doubles the limit so
//! V8 can unwind. `execute_stateful` then calls `runtime.snapshot()`
//! **unconditionally**, before it inspects `output_result` (which is `Err` and
//! makes it throw the snapshot away). When the termination interrupted the
//! event loop (anything after the first `await`), deno_core's op-driver /
//! promise-reaction state is still reachable from the heap, and V8's
//! `SnapshotCreator` aborts on an external pointer that is not in the
//! registered external-reference table. The abort is a C++ `abort()`, so the
//! surrounding `catch_unwind` cannot catch it and every session dies. A
//! top-level OOM leaves no such state behind, which is why the sync case is
//! fine.
//!
//! What this file asserts: the process must stay alive and other sessions must
//! keep working; the OOM must come back as a per-execution error, exactly as it
//! does with heap persistence off. The async case is red on `main` today; the
//! sync case is included as a green control so the contrast is explicit.
//!
//! Cost: the offline tests need no network, no git, no policies; they spawn the
//! built server binary with a 64 MiB heap cap and finish in a few seconds
//! using well under 200 MB of RAM and a few MB of disk.
//!
//! The two crashing cases are `#[ignore]`d so `cargo test` (the required CI
//! check) stays green until the engine is fixed; the green control always runs.
//! Run the reproduction for real with:
//!
//! ```bash
//! cargo test --test heap_snapshot_large_session_crash -- --ignored --nocapture
//! ```
//!
//! Remove the `#[ignore]`s when the fix lands so they guard the behaviour.
//!
//! Why not fix it here: skipping `runtime.snapshot()` on a failed run is not
//! enough — rusty_v8's `OwnedIsolate::drop` asserts that a snapshot-creator
//! isolate produced a blob (`create_blob`) before being dropped, so the engine
//! must either drain deno_core's pending op/promise state before snapshotting
//! a terminated isolate, or stop running stateful executions in a
//! snapshot-creator isolate that it cannot abandon.
//!
//! The original large-clone reproduction is kept as an opt-in variant (network,
//! ~1 minute, ~1 GB of RAM, ~150 MB of disk):
//!
//! ```bash
//! RUN_SNAPSHOT_CRASH_REPRO_REMOTE=1 cargo test --test heap_snapshot_large_session_crash -- --nocapture
//! ```

use reqwest::Client;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};

/// Allocate until the V8 heap limit trips. With a 64 MiB cap this takes well
/// under a second.
const OOM_LOOP: &str = r#"
globalThis.keep = [];
for (let i = 0; ; i++) {
    const a = new Array(4096);
    for (let j = 0; j < 4096; j++) a[j] = { i, j, s: "v" + j };
    globalThis.keep.push(a);
}
"#;

const BYSTANDER: &str =
    "globalThis.n = (globalThis.n ?? 0) + 1; console.log('bystander', globalThis.n);";

fn find_available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A temp dir that cleans itself up.
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

struct TestServer {
    child: Child,
    base: String,
}

/// Spawn the built server binary as a child process with heap persistence on,
/// so a crash cannot take the test runner down. `extra` appends flags.
async fn spawn_server(ws: &Workspace, heap_mb: u32, extra: &[&str]) -> TestServer {
    let port = find_available_port();
    let port_string = port.to_string();
    let heap_dir = ws.path("heaps");
    // Every server gets its own session log: tests run in parallel and the
    // default path is shared, which makes same-named sessions collide.
    let sessions = ws.path("sessions");
    let heap_mb = heap_mb.to_string();
    let mut args: Vec<&str> = vec![
        "--http-port",
        &port_string,
        "--heap-store",
        "dir",
        "--heap-dir",
        heap_dir.to_str().unwrap(),
        "--session-db-path",
        sessions.to_str().unwrap(),
        "--heap-memory-max",
        &heap_mb,
        "--execution-timeout",
        "300",
    ];
    args.extend_from_slice(extra);

    let child = Command::new(env!("CARGO_BIN_EXE_server"))
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn server");

    let base = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    for _ in 0..150 {
        if client
            .get(format!("{base}/api/executions"))
            .timeout(Duration::from_millis(200))
            .send()
            .await
            .is_ok()
        {
            return TestServer { child, base };
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

/// Poll an execution to a terminal state. Returns `None` when the server stops
/// answering mid-run, which is the crash this file is about.
async fn wait_terminal(client: &Client, base: &str, id: &str, max_polls: usize) -> Option<Value> {
    for _ in 0..max_polls {
        sleep(Duration::from_millis(250)).await;
        let resp = client
            .get(format!("{base}/api/executions/{id}"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .ok()?;
        let body: Value = resp.json().await.ok()?;
        match body["status"].as_str().unwrap_or("") {
            "running" | "queued" => continue,
            _ => return Some(body),
        }
    }
    panic!("execution {id} never reached a terminal state");
}

/// The shared scenario: a bystander session exists, `trigger` runs in another
/// session and is expected to overrun the heap. Asserts the server survives,
/// the trigger reports a graceful OOM, and the bystander still runs.
async fn assert_oom_is_graceful(server: &mut TestServer, trigger: &str, what: &str) {
    assert_survives(server, trigger, what, false).await;
}

/// Like `assert_oom_is_graceful`, but `allow_success` also accepts a run that
/// completes (for triggers whose OOM depends on the configured heap cap).
async fn assert_survives(server: &mut TestServer, trigger: &str, what: &str, allow_success: bool) {
    let client = Client::new();
    let base = server.base.clone();

    let bystander = submit(&client, &base, "bystander", BYSTANDER).await;
    let first = wait_terminal(&client, &base, &bystander, 200)
        .await
        .expect("bystander should run before the trigger");
    assert_eq!(first["status"], "completed", "bystander warm-up: {first}");

    let id = submit(&client, &base, "trigger", trigger).await;
    let result = match wait_terminal(&client, &base, &id, 1200).await {
        Some(result) => result,
        None => {
            sleep(Duration::from_millis(500)).await;
            let exited = server.child.try_wait().ok().flatten();
            panic!(
                "REPRODUCED ({what}): the server process died instead of reporting the OOM. \
                 With --heap-store dir, execute_stateful calls runtime.snapshot() even though \
                 the run was terminated by the heap-limit callback; V8's SnapshotCreator then \
                 aborts with 'Unknown external reference / <unresolved>' and every session dies. \
                 Child exit: {exited:?}. Expected the same graceful \
                 'Out of memory: V8 heap limit exceeded' result that --heap-store none returns."
            );
        }
    };

    assert!(
        server_alive(&client, &base).await,
        "server must stay alive after the OOM ({what}); result {result}"
    );
    if !(allow_success && result["status"] == "completed") {
        assert_eq!(
            result["status"], "failed",
            "OOM must be a failed execution ({what}): {result}"
        );
        let error = result["error"].as_str().unwrap_or("");
        assert!(
            error.contains("Out of memory"),
            "expected a graceful OOM error ({what}), got {error:?}: {result}"
        );
    }

    // Other sessions must be untouched: the bystander keeps its heap and runs.
    let again = submit(&client, &base, "bystander", BYSTANDER).await;
    let second = wait_terminal(&client, &base, &again, 200)
        .await
        .expect("bystander should still run after the OOM");
    assert_eq!(
        second["status"], "completed",
        "bystander after OOM ({what}): {second}"
    );
    let output: Value = client
        .get(format!("{base}/api/executions/{again}/output"))
        .send()
        .await
        .expect("bystander output")
        .json()
        .await
        .expect("bystander output json");
    assert_eq!(
        output["data"].as_str().unwrap_or("").trim(),
        "bystander 2",
        "bystander heap must persist across the other session's OOM ({what}); output: {output}"
    );
}

/// Control: an OOM in top-level synchronous code is handled gracefully even
/// with heap persistence on. Green on `main`.
#[tokio::test]
async fn sync_oom_with_heap_persistence_is_graceful() {
    let ws = Workspace::new();
    let mut server = spawn_server(&ws, 64, &[]).await;
    assert_oom_is_graceful(&mut server, OOM_LOOP, "sync OOM").await;
    let _ = server.child.kill().await;
}

/// The bug: the same OOM after the module has suspended at an `await` takes the
/// whole process down. Red on `main`. Offline, seconds, no git.
#[tokio::test]
#[ignore = "known engine crash (heap persistence + OOM after await); run with -- --ignored"]
async fn oom_after_await_with_heap_persistence_must_not_crash_the_process() {
    let ws = Workspace::new();
    let mut server = spawn_server(&ws, 64, &[]).await;
    let trigger = format!("await Promise.resolve();\n{OOM_LOOP}");
    assert_oom_is_graceful(&mut server, &trigger, "OOM after await").await;
    let _ = server.child.kill().await;
}

/// Same bug through a timer, the other common way a run suspends. Red on `main`.
#[tokio::test]
#[ignore = "known engine crash (heap persistence + OOM after timer); run with -- --ignored"]
async fn oom_after_timer_with_heap_persistence_must_not_crash_the_process() {
    let ws = Workspace::new();
    let mut server = spawn_server(&ws, 64, &[]).await;
    let trigger = format!("await new Promise((r) => setTimeout(r, 10));\n{OOM_LOOP}");
    assert_oom_is_graceful(&mut server, &trigger, "OOM after timer").await;
    let _ = server.child.kill().await;
}

/// Policies that let the sandbox import isomorphic-git from esm.sh, fetch from
/// GitHub, and use its session filesystem — the pi-irc clone setup.
fn write_open_policies(ws: &Workspace) -> std::path::PathBuf {
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

/// The original report, kept as an opt-in variant: a large isomorphic-git clone
/// into the session filesystem overruns the heap while decoding the response
/// bodies (after many awaits), and the process dies the same way. Needs
/// network and about a minute; gated behind `RUN_SNAPSHOT_CRASH_REPRO_REMOTE=1`.
/// Override the repo with `SNAPSHOT_CRASH_REPO_URL`.
#[tokio::test]
async fn large_remote_clone_with_heap_persistence_must_not_crash_the_process() {
    if std::env::var("RUN_SNAPSHOT_CRASH_REPRO_REMOTE").is_err() {
        eprintln!("skipping: set RUN_SNAPSHOT_CRASH_REPRO_REMOTE=1 to run (network, ~1 min)");
        return;
    }
    let repo = std::env::var("SNAPSHOT_CRASH_REPO_URL")
        .unwrap_or_else(|_| "https://github.com/trycua/cua".to_string());

    let ws = Workspace::new();
    let config = write_open_policies(&ws);
    let extra = [
        "--fs-store",
        "dir",
        "--allow-external-modules",
        "--config",
        config.to_str().unwrap(),
    ];
    let mut server = spawn_server(&ws, 2048, &extra).await;
    let trigger = format!(
        r#"
        const git = (await import("https://esm.sh/isomorphic-git@1.27.1")).default;
        const http = (await import("https://esm.sh/isomorphic-git@1.27.1/http/web/index.js")).default;
        await git.clone({{ fs, http, dir: "/repo", url: {repo}, depth: 1, singleBranch: true }});
        console.log("cloned", (await fs.readdir("/repo")).length, "entries");
        "#,
        repo = serde_json::to_string(&repo).unwrap()
    );
    assert_survives(&mut server, &trigger, "large remote clone", true).await;
    let _ = server.child.kill().await;
}
