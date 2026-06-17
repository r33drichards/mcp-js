
use arbitrary::Arbitrary;
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


struct FuzzWasmModule {
    bytes: Vec<u8>,
}


struct StatelessInput {
    code: String,
    wasm_modules: Vec<FuzzWasmModule>,
}

fuzz_target!(|input: StatelessInput| {
    ensure_v8();

        let mut wasm_input = input.wasm_modules;
    wasm_input.truncate(3);

    let modules: Vec<WasmModule> = wasm_input
        .into_iter()
        .enumerate()
        .map(|(i, m)| WasmModule {
            name: format!("m{}", i),
            bytes: m.bytes,
            max_memory_bytes: None,
            description: None,
        })
        .collect();

                    let max_bytes = 8 * 1024 * 1024;
    let wasm_default = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateless(&input.code, ExecutionConfig::new(max_bytes)
        .isolate_handle(handle)
        .wasm_modules(&modules)
        .wasm_default_max_bytes(wasm_default));
});
