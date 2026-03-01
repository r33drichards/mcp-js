/// Tests for stateless shell mode — verifies that `StatelessMcpService::run_js`
/// executes code and returns console output directly (no execution IDs exposed).

use std::sync::{Arc, Once};
use server::engine::{initialize_v8, Engine};
use server::engine::execution::ExecutionRegistry;
use server::mcp::StatelessMcpService;

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

use server::mcp::StatelessRunJsResponse;

/// Extract the JSON value from a StatelessRunJsResponse.
fn parse_response(resp: StatelessRunJsResponse) -> serde_json::Value {
    resp.value
}

#[tokio::test]
async fn test_stateless_shell_console_log() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine);

    let resp = service.run_js("console.log('hello world')".to_string(), None, None).await;
    let value = parse_response(resp);

    assert!(value["error"].is_null(), "Should not have error: {:?}", value);
    let output = value["output"].as_str().expect("Should have output field");
    assert!(output.contains("hello world"), "Output should contain 'hello world', got: {}", output);
}

#[tokio::test]
async fn test_stateless_shell_multiple_console_logs() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine);

    let code = r#"
        console.log("line 1");
        console.log("line 2");
        console.log("line 3");
    "#;

    let resp = service.run_js(code.to_string(), None, None).await;
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
    let service = StatelessMcpService::new(engine);

    let resp = service.run_js("throw new Error('boom')".to_string(), None, None).await;
    let value = parse_response(resp);

    assert!(!value["error"].is_null(), "Should have error field: {:?}", value);
    let error = value["error"].as_str().unwrap_or("");
    assert!(error.contains("boom"), "Error should mention 'boom', got: {}", error);
}

#[tokio::test]
async fn test_stateless_shell_no_execution_id_exposed() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine);

    let resp = service.run_js("console.log('test')".to_string(), None, None).await;
    let value = parse_response(resp);

    assert!(value["execution_id"].is_null(), "Should not expose execution_id: {:?}", value);
}

#[tokio::test]
async fn test_stateless_shell_computation_with_output() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine);

    let code = r#"
        const sum = [1, 2, 3, 4, 5].reduce((a, b) => a + b, 0);
        console.log("sum is", sum);
    "#;

    let resp = service.run_js(code.to_string(), None, None).await;
    let value = parse_response(resp);

    let output = value["output"].as_str().expect("Should have output");
    assert!(output.contains("sum is 15"), "Output should contain 'sum is 15', got: {}", output);
}
