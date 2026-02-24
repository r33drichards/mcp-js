/// Tests for parameter configurations: timeout and heap_memory_max_mb behavior.
///
/// Timeout tests use Engine::run_js (the production async path with
/// tokio::select! timeout). OOM tests call execute_stateless/execute_stateful
/// directly since they don't need timeout management.

use std::sync::{Arc, Mutex, Once};
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn no_handle() -> Arc<Mutex<Option<v8::IsolateHandle>>> {
    Arc::new(Mutex::new(None))
}

// ── Timeout tests (async, using Engine::run_js) ─────────────────────────

/// An infinite loop with a short timeout should return a descriptive timeout error.
#[tokio::test]
async fn test_timeout_produces_descriptive_error() {
    ensure_v8();

    let engine = Engine::new_stateless(64 * 1024 * 1024, 2, 4);
    let result = engine.run_js(
        "while (true) {}".to_string(),
        None, None, None, None,
    ).await;

    assert!(result.is_err(), "Infinite loop should fail, got: {:?}", result);
    let err = result.unwrap_err();
    assert!(
        err.to_lowercase().contains("timeout") || err.to_lowercase().contains("timed out"),
        "Error should mention timeout, but got: {}", err
    );
}

/// Timeout should also work correctly in stateful mode.
#[tokio::test]
async fn test_timeout_stateful_produces_descriptive_error() {
    ensure_v8();

    let heap_storage = server::engine::heap_storage::AnyHeapStorage::File(
        server::engine::heap_storage::FileHeapStorage::new("/tmp/mcp-v8-test-timeout-stateful"),
    );
    let engine = Engine::new_stateful(heap_storage, None, 64 * 1024 * 1024, 2, 4);
    let result = engine.run_js(
        "while (true) {}".to_string(),
        None, None, None, None,
    ).await;

    assert!(result.is_err(), "Infinite loop should fail, got: {:?}", result);
    let err = result.unwrap_err();
    assert!(
        err.to_lowercase().contains("timeout") || err.to_lowercase().contains("timed out"),
        "Error should mention timeout, but got: {}", err
    );
}

// ── OOM tests (direct V8 calls) ─────────────────────────────────────────

/// Allocating a huge array with a small heap should return a descriptive OOM error.
#[test]
fn test_oom_produces_descriptive_error_not_crash() {
    ensure_v8();

    let code = r#"
        var arr = [];
        for (var i = 0; i < 50000000; i++) {
            arr.push("item_" + i);
        }
        arr.length;
    "#;
    let heap_bytes = 16 * 1024 * 1024; // 16MB

    let (result, _oom) = server::engine::execute_stateless(code, heap_bytes, no_handle(), &[], heap_bytes);

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
    "#;
    let heap_bytes = 16 * 1024 * 1024; // 16MB

    let (result, _oom) = server::engine::execute_stateful(code, None, heap_bytes, no_handle(), &[], heap_bytes);

    assert!(result.is_err(), "Huge allocation with small heap should fail, got: {:?}", result);
    let err = result.unwrap_err();
    assert!(
        err.to_lowercase().contains("memory") || err.to_lowercase().contains("oom") || err.to_lowercase().contains("heap"),
        "Error should mention memory/OOM/heap, but got: {}", err
    );
}

// ── Sanity checks ────────────────────────────────────────────────────────

/// A fast computation with generous limits should succeed.
#[test]
fn test_fast_computation_succeeds() {
    ensure_v8();

    let code = r#"
        var sum = 0;
        for (var i = 0; i < 1000000; i++) { sum += i; }
        sum;
    "#;
    let heap_bytes = 64 * 1024 * 1024;

    let (result, _oom) = server::engine::execute_stateless(code, heap_bytes, no_handle(), &[], heap_bytes);

    assert!(result.is_ok(), "Fast computation should succeed, got: {:?}", result);
    assert_eq!(result.unwrap(), "499999500000");
}

/// Bare call with no special params should work fine.
#[test]
fn test_bare_call_default_params() {
    ensure_v8();

    let heap_bytes = server::engine::DEFAULT_HEAP_MEMORY_MAX_MB * 1024 * 1024;

    let (result, _oom) = server::engine::execute_stateless("1 + 1", heap_bytes, no_handle(), &[], heap_bytes);

    assert!(result.is_ok(), "Simple expression should succeed, got: {:?}", result);
    assert_eq!(result.unwrap(), "2");
}
