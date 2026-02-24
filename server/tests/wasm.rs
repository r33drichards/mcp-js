/// Tests for WebAssembly support — verifies that V8 can compile and run
/// WASM modules via the WebAssembly JavaScript API, including pre-loaded
/// global WASM modules.

use std::sync::Once;
use server::engine::{initialize_v8, Engine, WasmModule};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

/// Test basic WASM module: compile, instantiate, and call an exported `add` function.
#[tokio::test]
async fn test_wasm_add_function() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let code = r#"
const wasmBytes = new Uint8Array([
  0x00,0x61,0x73,0x6d, // magic
  0x01,0x00,0x00,0x00, // version
  0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f, // type: (i32,i32)->i32
  0x03,0x02,0x01,0x00, // function section
  0x07,0x07,0x01,0x03,0x61,0x64,0x64,0x00,0x00, // export "add"
  0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6a,0x0b // body: local.get 0, local.get 1, i32.add
]);
const mod = new WebAssembly.Module(wasmBytes);
const inst = new WebAssembly.Instance(mod);
inst.exports.add(21, 21);
"#;

    let result = engine.run_js(code.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "WASM add should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "42");
}

/// Test that WebAssembly.validate correctly identifies valid WASM bytes.
#[tokio::test]
async fn test_wasm_validate() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let code = r#"
const wasmBytes = new Uint8Array([
  0x00,0x61,0x73,0x6d,
  0x01,0x00,0x00,0x00,
  0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f,
  0x03,0x02,0x01,0x00,
  0x07,0x07,0x01,0x03,0x61,0x64,0x64,0x00,0x00,
  0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6a,0x0b
]);
WebAssembly.validate(wasmBytes);
"#;

    let result = engine.run_js(code.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "WASM validate should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "true");
}

/// Test that WebAssembly.validate rejects invalid bytes.
#[tokio::test]
async fn test_wasm_validate_invalid() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let code = r#"
const invalidBytes = new Uint8Array([0x00, 0x01, 0x02, 0x03]);
WebAssembly.validate(invalidBytes);
"#;

    let result = engine.run_js(code.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "WASM validate on invalid bytes should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "false");
}

/// Test WASM module with a multiply function.
#[tokio::test]
async fn test_wasm_multiply_function() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    // WASM module exporting a multiply(i32, i32) -> i32 function
    let code = r#"
const wasmBytes = new Uint8Array([
  0x00,0x61,0x73,0x6d, // magic
  0x01,0x00,0x00,0x00, // version
  0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f, // type: (i32,i32)->i32
  0x03,0x02,0x01,0x00, // function section
  0x07,0x0c,0x01,0x08,0x6d,0x75,0x6c,0x74,0x69,0x70,0x6c,0x79,0x00,0x00, // export "multiply"
  0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6c,0x0b // body: local.get 0, local.get 1, i32.mul
]);
const mod = new WebAssembly.Module(wasmBytes);
const inst = new WebAssembly.Instance(mod);
inst.exports.multiply(6, 7);
"#;

    let result = engine.run_js(code.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "WASM multiply should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "42");
}

/// Test that invalid WASM module throws an error at compile time.
#[tokio::test]
async fn test_wasm_compile_error() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let code = r#"
try {
  const bad = new Uint8Array([0x00, 0x01, 0x02, 0x03]);
  new WebAssembly.Module(bad);
  "no error";
} catch (e) {
  e instanceof WebAssembly.CompileError;
}
"#;

    let result = engine.run_js(code.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "WASM compile error test should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "true");
}

// ── Pre-loaded global WASM module tests ──────────────────────────────────

/// WASM bytes for an `add(i32, i32) -> i32` module.
fn add_wasm_bytes() -> Vec<u8> {
    vec![
        0x00,0x61,0x73,0x6d, // magic
        0x01,0x00,0x00,0x00, // version
        0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f, // type: (i32,i32)->i32
        0x03,0x02,0x01,0x00, // function section
        0x07,0x07,0x01,0x03,0x61,0x64,0x64,0x00,0x00, // export "add"
        0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6a,0x0b, // body
    ]
}

/// WASM bytes for a `multiply(i32, i32) -> i32` module.
fn multiply_wasm_bytes() -> Vec<u8> {
    vec![
        0x00,0x61,0x73,0x6d, // magic
        0x01,0x00,0x00,0x00, // version
        0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f, // type: (i32,i32)->i32
        0x03,0x02,0x01,0x00, // function section
        0x07,0x0c,0x01,0x08,0x6d,0x75,0x6c,0x74,0x69,0x70,0x6c,0x79,0x00,0x00, // export "multiply"
        0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6c,0x0b, // body
    ]
}

/// Test that a pre-loaded WASM module is available as a global.
#[tokio::test]
async fn test_wasm_global_module_add() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![
            WasmModule { name: "math".to_string(), bytes: add_wasm_bytes() },
        ]);

    let result = engine.run_js("math.add(21, 21);".to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "Global WASM add should work, got: {:?}", result);
    assert_eq!(result.unwrap().output, "42");
}

/// Test multiple pre-loaded WASM modules.
#[tokio::test]
async fn test_wasm_multiple_global_modules() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![
            WasmModule { name: "adder".to_string(), bytes: add_wasm_bytes() },
            WasmModule { name: "multiplier".to_string(), bytes: multiply_wasm_bytes() },
        ]);

    let code = "adder.add(10, 5) + multiplier.multiply(3, 4);";
    let result = engine.run_js(code.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "Multiple global WASM modules should work, got: {:?}", result);
    assert_eq!(result.unwrap().output, "27"); // 15 + 12
}

