#![no_main]
use libfuzzer_sys::fuzz_target;
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

// Fuzz V8 snapshot deserialization by passing raw fuzzer bytes as a snapshot
// blob. This verifies that the snapshot envelope validation in
// unwrap_snapshot correctly rejects arbitrary/corrupted data before it
// reaches V8's C++ snapshot deserializer (which would abort the process).
//
// Prior to the envelope validation fix, this target found that V8's
// Snapshot::Initialize calls V8_Fatal (abort) on invalid snapshot data,
// crashing the process. The fix wraps snapshots with a magic header that
// is validated before passing data to V8.
fuzz_target!(|data: &[u8]| {
    ensure_v8();

    // Validate the envelope — this is the code path under test.
    let raw_snapshot = match server::engine::unwrap_snapshot(data) {
        Ok(raw) => Some(raw),
        Err(_) => return, // Correctly rejected — nothing more to test.
    };

    // If validation passes (extremely unlikely with random data), run V8.
    let max_bytes = 64 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let wasm_default = 16 * 1024 * 1024;
    let _ = server::engine::execute_stateful("1", raw_snapshot, max_bytes, handle, &[], wasm_default, None, None, None);
});
