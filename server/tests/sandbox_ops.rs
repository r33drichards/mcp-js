/// Tests that dangerous deno_core built-in ops are neutralized.
///
/// 1. Deno.core.ops.op_panic() should throw a JS exception instead of panicking.
/// 2. Deno.core.print() should not write to stdout (captured or discarded).
///
/// Since all code runs as ES modules (no expression return values), tests use
/// console.log() to capture output via sled and assert on the captured content.

use std::sync::Once;
use server::engine::ExecutionConfig;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

/// Create a temp sled tree for console capture.
fn console_tree() -> (sled::Tree, std::path::PathBuf) {
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
    (tree, tmp)
}

/// Read all console output from a sled tree.
fn read_console(tree: &sled::Tree) -> String {
    let mut buf = Vec::new();
    for entry in tree.iter() {
        if let Ok((_, v)) = entry {
            buf.extend_from_slice(&v);
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

#[test]
fn test_op_panic_returns_error_instead_of_crashing() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(heap_bytes).console_tree(tree.clone());
    let (result, _oom) = server::engine::execute_stateless(
        r#"
        try {
            Deno.core.ops.op_panic("user triggered panic");
            console.log("should not reach here");
        } catch (e) {
            console.log("caught: " + e.message);
        }
        "#,
        config,
    );
    assert!(result.is_ok(), "Should not crash, got: {:?}", result);
    let output = read_console(&tree);
    assert!(
        output.contains("caught:") && output.contains("panic"),
        "Should catch panic as JS exception, got: {}",
        output,
    );
    let _ = std::fs::remove_dir_all(&tmp);
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
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(heap_bytes).console_tree(tree.clone());
    let (result, _oom) = server::engine::execute_stateless(
        r#"
        Deno.core.print("this should not appear on stdout");
        Deno.core.print("stderr test", true);
        console.log("ok");
        "#,
        config,
    );
    assert!(result.is_ok(), "print should not crash, got: {:?}", result);
    let output = read_console(&tree);
    assert!(
        output.contains("ok"),
        "Console should contain 'ok', got: {}",
        output,
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_print_routes_through_console_when_available() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(heap_bytes).console_tree(tree.clone());
    let (result, _oom) = server::engine::execute_stateless(
        r#"
        Deno.core.print("captured via print");
        console.log("done");
        "#,
        config,
    );
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);

    let output = read_console(&tree);
    assert!(
        output.contains("captured via print"),
        "Console output should contain print output, got: {}",
        output,
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_stateful_op_panic_returns_error() {
    ensure_v8();
    let heap_bytes = 8 * 1024 * 1024;
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(heap_bytes).console_tree(tree.clone());
    let (result, _oom) = server::engine::execute_stateful(
        r#"
        try {
            Deno.core.ops.op_panic("stateful panic");
            console.log("should not reach");
        } catch (e) {
            console.log("caught: " + e.message);
        }
        "#,
        None,
        config,
    );
    assert!(result.is_ok(), "Should not crash in stateful mode, got: {:?}", result);
    let output = read_console(&tree);
    assert!(
        output.contains("caught:") && output.contains("panic"),
        "Should catch panic in stateful mode, got: {}",
        output,
    );
    let _ = std::fs::remove_dir_all(&tmp);
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
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(heap_bytes).console_tree(tree.clone());
    let (result2, _) = server::engine::execute_stateless(
        r#"console.log(1 + 1)"#,
        config,
    );
    assert!(result2.is_ok(), "Second execution should succeed, got: {:?}", result2);
    let output = read_console(&tree);
    assert!(
        output.contains("2"),
        "Console should contain '2', got: {}",
        output,
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
