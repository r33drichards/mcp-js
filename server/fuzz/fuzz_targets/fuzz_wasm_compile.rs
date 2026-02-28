#![no_main]
use libfuzzer_sys::fuzz_target;
use server::engine::WasmModule;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

// Fuzz V8's WASM bytecode parser by passing arbitrary bytes as a WASM module.
// This is a focused target that exercises V8's WasmModuleObject::compile()
// (called inside inject_wasm_modules) with raw fuzzer data. The JS code is
// minimal ("1") so almost all fuzzer effort goes into exploring WASM
// compilation paths.
//
// Most inputs will fail at compile time (invalid magic, malformed sections,
// etc.), but the critical property is that V8 never crashes, corrupts memory,
// or triggers undefined behavior — it must return an error gracefully.
fuzz_target!(|data: &[u8]| {
    ensure_v8();

    let modules = vec![WasmModule {
        name: "m".to_string(),
        bytes: data.to_vec(),
        max_memory_bytes: Some(8 * 1024 * 1024),
    }];

    // We don't care about the result; we care that V8 doesn't crash.
    let max_bytes = 64 * 1024 * 1024;
    let wasm_default = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateless("1", max_bytes, handle, &modules, wasm_default, None);
});
