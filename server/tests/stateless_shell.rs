/// Tests for stateless shell mode — verifies that `StatelessMcpService::run_js`
/// executes code and returns console output directly (no execution IDs exposed).

use std::sync::{Arc, Once};
use server::engine::{initialize_v8, Engine};
use server::engine::execution::ExecutionRegistry;
use server::engine::run_js_file::RunJsFilePolicy;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn rand_id() -> u64 {
    use std::time::SystemTime;
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as u64
}

/// Create a stateless engine with an execution registry for async tests.
fn create_test_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!("mcp-stateless-shell-test-{}-{}", std::process::id(), rand_id()));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_execution_registry(Arc::new(registry))
}

/// Like `create_test_engine`, but with `run_js` file-path reads allowed for
/// any path (`--allow-run-js-file` equivalent).
fn create_test_engine_allow_file() -> Engine {
    create_test_engine().with_run_js_file_policy(RunJsFilePolicy::AllowAll)
}

/// Run stateless run_js via the shared dispatcher and return its JSON body
/// (`{output, error?}`), matching the old `StatelessMcpService::run_js` shape.
async fn run_js(
    engine: &Engine,
    code: Option<String>,
    file: Option<String>,
    max_mb: Option<usize>,
    timeout: Option<u64>,
) -> serde_json::Value {
    let mut args = serde_json::Map::new();
    if let Some(c) = code { args.insert("code".into(), serde_json::Value::String(c)); }
    if let Some(f) = file { args.insert("file".into(), serde_json::Value::String(f)); }
    if let Some(m) = max_mb { args.insert("heap_memory_max_mb".into(), serde_json::json!(m)); }
    if let Some(t) = timeout { args.insert("execution_timeout_secs".into(), serde_json::json!(t)); }
    server::mcp_dispatch::run_js_blocking(engine, None, &serde_json::Value::Object(args)).await
}

fn parse_response(resp: serde_json::Value) -> serde_json::Value {
    resp
}

#[tokio::test]
async fn test_stateless_shell_console_log() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = run_js(&engine,Some("console.log('hello world')".to_string()), None, None, None).await;
    let value = parse_response(resp);

    assert!(value["error"].is_null(), "Should not have error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output field");
    assert!(output.contains("hello world"), "Output should contain 'hello world', got: {}", output);
}

#[tokio::test]
async fn test_stateless_shell_multiple_console_logs() {
    ensure_v8();
    let engine = create_test_engine();
    let code = r#"
        console.log("line 1");
        console.log("line 2");
        console.log("line 3");
    "#;

    let resp = run_js(&engine, Some(code.to_string()), None, None, None).await;
    let value = parse_response(resp);

    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("line 1"), "Should contain line 1");
    assert!(output.contains("line 2"), "Should contain line 2");
    assert!(output.contains("line 3"), "Should contain line 3");
}

#[tokio::test]
async fn test_stateless_shell_error_handling() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = run_js(&engine,Some("throw new Error('boom')".to_string()), None, None, None).await;
    let value = parse_response(resp);

    assert!(!value["error"].is_null(), "Should have error field: {:?}", value);
    let error = value["error"].as_str().unwrap_or("");
    assert!(error.contains("boom"), "Error should mention 'boom', got: {}", error);
}

#[tokio::test]
async fn test_stateless_shell_no_execution_id_exposed() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = run_js(&engine,Some("console.log('test')".to_string()), None, None, None).await;
    let value = parse_response(resp);

    assert!(value["execution_id"].is_null(), "Should not expose execution_id: {:?}", value);
}

#[tokio::test]
async fn test_stateless_shell_computation_with_output() {
    ensure_v8();
    let engine = create_test_engine();
    let code = r#"
        const sum = [1, 2, 3, 4, 5].reduce((a, b) => a + b, 0);
        console.log("sum is", sum);
    "#;

    let resp = run_js(&engine, Some(code.to_string()), None, None, None).await;
    let value = parse_response(resp);

    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("sum is 15"), "Output should contain 'sum is 15', got: {}", output);
}

#[tokio::test]
async fn test_stateless_shell_top_level_await() {
    ensure_v8();
    let engine = create_test_engine();
    let code = r#"
        const result = await Promise.resolve(42);
        console.log("result is", result);
    "#;

    let resp = run_js(&engine, Some(code.to_string()), None, None, None).await;
    let value = parse_response(resp);

    assert!(value["error"].is_null(), "Top-level await should not error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("result is 42"), "Output should contain 'result is 42', got: {}", output);
}

#[tokio::test]
async fn test_stateless_shell_top_level_await_async_chain() {
    ensure_v8();
    let engine = create_test_engine();
    let code = r#"
        const a = await Promise.resolve(10);
        const b = await Promise.resolve(20);
        console.log(a + b);
    "#;

    let resp = run_js(&engine, Some(code.to_string()), None, None, None).await;
    let value = parse_response(resp);

    assert!(value["error"].is_null(), "Chained top-level await should not error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("30"), "Output should contain '30', got: {}", output);
}

// ── run_js `file` parameter (server-side file-path execution) ────────────

#[tokio::test]
async fn test_run_js_file_allow_all_reads_and_runs() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.js");
    std::fs::write(&script, "console.log('from a file', 6 * 7);").unwrap();

    let engine = create_test_engine_allow_file();

    // code omitted; file provided.
    let resp = run_js(&engine, None, Some(script.to_str().unwrap().to_string()), None, None).await;
    let value = parse_response(resp);

    assert!(value["error"].is_null(), "file read should not error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("from a file 42"), "got: {}", output);
}

#[tokio::test]
async fn test_run_js_file_disabled_by_default_errors() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.js");
    std::fs::write(&script, "console.log('nope');").unwrap();

    // Default engine has no run_js_file policy → file reads are disabled.
    let engine = create_test_engine();

    let resp = run_js(&engine, None, Some(script.to_str().unwrap().to_string()), None, None).await;
    let value = parse_response(resp);

    let error = value["error"].as_str().unwrap_or("");
    assert!(error.contains("disabled"), "expected 'disabled' error, got: {:?}", value);
}

#[tokio::test]
async fn test_run_js_file_and_code_conflict_errors() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.js");
    std::fs::write(&script, "console.log('file');").unwrap();

    let engine = create_test_engine_allow_file();

    // Both code and file supplied → error.
    let resp = run_js(
        &engine,
        Some("console.log('inline')".to_string()),
        Some(script.to_str().unwrap().to_string()),
        None,
        None,
    )
    .await;
    let value = parse_response(resp);

    let error = value["error"].as_str().unwrap_or("");
    assert!(error.contains("either"), "expected 'either ... not both' error, got: {:?}", value);
}
