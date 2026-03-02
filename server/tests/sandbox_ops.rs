/// Tests that dangerous deno_core built-in ops are neutralized.
///
/// 1. Deno.core.ops.op_panic() should throw a JS exception instead of panicking.
/// 2. Deno.core.print() should not write to stdout (captured or discarded).

use std::sync::Once;
use server::engine::ExecutionConfig;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

#[test]
fn test_op_panic_returns_error_instead_of_crashing() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateless(
        r#"
        try {
            Deno.core.ops.op_panic("user triggered panic");
            "should not reach here";
        } catch (e) {
            "caught: " + e.message;
        }
        "#,
        ExecutionConfig::new(heap_bytes),
    );
    assert!(result.is_ok(), "Should not crash, got: {:?}", result);
    let output = result.unwrap();
    assert!(
        output.contains("caught:") && output.contains("panic"),
        "Should catch panic as JS exception, got: {}",
        output,
    );
}

#[test]
fn test_op_panic_uncaught_is_js_error() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateless(
        r#"Deno.core.ops.op_panic("deliberate panic")"#,
        ExecutionConfig::new(heap_bytes),
    );
    // Should be an Err (JS exception), but the process should NOT crash.
    assert!(result.is_err(), "Uncaught panic should be a JS error");
    let err = result.unwrap_err();
    assert!(
        err.contains("panic"),
        "Error should mention 'panic', got: {}",
        err,
    );
}

#[test]
fn test_print_does_not_crash() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateless(
        r#"
        Deno.core.print("this should not appear on stdout");
        Deno.core.print("stderr test", true);
        "ok";
        "#,
        ExecutionConfig::new(heap_bytes),
    );
    assert!(result.is_ok(), "print should not crash, got: {:?}", result);
    assert_eq!(result.unwrap(), "ok");
}

#[test]
fn test_print_routes_through_console_when_available() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;

    let tmp = std::env::temp_dir().join(format!(
        "mcp-sandbox-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = sled::open(&tmp).expect("Failed to open sled db");
    let tree = db.open_tree("console").expect("Failed to open tree");

    let config = ExecutionConfig::new(heap_bytes).console_tree(tree.clone());
    let (result, _oom) = server::engine::execute_stateless(
        r#"
        Deno.core.print("captured via print");
        "done";
        "#,
        config,
    );
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);

    // Read console output from sled.
    let mut output = Vec::new();
    for entry in tree.iter() {
        if let Ok((_, v)) = entry {
            output.extend_from_slice(&v);
        }
    }
    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains("captured via print"),
        "Console output should contain print output, got: {}",
        output_str,
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_stateful_op_panic_returns_error() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateful(
        r#"
        try {
            Deno.core.ops.op_panic("stateful panic");
            "should not reach";
        } catch (e) {
            "caught: " + e.message;
        }
        "#,
        None,
        ExecutionConfig::new(heap_bytes),
    );
    assert!(result.is_ok(), "Should not crash in stateful mode, got: {:?}", result);
    let (output, _snapshot, _hash) = result.unwrap();
    assert!(
        output.contains("caught:") && output.contains("panic"),
        "Should catch panic in stateful mode, got: {}",
        output,
    );
}

#[test]
fn test_process_survives_after_panic_interception() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;

    // First execution triggers the intercepted panic.
    let (result1, _) = server::engine::execute_stateless(
        r#"Deno.core.ops.op_panic("test")"#,
        ExecutionConfig::new(heap_bytes),
    );
    assert!(result1.is_err());

    // Second execution should work fine -- V8 is not corrupted.
    let (result2, _) = server::engine::execute_stateless(
        "1 + 1",
        ExecutionConfig::new(heap_bytes),
    );
    assert!(result2.is_ok());
    assert_eq!(result2.unwrap(), "2");
}
