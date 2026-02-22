#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::Once;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::mcp::initialize_v8();
    });
}

// Fuzz V8 snapshot deserialization specifically by always providing the raw
// fuzzer bytes as a snapshot blob. This is the most security-critical fuzz
// target because:
//
//   - V8 snapshot deserialization involves unsafe C++ code that parses a
//     binary format to reconstruct a full V8 heap
//   - Malformed snapshots could trigger out-of-bounds reads/writes,
//     use-after-free, or type confusion in V8's deserializer
//   - The `execute_stateful` function passes user-controlled bytes directly
//     to `v8::Isolate::snapshot_creator_from_existing_snapshot`
//
// A minimal valid JS string is used as code so the fuzzer focuses its
// mutation energy on the snapshot bytes.
fuzz_target!(|data: &[u8]| {
    ensure_v8();

    // Always pass fuzzer data as a snapshot — this maximizes coverage of
    // the snapshot deserialization code path.
    let snapshot = Some(data.to_vec());
    let code = "1".to_string();

    let max_bytes = server::mcp::DEFAULT_HEAP_MEMORY_MAX_MB * 1024 * 1024;
    let _ = server::mcp::execute_stateful(code, snapshot, max_bytes);
});
