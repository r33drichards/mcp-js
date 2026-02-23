#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::Once;

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
    let code = String::from_utf8_lossy(data).into_owned();

    // We don't care whether the JS succeeds or fails; we care that V8 doesn't
    // crash, corrupt memory, or trigger undefined behavior.
    // Use a small heap limit for fuzzing to avoid process-level OOM
    let max_bytes = 64 * 1024 * 1024;
    // Use a short timeout to prevent slow-unit failures from pathological inputs
    let timeout_secs = 5;
    let _ = server::engine::execute_stateless(code, max_bytes, timeout_secs);
});
