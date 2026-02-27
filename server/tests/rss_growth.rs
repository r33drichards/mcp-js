/// RSS growth measurement for V8 isolate create/destroy cycles.
///
/// This test measures whether RSS grows across iterations of creating
/// and destroying V8 isolates — the same pattern used by fuzz targets.
/// Run WITHOUT ASAN to establish baseline; compare with ASAN CI results.
///
/// Hypotheses:
///   H1: ASAN shadow memory overhead — RSS flat without ASAN
///   H2: V8 internal caches/pools — RSS grows with and without ASAN
///   H3: Code-level resource leak — RSS grows proportional to leak size

use std::sync::{Arc, Mutex, Once};
use server::engine::{WasmModule, initialize_v8, execute_stateless, execute_stateful};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn no_handle() -> Arc<Mutex<Option<deno_core::v8::IsolateHandle>>> {
    Arc::new(Mutex::new(None))
}

/// Get current RSS in KB using ps (works on macOS and Linux).
fn get_rss_kb() -> usize {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("failed to run ps");
    let rss_str = String::from_utf8(output.stdout).unwrap();
    rss_str.trim().parse::<usize>().unwrap_or(0)
}

/// Measure RSS growth over N iterations of a closure.
/// Returns (initial_rss_kb, final_rss_kb, per_iter_growth_kb).
fn measure_rss_growth<F: FnMut()>(label: &str, iterations: usize, mut f: F) -> (usize, usize, f64) {
    // Warm up: run a few iterations to stabilize V8 internal state.
    for _ in 0..5 {
        f();
    }

    let rss_before = get_rss_kb();
    let checkpoint_interval = iterations / 5;

    for i in 0..iterations {
        f();
        if checkpoint_interval > 0 && (i + 1) % checkpoint_interval == 0 {
            let rss_now = get_rss_kb();
            let growth = rss_now as f64 - rss_before as f64;
            let per_iter = growth / (i + 1) as f64;
            eprintln!(
                "  [{label}] iter {}: RSS = {} KB (growth = {:.0} KB, {:.1} KB/iter)",
                i + 1, rss_now, growth, per_iter
            );
        }
    }

    let rss_after = get_rss_kb();
    let total_growth = rss_after as f64 - rss_before as f64;
    let per_iter = total_growth / iterations as f64;
    eprintln!(
        "  [{label}] FINAL: {} → {} KB (total growth = {:.0} KB, {:.1} KB/iter over {} iters)",
        rss_before, rss_after, total_growth, per_iter, iterations
    );
    (rss_before, rss_after, per_iter)
}

#[test]
fn test_rss_growth_stateless_no_wasm() {
    ensure_v8();
    let iterations = 500;
    let heap_bytes = 8 * 1024 * 1024;

    eprintln!("\n=== RSS Growth: execute_stateless (no WASM) ===");
    let (_before, _after, per_iter_kb) = measure_rss_growth(
        "stateless-no-wasm",
        iterations,
        || {
            let handle = no_handle();
            let _ = execute_stateless("1 + 1", heap_bytes, handle, &[], heap_bytes);
        },
    );

    eprintln!("Per-iteration RSS growth: {:.1} KB", per_iter_kb);
}

#[test]
fn test_rss_growth_stateless_with_invalid_wasm() {
    ensure_v8();
    let iterations = 500;
    let heap_bytes = 8 * 1024 * 1024;

    eprintln!("\n=== RSS Growth: execute_stateless (invalid WASM) ===");
    let (_before, _after, per_iter_kb) = measure_rss_growth(
        "stateless-invalid-wasm",
        iterations,
        || {
            let handle = no_handle();
            let modules = vec![WasmModule {
                name: "m".to_string(),
                bytes: vec![0xde, 0xad, 0xbe, 0xef],
                max_memory_bytes: None,
            }];
            let _ = execute_stateless("1", heap_bytes, handle, &modules, heap_bytes);
        },
    );

    eprintln!("Per-iteration RSS growth: {:.1} KB", per_iter_kb);
}

#[test]
fn test_rss_growth_stateful_no_wasm() {
    ensure_v8();
    let iterations = 500;
    let heap_bytes = 8 * 1024 * 1024;

    eprintln!("\n=== RSS Growth: execute_stateful (no WASM) ===");
    let (_before, _after, per_iter_kb) = measure_rss_growth(
        "stateful-no-wasm",
        iterations,
        || {
            let handle = no_handle();
            let _ = execute_stateful("1 + 1", None, heap_bytes, handle, &[], heap_bytes);
        },
    );

    eprintln!("Per-iteration RSS growth: {:.1} KB", per_iter_kb);
}

#[test]
fn test_rss_growth_stateless_syntax_error() {
    ensure_v8();
    let iterations = 500;
    let heap_bytes = 8 * 1024 * 1024;

    eprintln!("\n=== RSS Growth: execute_stateless (syntax error) ===");
    let (_before, _after, per_iter_kb) = measure_rss_growth(
        "stateless-syntax-error",
        iterations,
        || {
            let handle = no_handle();
            let _ = execute_stateless("= mon:= m {", heap_bytes, handle, &[], heap_bytes);
        },
    );

    eprintln!("Per-iteration RSS growth: {:.1} KB", per_iter_kb);
}
