/// Tests for stateless shell mode — verifies that `StatelessMcpService::run_js`
/// executes code and returns console output directly (no execution IDs exposed).

use std::sync::{Arc, Once};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use server::engine::{initialize_v8, Engine};
use server::engine::execution::ExecutionRegistry;
use server::mcp::{StatelessMcpService, StatelessRunJsArgs};

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

/// Helper: invoke `run_js` with code only, parse the JSON text content of the
/// returned `CallToolResult`.
async fn run(service: &StatelessMcpService, code: &str) -> serde_json::Value {
    let args = StatelessRunJsArgs {
        code: code.to_string(),
        heap_memory_max_mb: None,
        execution_timeout_secs: None,
    };
    let result: CallToolResult = service
        .run_js(Parameters(args))
        .await
        .expect("run_js returned McpError");
    let raw_content = result
        .content
        .first()
        .expect("CallToolResult has no content");
    let json = serde_json::to_value(raw_content).expect("content not serializable");
    let text = json
        .get("text")
        .and_then(|v| v.as_str())
        .expect("first content block is not text");
    serde_json::from_str(text).unwrap_or_else(|e| panic!("response text is not JSON: {}: {}", e, text))
}

#[tokio::test]
async fn test_stateless_shell_console_log() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let value = run(&service, "console.log('hello world')").await;

    assert!(value["error"].is_null(), "Should not have error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output field");
    assert!(output.contains("hello world"), "Output should contain 'hello world', got: {}", output);
}

#[tokio::test]
async fn test_stateless_shell_multiple_console_logs() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let code = r#"
        console.log("line 1");
        console.log("line 2");
        console.log("line 3");
    "#;

    let value = run(&service, code).await;

    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("line 1"), "Should contain line 1");
    assert!(output.contains("line 2"), "Should contain line 2");
    assert!(output.contains("line 3"), "Should contain line 3");
}

#[tokio::test]
async fn test_stateless_shell_error_handling() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let value = run(&service, "throw new Error('boom')").await;

    assert!(!value["error"].is_null(), "Should have error field: {:?}", value);
    let error = value["error"].as_str().unwrap_or("");
    assert!(error.contains("boom"), "Error should mention 'boom', got: {}", error);
}

#[tokio::test]
async fn test_stateless_shell_no_execution_id_exposed() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let value = run(&service, "console.log('test')").await;

    assert!(value["execution_id"].is_null(), "Should not expose execution_id: {:?}", value);
}

#[tokio::test]
async fn test_stateless_shell_computation_with_output() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let code = r#"
        const sum = [1, 2, 3, 4, 5].reduce((a, b) => a + b, 0);
        console.log("sum is", sum);
    "#;

    let value = run(&service, code).await;

    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("sum is 15"), "Output should contain 'sum is 15', got: {}", output);
}

#[tokio::test]
async fn test_stateless_shell_top_level_await() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let code = r#"
        const result = await Promise.resolve(42);
        console.log("result is", result);
    "#;

    let value = run(&service, code).await;

    assert!(value["error"].is_null(), "Top-level await should not error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("result is 42"), "Output should contain 'result is 42', got: {}", output);
}

#[tokio::test]
async fn test_stateless_shell_top_level_await_async_chain() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine, None);

    let code = r#"
        const a = await Promise.resolve(10);
        const b = await Promise.resolve(20);
        console.log(a + b);
    "#;

    let value = run(&service, code).await;

    assert!(value["error"].is_null(), "Chained top-level await should not error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("30"), "Output should contain '30', got: {}", output);
}
