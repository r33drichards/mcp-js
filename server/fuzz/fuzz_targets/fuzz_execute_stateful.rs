#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::WasmModule;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        // Disable V8 background threads (Maglev JIT, TurboFan, concurrent GC)
        // to prevent cumulative memory exhaustion across fuzz iterations.
        deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
        // Override libfuzzer's abort-on-panic hook so that catch_unwind in
        // execute_stateful can handle panics in deno_core/V8 gracefully.
        // Real crashes (ASAN, signals) are still caught by libfuzzer.
        std::panic::set_hook(Box::new(|_| {}));
    });
}

#[derive(Arbitrary, Debug)]
struct FuzzWasmModule {
    bytes: Vec<u8>,
}

#[derive(Arbitrary, Debug)]
struct StatefulInput {
    code: String,
    has_snapshot: bool,
    snapshot_bytes: Vec<u8>,
    wasm_modules: Vec<FuzzWasmModule>,
}

// Fuzz the stateful V8 execution path with arbitrary JavaScript code,
// optional heap snapshot data, and optional WASM modules. This verifies that:
//   1. V8 script compilation and execution handles arbitrary code safely
//   2. The snapshot envelope validation rejects invalid/corrupted data
//      gracefully (returning Err) instead of letting V8 abort the process
//   3. WASM module compilation handles arbitrary bytes without crashing
fuzz_target!(|input: StatefulInput| {
    ensure_v8();

    let raw_snapshot = if input.has_snapshot {
        // Validate envelope — invalid data is rejected here, not inside V8.
        match server::engine::unwrap_snapshot(&input.snapshot_bytes) {
            Ok(raw) => Some(raw),
            Err(_) => None,
        }
    } else {
        None
    };

    // Cap modules to avoid excessive per-iteration overhead.
    let mut wasm_input = input.wasm_modules;
    wasm_input.truncate(3);

    let modules: Vec<WasmModule> = wasm_input
        .into_iter()
        .enumerate()
        .map(|(i, m)| WasmModule {
            name: format!("m{}", i),
            bytes: m.bytes,
            max_memory_bytes: None,
        })
        .collect();

    // Run stateful execution — we don't care about the result, only that
    // it doesn't crash.
    // Use the production default (8MB) — with ASAN overhead, larger heaps
    // can cause OOM on CI runners.
    let max_bytes = 8 * 1024 * 1024;
    let wasm_default = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateful(&input.code, raw_snapshot, max_bytes, handle, &modules, wasm_default, None, None);
});
