
use libfuzzer_sys::fuzz_target;
use server::engine::ExecutionConfig;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
                        deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
                                std::panic::set_hook(Box::new(|_| {}));
    });
}

fuzz_target!(|data: &[u8]| {
    ensure_v8();

        let raw_snapshot = match server::engine::unwrap_snapshot(data) {
        Ok(raw) => Some(raw),
        Err(_) => return,     };

        let max_bytes = 64 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let wasm_default = 16 * 1024 * 1024;
    let _ = server::engine::execute_stateful("1", raw_snapshot, ExecutionConfig::new(max_bytes)
        .isolate_handle(handle)
        .wasm_default_max_bytes(wasm_default));
});
