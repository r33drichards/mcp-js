/// Tests for the operator-configured pre-run scripts:
///
/// - `--init-script`: runs before an execution whenever the isolate lacks the
///   `__mcpV8InitDone` marker (fresh isolates, and restored heaps that never
///   ran it). On success the marker is set and baked into the heap snapshot,
///   so each stateful heap lineage runs it once. In stateless mode every
///   isolate is fresh, so it runs before every execution.
/// - `--pre-run-script`: runs before every execution, right before user code.
///
/// Ordering is: marker-gated init script → pre-run script → user code.
///
/// Since all code runs as ES modules (no expression return values), tests use
/// console.log() to capture output via sled and assert on the captured content.
use std::sync::{Arc, Once};

use server::engine::{Engine, ExecutionConfig, execute_stateful, execute_stateless, unwrap_snapshot};
use server::engine::execution::ExecutionRegistry;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

const HEAP_BYTES: usize = 32 * 1024 * 1024;

fn rand_id() -> u64 {
    use std::time::SystemTime;
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as u64
}

/// Create a temp sled tree for console capture.
fn console_tree() -> (sled::Tree, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-pre-run-test-{}-{}",
        std::process::id(),
        rand_id()
    ));
    let db = sled::open(&tmp).expect("Failed to open sled db");
    let tree = db.open_tree("console").expect("Failed to open tree");
    (tree, tmp)
}

