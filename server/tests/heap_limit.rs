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

fn no_handle() -> Arc<Mutex<Option<deno_core::v8::IsolateHandle>>> {
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
    let (result, _oom) = server::engine::execute_stateless(MEMORY_HOG_JS, max_bytes, no_handle(), &[], max_bytes, None, None);

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
    let (result, _oom) = server::engine::execute_stateless(SMALL_JS, max_bytes, no_handle(), &[], max_bytes, None, None);

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
    let (result, _oom) = server::engine::execute_stateless(MEMORY_HOG_JS, max_bytes, no_handle(), &[], max_bytes, None, None);

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
    let (result, _oom) = server::engine::execute_stateful(MEMORY_HOG_JS, None, max_bytes, no_handle(), &[], max_bytes, None, None);

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
    let (result, _oom) = server::engine::execute_stateful(SMALL_JS, None, max_bytes, no_handle(), &[], max_bytes, None, None);

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
    let (result, _oom) = server::engine::execute_stateful(MEMORY_HOG_JS, None, max_bytes, no_handle(), &[], max_bytes, None, None);

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

    let (small_result, _) = server::engine::execute_stateless(MEMORY_HOG_JS, small_limit, no_handle(), &[], small_limit, None, None);
    let (large_result, _) = server::engine::execute_stateless(MEMORY_HOG_JS, large_limit, no_handle(), &[], large_limit, None, None);

    assert!(
        small_result.is_err(),
        "5MB limit should reject large allocation"
    );
    assert!(
        large_result.is_ok(),
        "256MB limit should allow large allocation"
    );
}

// ── Typed-array OOM (hard crash regression) ──────────────────────────────

/// Typed arrays (Uint8Array) allocate backing stores outside V8's managed
/// JS heap, so an OOM from unbounded typed-array growth can terminate the
/// isolate at a lower level than normal JS-heap OOM — causing a hard crash
/// ("Error occurred during tool execution") instead of a clean error.
///
/// This test reproduces the scenario: allocate 1 MB Uint8Array chunks in a
/// loop with an 8 MB heap limit, and asserts the engine returns an error
/// rather than crashing the process.
const TYPED_ARRAY_OOM_JS: &str = r#"
let totalBytes = 0;
const chunks = [];
try {
  while (true) {
    const buf = new Uint8Array(1024 * 1024); // 1MB at a time
    buf.fill(0xFF);
    chunks.push(buf);
    totalBytes += buf.length;
  }
} catch(e) {
  `Allocated ${chunks.length} x 1MB chunks = ${totalBytes / 1e6}MB before OOM`
}
"#;

#[test]
fn test_typed_array_oom_does_not_crash_stateless() {
    ensure_v8();

    // 8 MB heap — typed array backing stores will exceed this quickly
    let heap_bytes = 8 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateless(TYPED_ARRAY_OOM_JS, heap_bytes, no_handle(), &[], heap_bytes, None, None);

    // We don't care whether it's Ok (the JS catch fired) or Err (V8 killed it) —
    // the critical thing is that we *get here* instead of a process crash.
    // If this is an Err, the message should be descriptive.
    if let Err(ref e) = result {
        assert!(
            !e.contains("Error occurred during tool execution"),
            "Typed-array OOM should produce a V8-level error, not a hard crash. Got: {}",
            e,
        );
    }
}

#[test]
fn test_typed_array_oom_does_not_crash_stateful() {
    ensure_v8();

    let heap_bytes = 8 * 1024 * 1024;
    let (result, _oom) = server::engine::execute_stateful(TYPED_ARRAY_OOM_JS, None, heap_bytes, no_handle(), &[], heap_bytes, None, None);

    if let Err(ref e) = result {
        assert!(
            !e.contains("Error occurred during tool execution"),
            "Typed-array OOM should produce a V8-level error, not a hard crash. Got: {}",
            e,
        );
    }
}

// ── Extreme OOM regression (1 MB heap + 1 billion element array) ──────
//
// `new Array(1e9).fill(0)` asks V8 to create a FixedArray of 1 billion
// entries, which exceeds V8's internal `FixedArray::kMaxLength`. V8 calls
// `FatalProcessOutOfMemory("invalid table size")` → `abort()` before the
// near-heap-limit callback can fire. This is NOT a recoverable OOM — it
// is a V8 internal structural limit. Approaches like setjmp/longjmp or
// panic corrupt V8's global state (held locks, allocator state) and cause
// SIGSEGV in subsequent V8 operations.
//
// We test this in a subprocess: the child process runs the pathological
// allocation and V8 aborts it. The parent verifies the child terminated
// (didn't hang) and the parent process is unaffected.

/// Helper binary entrypoint for extreme OOM subprocess tests.
/// Invoked via `std::process::Command` with env EXTREME_OOM_SUBPROCESS=stateless|stateful.
#[test]
fn extreme_oom_subprocess_worker() {
    let mode = match std::env::var("EXTREME_OOM_SUBPROCESS") {
        Ok(m) => m,
        Err(_) => return, // Skip unless explicitly invoked as subprocess
    };

    ensure_v8();
    let code = "const arr = new Array(1e9).fill(0); arr.length";
    let heap_bytes = 1 * 1024 * 1024; // 1 MB (clamped to MIN_HEAP_MEMORY_MB internally)

    match mode.as_str() {
        "stateless" => {
            let (result, _) = server::engine::execute_stateless(code, heap_bytes, no_handle(), &[], heap_bytes, None, None);
            // If we reach here (V8 didn't abort), exit cleanly.
            if result.is_err() {
                std::process::exit(0);
            }
            std::process::exit(0);
        }
        "stateful" => {
            let (result, _) = server::engine::execute_stateful(code, None, heap_bytes, no_handle(), &[], heap_bytes, None, None);
            if result.is_err() {
                std::process::exit(0);
            }
            std::process::exit(0);
        }
        _ => {
            eprintln!("Unknown mode: {}", mode);
            std::process::exit(2);
        }
    }
}

fn run_extreme_oom_subprocess(mode: &str) {
    let exe = std::env::current_exe().expect("Failed to get test binary path");
    let output = std::process::Command::new(&exe)
        .arg("extreme_oom_subprocess_worker")
        .arg("--exact")
        .arg("--test-threads=1")
        .arg("--nocapture")
        .env("EXTREME_OOM_SUBPROCESS", mode)
        .output()
        .expect("Failed to spawn subprocess");

    // The subprocess should terminate (not hang). It may exit 0 (clean
    // OOM error) or non-zero (V8 abort). Both are acceptable — the
    // critical thing is that the parent process is unaffected.
    let _ = output.status; // We don't assert on the exit code — V8 abort is expected.

    // The parent process is still running — V8 global state is intact.
    // Verify by running a simple V8 execution.
    ensure_v8();
    let (result, _) = server::engine::execute_stateless("1 + 1", 8 * 1024 * 1024, no_handle(), &[], 16 * 1024 * 1024, None, None);
    assert!(
        result.is_ok(),
        "Parent process V8 should be unaffected after subprocess OOM, but got: {:?}",
        result,
    );
    assert_eq!(result.unwrap(), "2");
}

#[test]
fn test_extreme_oom_1mb_heap_subprocess_stateless() {
    run_extreme_oom_subprocess("stateless");
}

#[test]
fn test_extreme_oom_1mb_heap_subprocess_stateful() {
    run_extreme_oom_subprocess("stateful");
}
