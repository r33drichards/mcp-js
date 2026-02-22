#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::Once;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::mcp::initialize_v8();
    });
}

// Fuzz V8 snapshot deserialization by passing raw fuzzer bytes as a snapshot
// blob. This verifies that the snapshot envelope validation in
// execute_stateful correctly rejects arbitrary/corrupted data before it
// reaches V8's C++ snapshot deserializer (which would abort the process).
//
// Prior to the envelope validation fix, this target found that V8's
// Snapshot::Initialize calls V8_Fatal (abort) on invalid snapshot data,
// crashing the process. The fix wraps snapshots with a magic header that
// is validated before passing data to V8.
fuzz_target!(|data: &[u8]| {
    ensure_v8();

    // Always pass fuzzer data as a snapshot — this exercises the snapshot
    // validation code path.
    let snapshot = Some(data.to_vec());
    let code = "1".to_string();

    // Use a small heap limit for fuzzing to avoid process-level OOM
    let max_bytes = 64 * 1024 * 1024;
    let _ = server::mcp::execute_stateful(code, snapshot, max_bytes);
});
