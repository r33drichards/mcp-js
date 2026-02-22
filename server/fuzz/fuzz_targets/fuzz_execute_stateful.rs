#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use std::sync::Once;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::mcp::initialize_v8();
    });
}

#[derive(Arbitrary, Debug)]
struct StatefulInput {
    code: String,
    has_snapshot: bool,
    snapshot_bytes: Vec<u8>,
}

// Fuzz the stateful V8 execution path with arbitrary JavaScript code and
// optional heap snapshot data. This targets two unsafe surfaces:
//   1. V8 script compilation and execution (same as stateless)
//   2. V8 snapshot deserialization — feeding arbitrary bytes as a serialized
//      V8 heap, which exercises unsafe snapshot parsing in the V8 engine.
//
// The snapshot deserialization path is particularly interesting because
// corrupted or adversarial snapshot blobs could trigger memory safety
// issues in V8's C++ deserialization code.
fuzz_target!(|input: StatefulInput| {
    ensure_v8();

    let snapshot = if input.has_snapshot {
        Some(input.snapshot_bytes)
    } else {
        None
    };

    // Run stateful execution — we don't care about the result, only that
    // V8 handles arbitrary inputs without memory corruption.
    let max_bytes = server::mcp::DEFAULT_HEAP_MEMORY_MAX_MB * 1024 * 1024;
    let _ = server::mcp::execute_stateful(input.code, snapshot, max_bytes);
});