/// Read all console output from a sled tree.
fn read_console(tree: &sled::Tree) -> String {
    let mut buf = Vec::new();
    for entry in tree.iter().flatten() {
        buf.extend_from_slice(&entry.1);
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// Run stateful with the given scripts and (wrapped) input snapshot; return
/// (console output, wrapped output snapshot) or the execution error.
fn run_stateful(
    code: &str,
    wrapped_snapshot: Option<&[u8]>,
    init_script: Option<&str>,
    pre_run_script: Option<&str>,
) -> Result<(String, Vec<u8>), String> {
    let raw = match wrapped_snapshot {
        Some(w) => Some(unwrap_snapshot(w).expect("snapshot must unwrap")),
        None => None,
    };
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(HEAP_BYTES)
        .console_tree(tree.clone())
        .maybe_init_script(init_script)
        .maybe_pre_run_script(pre_run_script);
    let (result, _oom) = execute_stateful(code, raw, config);
    let output = read_console(&tree);
    let _ = std::fs::remove_dir_all(&tmp);
    match result {
        Ok((_out, wrapped, _hash)) => Ok((output, wrapped)),
        Err(e) => Err(e),
    }
}

// ── Stateless mode ──────────────────────────────────────────────────────

/// In stateless mode every isolate is fresh, so both scripts run on every
/// execution, in order: init → pre-run → user code.
#[test]
fn stateless_scripts_run_every_execution_in_order() {
    ensure_v8();
    for _ in 0..2 {
        let (tree, tmp) = console_tree();
        let config = ExecutionConfig::new(HEAP_BYTES)
            .console_tree(tree.clone())
            .maybe_init_script(Some("globalThis.base = 42; console.log('INIT')"))
            .maybe_pre_run_script(Some("console.log('PRE')"));
        let (result, _oom) = execute_stateless("console.log('USER base=' + globalThis.base)", config);
        assert!(result.is_ok(), "got: {:?}", result);
        let output = read_console(&tree);
        let _ = std::fs::remove_dir_all(&tmp);
        let (init_at, pre_at, user_at) = (
            output.find("INIT").expect("init script should run"),
            output.find("PRE").expect("pre-run script should run"),
            output.find("USER base=42").expect("user code should see init globals"),
        );
        assert!(init_at < pre_at && pre_at < user_at, "wrong order: {}", output);
    }
}

/// Module bindings of a script must not leak into the user code's scope
/// (only explicit `globalThis` assignments are shared).
#[test]
fn stateless_script_bindings_do_not_leak() {
    ensure_v8();
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(HEAP_BYTES)
        .console_tree(tree.clone())
        .maybe_init_script(Some("const secret = 1; globalThis.shared = 2"));
    let (result, _oom) = execute_stateless(
        "console.log('secret=' + typeof secret + ' shared=' + globalThis.shared)",
        config,
    );
    assert!(result.is_ok(), "got: {:?}", result);
    let output = read_console(&tree);
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(output.contains("secret=undefined shared=2"), "got: {}", output);
}

/// Scripts run as ES modules, so `import` of embedded node: modules works.
#[test]
fn init_script_can_import_node_modules() {
    ensure_v8();
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(HEAP_BYTES)
        .console_tree(tree.clone())
        .maybe_init_script(Some(
            "import path from 'node:path';\nglobalThis.joined = path.join('a', 'b');",
        ));
    let (result, _oom) = execute_stateless("console.log('joined=' + globalThis.joined)", config);
    assert!(result.is_ok(), "got: {:?}", result);
    let output = read_console(&tree);
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(output.contains("joined=a/b"), "got: {}", output);
}

// ── Stateful mode: marker-gated init across a heap lineage ──────────────

/// The init script runs on the first (fresh) execution and is skipped on the
/// snapshot-restored follow-up; the pre-run script runs both times; init
/// globals and the marker survive in the heap. The marker is hidden from
/// enumeration.
#[test]
fn stateful_init_runs_once_per_lineage() {
    ensure_v8();
    let (out1, snap1) = run_stateful(
        "console.log('USER1')",
        None,
        Some("globalThis.base = 42; console.log('INIT')"),
        Some("console.log('PRE')"),
    )
    .expect("run 1 should succeed");
    assert!(out1.contains("INIT") && out1.contains("PRE") && out1.contains("USER1"), "got: {}", out1);

    let (out2, _snap2) = run_stateful(
        r#"
        console.log('USER2 base=' + globalThis.base + ' marker=' + globalThis.__mcpV8InitDone);
        console.log('enumerated=' + Object.keys(globalThis).includes('__mcpV8InitDone'));
        "#,
        Some(&snap1),
        Some("globalThis.base = 42; console.log('INIT')"),
        Some("console.log('PRE')"),
    )
    .expect("run 2 should succeed");
    assert!(!out2.contains("INIT"), "init must not re-run on a marked heap: {}", out2);
    assert!(out2.contains("PRE"), "pre-run must run on restored heaps: {}", out2);
    assert!(out2.contains("USER2 base=42 marker=true"), "got: {}", out2);
    assert!(out2.contains("enumerated=false"), "marker should be DONT_ENUM: {}", out2);
}

/// A heap created without the init script (e.g. before the flag existed) gets
/// initialized on its next run under a flag-carrying server, exactly once.
#[test]
fn pre_existing_heap_gets_init_on_restore() {
    ensure_v8();
    // Run A: no scripts configured → marker-less heap.
    let (out_a, snap_a) = run_stateful("console.log('USERA')", None, None, None)
        .expect("run A should succeed");
    assert!(!out_a.contains("INIT"), "got: {}", out_a);

    // Run B: restore the marker-less heap with an init script → it runs.
    let (out_b, snap_b) = run_stateful(
        "console.log('USERB v=' + globalThis.v)",
        Some(&snap_a),
        Some("globalThis.v = 7; console.log('INIT')"),
        None,
    )
    .expect("run B should succeed");
    assert!(out_b.contains("INIT") && out_b.contains("USERB v=7"), "got: {}", out_b);

    // Run C: the lineage is now marked → init is skipped.
    let (out_c, _snap_c) = run_stateful(
        "console.log('USERC v=' + globalThis.v)",
        Some(&snap_b),
        Some("globalThis.v = 7; console.log('INIT')"),
        None,
    )
    .expect("run C should succeed");
    assert!(!out_c.contains("INIT"), "got: {}", out_c);
    assert!(out_c.contains("USERC v=7"), "init globals persist in the heap: {}", out_c);
}

/// The marker is only set after a successful init run, so a failing init
/// fails the execution, persists nothing, and retries on the next run.
#[test]
fn failed_init_does_not_persist_marker() {
    ensure_v8();
    // Fresh run with a throwing init → prefixed error, no snapshot produced.
    let err = run_stateful("console.log('USER')", None, Some("throw new Error('boom')"), None)
        .expect_err("throwing init should fail the execution");
    assert!(err.contains("init script failed"), "got: {}", err);
    assert!(err.contains("boom"), "got: {}", err);

    // Build a marker-less lineage, fail init on it once, then succeed.
    let (_out, snap) = run_stateful("console.log('SEED')", None, None, None)
        .expect("seed run should succeed");
    let err = run_stateful("console.log('USER')", Some(&snap), Some("throw new Error('boom')"), None)
        .expect_err("throwing init should fail on the restored heap too");
    assert!(err.contains("init script failed"), "got: {}", err);

    let (out, _snap2) = run_stateful(
        "console.log('USER ok=' + globalThis.ok)",
        Some(&snap),
        Some("globalThis.ok = 1; console.log('INIT')"),
        None,
    )
    .expect("init should retry after a failure");
    assert!(out.contains("INIT") && out.contains("USER ok=1"), "got: {}", out);
}

/// Deleting the marker from user code forces a re-init on the next run.
#[test]
fn deleting_marker_forces_reinit() {
    ensure_v8();
    let init = Some("globalThis.n = (globalThis.n ?? 0) + 1; console.log('INIT')");
    let (out1, snap1) = run_stateful("console.log('n=' + globalThis.n)", None, init, None)
        .expect("run 1 should succeed");
    assert!(out1.contains("INIT") && out1.contains("n=1"), "got: {}", out1);

    let (out2, snap2) = run_stateful(
        "delete globalThis.__mcpV8InitDone; console.log('n=' + globalThis.n)",
        Some(&snap1),
        init,
        None,
    )
    .expect("run 2 should succeed");
    assert!(!out2.contains("INIT") && out2.contains("n=1"), "got: {}", out2);

    let (out3, _snap3) = run_stateful("console.log('n=' + globalThis.n)", Some(&snap2), init, None)
        .expect("run 3 should succeed");
    assert!(out3.contains("INIT") && out3.contains("n=2"), "marker deleted → re-init: {}", out3);
}

/// A failing pre-run script fails the execution with a prefixed error, on
/// fresh and restored isolates alike.
#[test]
fn failed_pre_run_script_fails_execution() {
    ensure_v8();
    let err = run_stateful("console.log('USER')", None, None, Some("throw new Error('nope')"))
        .expect_err("throwing pre-run should fail the execution");
    assert!(err.contains("pre-run script failed"), "got: {}", err);
    assert!(err.contains("nope"), "got: {}", err);

    let (_out, snap) = run_stateful("console.log('SEED')", None, None, None)
        .expect("seed run should succeed");
    let err = run_stateful("console.log('USER')", Some(&snap), None, Some("throw new Error('nope')"))
        .expect_err("throwing pre-run should fail on restored heaps too");
    assert!(err.contains("pre-run script failed"), "got: {}", err);
}

// ── Engine-level (production async path) ────────────────────────────────

fn make_registry() -> Arc<ExecutionRegistry> {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-pre-run-reg-{}-{}",
        std::process::id(),
        rand_id()
    ));
    Arc::new(ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry"))
}

/// Submit code and wait for the result (blocking poll).
async fn run_and_wait(engine: &Engine, code: &str) -> Result<String, String> {
    let exec_id = engine.run_js(code).execute().await?;
    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return info.result.ok_or_else(|| "No result".to_string()),
                "failed" => return Err(info.error.unwrap_or_else(|| "Unknown error".to_string())),
                "timed_out" => return Err("Timed out".to_string()),
                "cancelled" => return Err("Cancelled".to_string()),
                _ => continue,
            }
        }
    }
    Err("Execution did not complete within timeout".to_string())
}

/// The Engine builders thread both scripts into executions.
#[tokio::test]
async fn engine_threads_scripts_into_executions() {
    ensure_v8();
    let engine = Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_execution_registry(make_registry())
        .with_init_script("globalThis.base = 42".to_string())
        .with_pre_run_script("globalThis.pre = true".to_string());
    let result = run_and_wait(
        &engine,
        "if (globalThis.base !== 42) throw new Error('no init'); \
         if (globalThis.pre !== true) throw new Error('no pre-run');",
    )
    .await;
    assert!(result.is_ok(), "got: {:?}", result);

    let result = run_and_wait(&engine, "if (globalThis.base !== 42) throw new Error('no init')").await;
    assert!(result.is_ok(), "stateless init runs on every execution, got: {:?}", result);
}

/// Engine-level error surface: a failing init script fails the execution.
#[tokio::test]
async fn engine_surfaces_init_script_errors() {
    ensure_v8();
    let engine = Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_execution_registry(make_registry())
        .with_init_script("throw new Error('boom')".to_string());
    let err = run_and_wait(&engine, "console.log('unreachable')")
        .await
        .expect_err("execution should fail");
    assert!(err.contains("init script failed"), "got: {}", err);
}
