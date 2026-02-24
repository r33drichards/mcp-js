#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

// Fuzz the stateless V8 execution path with arbitrary JavaScript code strings.
// This exercises the V8 FFI boundary: string creation, script compilation,
// execution, and result conversion — all of which rely on unsafe C++ interop
// inside the v8 crate.
fuzz_target!(|data: &[u8]| {
    ensure_v8();

    // Treat the fuzzer input as a UTF-8 string (lossy — V8 must handle any input)
    let code = String::from_utf8_lossy(data);

    // We don't care whether the JS succeeds or fails; we care that V8 doesn't
    // crash, corrupt memory, or trigger undefined behavior.
    // Use the production default (8MB) — with ASAN overhead, larger heaps
    // can cause OOM on CI runners.
    let max_bytes = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateless(&code, max_bytes, handle, &[], &Default::default());
});
