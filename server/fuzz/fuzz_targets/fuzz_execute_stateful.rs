#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use std::sync::Once;

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

    let snapshot = if input.has_snapshot {
        Some(input.snapshot_bytes)
    } else {
        None
    };

    // Run stateful execution — we don't care about the result, only that
    // it doesn't crash. Invalid snapshots should be rejected by the
    // envelope validation before reaching V8.
    let max_bytes = 64 * 1024 * 1024;
    // Use a short timeout to prevent slow-unit failures from pathological inputs
    let timeout_secs = 5;
    let _ = server::engine::execute_stateful(input.code, snapshot, max_bytes, timeout_secs);
});
