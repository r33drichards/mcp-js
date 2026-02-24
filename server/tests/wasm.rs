/// Tests for WebAssembly support — verifies that V8 can compile and run
/// WASM modules via the WebAssembly JavaScript API.

use std::sync::Once;
use server::engine::{initialize_v8, Engine};

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
