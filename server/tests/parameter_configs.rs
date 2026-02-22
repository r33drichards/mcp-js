/// Tests for parameter configurations: timeout and heap_memory_max_mb behavior.
///
/// These tests reproduce issues found during manual testing:
/// 1. OOM with small heap causes a thread panic ("Task join error") instead of a graceful error
/// 2. Timeout and OOM both produce generic "Failed to run script" instead of descriptive messages

use std::sync::Once;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::mcp::initialize_v8();
    });
}

// ── Timeout tests ──────────────────────────────────────────────────────────

/// An infinite loop with a short timeout should return a descriptive timeout error,
/// not the generic "Failed to run script".
#[test]
fn test_timeout_produces_descriptive_error() {
    ensure_v8();

    let code = "while (true) {}".to_string();
    let heap_bytes = 64 * 1024 * 1024; // 64MB - plenty of memory
    let timeout_secs = 2;

    let result = server::mcp::execute_stateless(code, heap_bytes, timeout_secs);

    assert!(result.is_err(), "Infinite loop should fail, got: {:?}", result);
    let err = result.unwrap_err();
    assert!(
        err.to_lowercase().contains("timeout") || err.to_lowercase().contains("timed out"),
        "Error should mention timeout, but got: {}", err
    );
}

/// Timeout should also work correctly in stateful mode.
#[test]
fn test_timeout_stateful_produces_descriptive_error() {
    ensure_v8();

    let code = "while (true) {}".to_string();
    let heap_bytes = 64 * 1024 * 1024;
    let timeout_secs = 2;

    let result = server::mcp::execute_stateful(code, None, heap_bytes, timeout_secs);

    assert!(result.is_err(), "Infinite loop should fail, got: {:?}", result);
    let err = result.unwrap_err();
    assert!(
        err.to_lowercase().contains("timeout") || err.to_lowercase().contains("timed out"),
        "Error should mention timeout, but got: {}", err
    );
}

// ── OOM tests ──────────────────────────────────────────────────────────────

/// Allocating a huge array with a small heap should return a descriptive OOM error,
/// not crash the thread or return "Failed to run script".
#[test]
fn test_oom_produces_descriptive_error_not_crash() {
    ensure_v8();

    // 50M element array with 16MB heap - this previously caused a thread panic
    let code = r#"
        var arr = [];
        for (var i = 0; i < 50000000; i++) {
            arr.push("item_" + i);
        }
        arr.length;
    "#.to_string();
    let heap_bytes = 16 * 1024 * 1024; // 16MB
    let timeout_secs = 30;

    let result = server::mcp::execute_stateless(code, heap_bytes, timeout_secs);

    assert!(result.is_err(), "Huge allocation with small heap should fail, got: {:?}", result);
    let err = result.unwrap_err();
    assert!(
        err.to_lowercase().contains("memory") || err.to_lowercase().contains("oom") || err.to_lowercase().contains("heap"),
        "Error should mention memory/OOM/heap, but got: {}", err
    );
}

/// OOM in stateful mode should also produce a descriptive error.
#[test]
fn test_oom_stateful_produces_descriptive_error_not_crash() {
    ensure_v8();

    let code = r#"
        var arr = [];
        for (var i = 0; i < 50000000; i++) {
            arr.push("item_" + i);
        }
        arr.length;
    "#.to_string();
    let heap_bytes = 16 * 1024 * 1024; // 16MB
    let timeout_secs = 30;

    let result = server::mcp::execute_stateful(code, None, heap_bytes, timeout_secs);

    assert!(result.is_err(), "Huge allocation with small heap should fail, got: {:?}", result);
    let err = result.unwrap_err();
    assert!(
        err.to_lowercase().contains("memory") || err.to_lowercase().contains("oom") || err.to_lowercase().contains("heap"),
        "Error should mention memory/OOM/heap, but got: {}", err
    );
}

// ── Sanity checks ──────────────────────────────────────────────────────────

/// A fast computation with generous limits should succeed.
#[test]
fn test_fast_computation_with_timeout_succeeds() {
    ensure_v8();

    let code = r#"
        var sum = 0;
        for (var i = 0; i < 1000000; i++) { sum += i; }
        sum;
    "#.to_string();
    let heap_bytes = 64 * 1024 * 1024;
    let timeout_secs = 5;

    let result = server::mcp::execute_stateless(code, heap_bytes, timeout_secs);

    assert!(result.is_ok(), "Fast computation should succeed, got: {:?}", result);
    assert_eq!(result.unwrap(), "499999500000");
}

/// Bare call with no special params should work fine.
#[test]
fn test_bare_call_default_params() {
    ensure_v8();

    let code = "1 + 1".to_string();
    let heap_bytes = server::mcp::DEFAULT_HEAP_MEMORY_MAX_MB * 1024 * 1024;
    let timeout_secs = server::mcp::DEFAULT_EXECUTION_TIMEOUT_SECS;

    let result = server::mcp::execute_stateless(code, heap_bytes, timeout_secs);

    assert!(result.is_ok(), "Simple expression should succeed, got: {:?}", result);
    assert_eq!(result.unwrap(), "2");
}