/// Test that global WASM modules work alongside user-written JS.
#[tokio::test]
async fn test_wasm_global_with_user_code() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![
            WasmModule { name: "calc".to_string(), bytes: add_wasm_bytes() },
        ]);

    let code = r#"
var results = [];
for (var i = 0; i < 5; i++) {
    results.push(calc.add(i, i));
}
JSON.stringify(results);
"#;

    let result = engine.run_js(code.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "WASM global with user code should work, got: {:?}", result);
    assert_eq!(result.unwrap().output, "[0,2,4,6,8]");
}

/// Test that no global modules means no preamble overhead.
#[tokio::test]
async fn test_wasm_no_modules_no_preamble() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    // typeof should be "undefined" if no WASM globals are injected
    let result = engine.run_js("typeof math;".to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "No modules should mean no globals, got: {:?}", result);
    assert_eq!(result.unwrap().output, "undefined");
}

// ── File-path loading tests ──────────────────────────────────────────────

/// Test loading a WASM module from a `.wasm` file on disk.
#[tokio::test]
async fn test_wasm_load_from_filepath() {
    ensure_v8();

    let wasm_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("add.wasm");

    let bytes = std::fs::read(&wasm_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", wasm_path.display(), e));

    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![
            WasmModule { name: "math".to_string(), bytes },
        ]);

    let result = engine.run_js("math.add(21, 21);".to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "WASM loaded from filepath should work, got: {:?}", result);
    assert_eq!(result.unwrap().output, "42");
}

// ── __wasm_<name> Module exposure tests ──────────────────────────────────

/// Self-contained modules expose both `<name>` (exports) and `__wasm_<name>` (Module).
#[tokio::test]
async fn test_wasm_module_global_exposed_for_no_imports() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![
            WasmModule { name: "math".to_string(), bytes: add_wasm_bytes() },
        ]);

    // __wasm_math should be a WebAssembly.Module
    let code = "__wasm_math instanceof WebAssembly.Module;";
    let result = engine.run_js(code.to_string(), None, None, None, None).await;
    assert!(result.is_ok(), "Module global should exist, got: {:?}", result);
    assert_eq!(result.unwrap().output, "true");
}

/// The exposed Module can be manually instantiated from JS.
#[tokio::test]
async fn test_wasm_module_global_manual_instantiation() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![
            WasmModule { name: "math".to_string(), bytes: add_wasm_bytes() },
        ]);

    let code = r#"
var inst = new WebAssembly.Instance(__wasm_math);
inst.exports.add(100, 200);
"#;
    let result = engine.run_js(code.to_string(), None, None, None, None).await;
    assert!(result.is_ok(), "Manual instantiation should work, got: {:?}", result);
    assert_eq!(result.unwrap().output, "300");
}

/// WASM bytes for a module that imports a function: (import "env" "double" (func (param i32) (result i32)))
/// Exports: triple(i32) -> i32  which calls double(x) + x
fn wasm_with_import_bytes() -> Vec<u8> {
    // (module
    //   (type (func (param i32) (result i32)))
    //   (import "env" "double" (func (type 0)))
    //   (func (type 0) (param i32) (result i32) local.get 0  call 0  local.get 0  i32.add)
    //   (export "triple" (func 1)))
    vec![
        0x00,0x61,0x73,0x6d, 0x01,0x00,0x00,0x00, // header
        // Type section: 1 type (i32)->i32
        0x01, 0x06, 0x01, 0x60, 0x01, 0x7f, 0x01, 0x7f,
        // Import section (length=14): import "env"."double" as func type 0
        0x02, 0x0e, 0x01, 0x03, 0x65,0x6e,0x76, 0x06, 0x64,0x6f,0x75,0x62,0x6c,0x65, 0x00, 0x00,
        // Function section: 1 function, type 0
        0x03, 0x02, 0x01, 0x00,
        // Export section: export "triple" as func 1
        0x07, 0x0a, 0x01, 0x06, 0x74,0x72,0x69,0x70,0x6c,0x65, 0x00, 0x01,
        // Code section: body = local.get 0, call 0, local.get 0, i32.add, end
        0x0a, 0x0b, 0x01, 0x09, 0x00, 0x20,0x00, 0x10,0x00, 0x20,0x00, 0x6a, 0x0b,
    ]
}

/// Modules with imports are NOT auto-instantiated but the Module is still exposed.
#[tokio::test]
async fn test_wasm_module_with_imports_not_auto_instantiated() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![
            WasmModule { name: "mymod".to_string(), bytes: wasm_with_import_bytes() },
        ]);

    // The auto-instantiated `mymod` should NOT exist (module has imports)
    let code = "typeof mymod;";
    let result = engine.run_js(code.to_string(), None, None, None, None).await;
    assert!(result.is_ok(), "Should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "undefined");
}

/// Modules with imports expose __wasm_<name> as a WebAssembly.Module for manual instantiation.
#[tokio::test]
async fn test_wasm_module_with_imports_manual_instantiation() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![
            WasmModule { name: "mymod".to_string(), bytes: wasm_with_import_bytes() },
        ]);

    // Manually instantiate with the required import
    let code = r#"
var inst = new WebAssembly.Instance(__wasm_mymod, {
    env: { double: function(x) { return x * 2; } }
});
inst.exports.triple(10);
"#;
    let result = engine.run_js(code.to_string(), None, None, None, None).await;
    assert!(result.is_ok(), "Manual instantiation with imports should work, got: {:?}", result);
    assert_eq!(result.unwrap().output, "30"); // double(10) + 10 = 20 + 10 = 30
}
