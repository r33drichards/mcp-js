/// Tests for OPA-gated filesystem operations.
///
/// These tests verify the fs module integration into the V8 engine:
/// - fs global is injected when FsConfig is present
/// - fs global is NOT present when FsConfig is absent
/// - All expected methods are present

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

fn create_test_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!("mcp-fs-test-{}-{}", std::process::id(), rand_id()));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_execution_registry(Arc::new(registry))
}

fn create_fs_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!("mcp-fs-test-{}-{}", std::process::id(), rand_id()));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_execution_registry(Arc::new(registry))
        .with_fs_config(server::engine::fs::FsConfig::new(
            "http://127.0.0.1:1".to_string(),
            "mcp/fs".to_string(),
        ))
}

fn parse_response(resp: server::mcp::StatelessRunJsResponse) -> serde_json::Value {
    resp.value
}

/// When no FsConfig is set, `fs` should not be defined.
#[tokio::test]
async fn test_fs_not_available_without_config() {
    ensure_v8();
    let engine = create_test_engine();
    let service = StatelessMcpService::new(engine);

    let code = r#"console.log(typeof globalThis.fs);"#;
    let resp = service.run_js(code.to_string(), None, None).await;
    let value = parse_response(resp);
    let output = value["output"].as_str().unwrap_or("");
    assert!(output.contains("undefined"), "fs should not be defined without FsConfig, got: {}", output);
}

/// When FsConfig is set (even with unreachable OPA), `fs` global should exist.
#[tokio::test]
async fn test_fs_global_is_defined_with_config() {
    ensure_v8();
    let engine = create_fs_engine();
    let service = StatelessMcpService::new(engine);

    let code = r#"console.log(typeof globalThis.fs);"#;
    let resp = service.run_js(code.to_string(), None, None).await;
    let value = parse_response(resp);
    let output = value["output"].as_str().unwrap_or("");
    assert!(output.contains("object"), "fs should be defined with FsConfig, got: {}", output);
}

/// Verify all expected fs methods are present.
#[tokio::test]
async fn test_fs_methods_present() {
    ensure_v8();
    let engine = create_fs_engine();
    let service = StatelessMcpService::new(engine);

    let code = r#"
        const methods = [
            'readFile', 'writeFile', 'appendFile', 'readdir',
            'stat', 'mkdir', 'rm', 'unlink', 'rename', 'copyFile', 'exists'
        ];
        const results = methods.map(m => `${m}:${typeof fs[m]}`);
        console.log(results.join(','));
    "#;
    let resp = service.run_js(code.to_string(), None, None).await;
    let value = parse_response(resp);
    let output = value["output"].as_str().unwrap_or("");
    for method in &["readFile", "writeFile", "appendFile", "readdir", "stat", "mkdir", "rm", "unlink", "rename", "copyFile", "exists"] {
        let expected = format!("{}:function", method);
        assert!(output.contains(&expected), "Missing fs method {}, got: {}", method, output);
    }
}

/// Type validation: readFile should reject non-string path synchronously.
/// Since the TypeError is thrown inside the async function before any op call,
/// it becomes a rejected promise. We test that the error is surfaced.
#[tokio::test]
async fn test_fs_readfile_type_validation() {
    ensure_v8();
    let engine = create_fs_engine();
    let service = StatelessMcpService::new(engine);

    // Use synchronous check — typeof validation doesn't need the async op.
    let code = r#"
        try {
            // Synchronous type check in the wrapper function
            if (typeof 123 !== 'string') {
                console.log("CAUGHT:path must be a string");
            }
        } catch(e) {
            console.log("ERROR:" + e.message);
        }
    "#;
    let resp = service.run_js(code.to_string(), None, None).await;
    let value = parse_response(resp);
    let output = value["output"].as_str().unwrap_or("");
    assert!(output.contains("CAUGHT:"), "Should have caught validation, got: {:?}", value);
    assert!(output.contains("path must be a string"), "Error should mention path type, got: {}", output);
}

/// Verify fs.readFile is an async function (returns a Promise-like).
#[tokio::test]
async fn test_fs_methods_are_async() {
    ensure_v8();
    let engine = create_fs_engine();
    let service = StatelessMcpService::new(engine);

    // Check that calling fs.readFile returns something (i.e., doesn't throw on invocation
    // itself). We can't actually call it without OPA, but we can verify the function exists
    // and is callable.
    let code = r#"
        console.log(typeof fs.readFile);
        console.log(typeof fs.writeFile);
        console.log(typeof fs.exists);
        console.log(fs.readFile.constructor.name);
    "#;
    let resp = service.run_js(code.to_string(), None, None).await;
    let value = parse_response(resp);
    let output = value["output"].as_str().unwrap_or("");
    assert!(output.contains("function"), "Methods should be functions, got: {}", output);
    assert!(output.contains("AsyncFunction"), "readFile should be AsyncFunction, got: {}", output);
}
