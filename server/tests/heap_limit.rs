/// Tests for V8 heap memory limits
///
/// These tests verify that the heap_limits parameter correctly constrains
/// V8 isolate memory usage, preventing unbounded heap growth.

use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

fn no_handle() -> Arc<Mutex<Option<v8::IsolateHandle>>> {
    Arc::new(Mutex::new(None))
}

/// JS code that allocates a large amount of memory by building a huge array of strings.
/// Each string element is ~10 bytes, so 2M elements ≈ 20MB+ of heap.
const MEMORY_HOG_JS: &str = r#"
var arr = [];
for (var i = 0; i < 2000000; i++) {
    arr.push("item_" + i);
}
arr.length;
"#;

/// Small JS that should succeed with any reasonable heap limit.
const SMALL_JS: &str = "1 + 1";

#[test]
fn test_stateless_small_heap_limit_rejects_large_allocation() {
    ensure_v8();

    // 5 MB heap limit — too small for a 2M-element string array
    let max_bytes = 5 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateless(MEMORY_HOG_JS, max_bytes, no_handle());

    assert!(
        result.is_err(),
        "Expected OOM error with 5MB heap limit, but got: {:?}",
        result
    );
}

#[test]
fn test_stateless_default_limit_allows_small_code() {
    ensure_v8();

    let max_bytes = server::engine::DEFAULT_HEAP_MEMORY_MAX_MB * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateless(SMALL_JS, max_bytes, no_handle());

    assert!(
        result.is_ok(),
        "Simple JS should succeed with default heap limit, but got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), "2");
}

#[test]
fn test_stateless_generous_limit_allows_large_allocation() {
    ensure_v8();

    // 256 MB — should be plenty for 2M strings
    let max_bytes = 256 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateless(MEMORY_HOG_JS, max_bytes, no_handle());

    assert!(
        result.is_ok(),
        "Large allocation should succeed with 256MB heap limit, but got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), "2000000");
}

#[test]
fn test_stateful_small_heap_limit_rejects_large_allocation() {
    ensure_v8();

    // 5 MB heap limit — too small for a 2M-element string array
    let max_bytes = 5 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateful(MEMORY_HOG_JS, None, max_bytes, no_handle());

    assert!(
        result.is_err(),
        "Expected OOM error with 5MB heap limit in stateful mode, but got: {:?}",
        result
    );
}

#[test]
fn test_stateful_default_limit_allows_small_code() {
    ensure_v8();

    let max_bytes = server::engine::DEFAULT_HEAP_MEMORY_MAX_MB * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateful(SMALL_JS, None, max_bytes, no_handle());

    assert!(
        result.is_ok(),
        "Simple JS should succeed with default heap limit in stateful mode, but got: {:?}",
        result
    );
    let (output, snapshot, _content_hash) = result.unwrap();
    assert_eq!(output, "2");
    assert!(!snapshot.is_empty(), "Snapshot should be non-empty");
}

#[test]
fn test_stateful_generous_limit_allows_large_allocation() {
    ensure_v8();

    // 256 MB — should be plenty for 2M strings
    let max_bytes = 256 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateful(MEMORY_HOG_JS, None, max_bytes, no_handle());

    assert!(
        result.is_ok(),
        "Large allocation should succeed with 256MB heap limit in stateful mode, but got: {:?}",
        result
    );
    let (output, _, _) = result.unwrap();
    assert_eq!(output, "2000000");
}

#[test]
fn test_different_limits_produce_different_outcomes() {
    ensure_v8();

    // Same code, different limits — small should fail, large should succeed
    let small_limit = 5 * 1024 * 1024;
    let large_limit = 256 * 1024 * 1024;

    let (small_result, _) = server::engine::execute_stateless(MEMORY_HOG_JS, small_limit, no_handle());
    let (large_result, _) = server::engine::execute_stateless(MEMORY_HOG_JS, large_limit, no_handle());

    assert!(
        small_result.is_err(),
        "5MB limit should reject large allocation"
    );
    assert!(
        large_result.is_ok(),
        "256MB limit should allow large allocation"
    );
}
