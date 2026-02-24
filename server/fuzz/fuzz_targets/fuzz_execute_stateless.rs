#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::WasmModule;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

#[derive(Arbitrary, Debug)]
struct FuzzWasmModule {
    bytes: Vec<u8>,
}

#[derive(Arbitrary, Debug)]
struct StatelessInput {
    code: String,
    wasm_modules: Vec<FuzzWasmModule>,
}

// Fuzz the stateless V8 execution path with arbitrary JavaScript code strings
// and optional WASM modules. This exercises the V8 FFI boundary: string
// creation, script compilation, execution, result conversion, and WASM module
// compilation/instantiation — all of which rely on unsafe C++ interop inside
// the v8 crate.
fuzz_target!(|input: StatelessInput| {
    ensure_v8();

    // Cap modules to avoid excessive per-iteration overhead.
    let mut wasm_input = input.wasm_modules;
    wasm_input.truncate(3);

    let modules: Vec<WasmModule> = wasm_input
        .into_iter()
        .enumerate()
        .map(|(i, m)| WasmModule {
            name: format!("m{}", i),
            bytes: m.bytes,
        })
        .collect();

    // We don't care whether the JS succeeds or fails; we care that V8 doesn't
    // crash, corrupt memory, or trigger undefined behavior.
    // Use the production default (8MB) — with ASAN overhead, larger heaps
    // can cause OOM on CI runners.
    let max_bytes = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let _ = server::engine::execute_stateless(&input.code, max_bytes, handle, &modules);
});
