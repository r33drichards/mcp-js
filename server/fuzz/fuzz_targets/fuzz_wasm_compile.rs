
use libfuzzer_sys::fuzz_target;
use server::engine::{ExecutionConfig, WasmModule};
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

    let modules = vec![WasmModule {
        name: "m".to_string(),
        bytes: data.to_vec(),
        max_memory_bytes: Some(8 * 1024 * 1024),
        description: None,
    }];

        let max_bytes = 64 * 1024 * 1024;
    let wasm_default = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateless("1", ExecutionConfig::new(max_bytes)
        .isolate_handle(handle)
        .wasm_modules(&modules)
        .wasm_default_max_bytes(wasm_default));
});
