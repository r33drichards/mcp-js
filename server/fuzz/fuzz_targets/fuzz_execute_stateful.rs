#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

#[derive(Arbitrary, Debug)]
struct StatefulInput {
    code: String,
    has_snapshot: bool,
    snapshot_bytes: Vec<u8>,
}

// Fuzz the stateful V8 execution path with arbitrary JavaScript code and
// optional heap snapshot data. This verifies that:
//   1. V8 script compilation and execution handles arbitrary code safely
//   2. The snapshot envelope validation rejects invalid/corrupted data
//      gracefully (returning Err) instead of letting V8 abort the process
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

    // Run stateful execution — we don't care about the result, only that
    // it doesn't crash.
    // Use the production default (8MB) — with ASAN overhead, larger heaps
    // can cause OOM on CI runners.
    let max_bytes = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateful(&input.code, raw_snapshot, max_bytes, handle, &[], &Default::default());
});
