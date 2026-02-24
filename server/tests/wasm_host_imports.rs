/// Tests for WASM host imports: filesystem, networking, and OPA policy middleware.
///
/// Uses the `wat` crate to compile WAT text into WASM bytes for test modules
/// that declare host imports (env.host_read_file, env.host_write_file, etc.).

use std::sync::Once;
use server::engine::{initialize_v8, Engine, WasmModule};
use server::engine::wasm_host::WasmHostConfig;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

/// Build a regorus engine from an inline Rego policy string.
fn policy_engine(rego: &str) -> regorus::Engine {
    let mut engine = regorus::Engine::new();
    engine
        .add_policy("test.rego".to_string(), rego.to_string())
        .expect("Failed to parse test policy");
    engine
}

/// WAT module that imports host_read_file and exports:
/// - memory, alloc (required by host)
/// - do_read(ptr, len) -> i32: forwards to host_read_file
fn read_file_wat() -> Vec<u8> {
    wat::parse_str(
        r#"
        (module
            (import "env" "host_read_file" (func $host_read_file (param i32 i32) (result i32)))
            (memory (export "memory") 1)

            ;; Simple bump allocator starting at 4096
            (global $heap (mut i32) (i32.const 4096))
            (func (export "alloc") (param $size i32) (result i32)
                (local $ptr i32)
                (local.set $ptr (global.get $heap))
                (global.set $heap (i32.add (local.get $ptr) (local.get $size)))
                (local.get $ptr)
            )

            ;; Forward to host_read_file
            (func (export "do_read") (param $ptr i32) (param $len i32) (result i32)
                (call $host_read_file (local.get $ptr) (local.get $len))
            )
        )
    "#,
    )
    .expect("Failed to parse read WAT")
}

/// WAT module that imports host_write_file and exports:
/// - memory, alloc
/// - do_write(path_ptr, path_len, data_ptr, data_len) -> i32
fn write_file_wat() -> Vec<u8> {
    wat::parse_str(
        r#"
        (module
            (import "env" "host_write_file" (func $host_write_file (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)

            (global $heap (mut i32) (i32.const 4096))
            (func (export "alloc") (param $size i32) (result i32)
                (local $ptr i32)
                (local.set $ptr (global.get $heap))
                (global.set $heap (i32.add (local.get $ptr) (local.get $size)))
                (local.get $ptr)
            )

            (func (export "do_write") (param $pp i32) (param $pl i32) (param $dp i32) (param $dl i32) (result i32)
                (call $host_write_file (local.get $pp) (local.get $pl) (local.get $dp) (local.get $dl))
            )
        )
    "#,
    )
    .expect("Failed to parse write WAT")
}

/// WAT module that imports host_last_error and host_read_file.
fn error_wat() -> Vec<u8> {
    wat::parse_str(
        r#"
        (module
            (import "env" "host_read_file" (func $host_read_file (param i32 i32) (result i32)))
            (import "env" "host_last_error" (func $host_last_error (result i32)))
            (memory (export "memory") 1)

            (global $heap (mut i32) (i32.const 4096))
            (func (export "alloc") (param $size i32) (result i32)
                (local $ptr i32)
                (local.set $ptr (global.get $heap))
                (global.set $heap (i32.add (local.get $ptr) (local.get $size)))
                (local.get $ptr)
            )

            (func (export "do_read") (param $ptr i32) (param $len i32) (result i32)
                (call $host_read_file (local.get $ptr) (local.get $len))
            )

            (func (export "get_error") (result i32)
                (call $host_last_error)
            )
        )
    "#,
    )
    .expect("Failed to parse error WAT")
}

// ── Policy allow tests ──────────────────────────────────────────────────

#[tokio::test]
async fn test_host_read_file_allowed() {
    ensure_v8();

    let tmp = tempfile::tempdir().unwrap();
    let test_file = tmp.path().join("hello.txt");
    std::fs::write(&test_file, "hello world").unwrap();

    let config = WasmHostConfig {
        policy_engine: Some(policy_engine(
            r#"
            package wasm.authz
            import rego.v1
            allow if { input.action == "fs_read" }
            "#,
        )),
    };

    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![WasmModule {
            name: "reader".to_string(),
            bytes: read_file_wat(),
        }])
        .with_wasm_host_config(config);

    // JS: write path into WASM memory, call do_read, decode result
    let code = format!(
        r#"
        var path = "{}";
        var mem = new Uint8Array(reader.memory.buffer);
        for (var i = 0; i < path.length; i++) mem[i] = path.charCodeAt(i);
        var resultPtr = reader.do_read(0, path.length);
        if (resultPtr === 0) {{
            "ERROR: got null ptr";
        }} else {{
            var view = new DataView(reader.memory.buffer);
            var len = view.getInt32(resultPtr, true);
            var bytes = new Uint8Array(reader.memory.buffer, resultPtr + 4, len);
            var result = "";
            for (var j = 0; j < bytes.length; j++) result += String.fromCharCode(bytes[j]);
            result;
        }}
        "#,
        test_file.display()
    );

    let result = engine
        .run_js(code, None, None, None, None)
        .await;
    assert!(result.is_ok(), "read should succeed, got: {:?}", result);
    assert_eq!(result.unwrap().output, "hello world");
}

// ── Policy deny tests ───────────────────────────────────────────────────

#[tokio::test]
async fn test_host_read_file_denied_by_policy() {
    ensure_v8();

    let tmp = tempfile::tempdir().unwrap();
    let test_file = tmp.path().join("secret.txt");
    std::fs::write(&test_file, "secret data").unwrap();

    // Policy only allows reads under /allowed (not /tmp)
    let config = WasmHostConfig {
        policy_engine: Some(policy_engine(
            r#"
            package wasm.authz
            import rego.v1
            allow if {
                input.action == "fs_read"
                startswith(input.path, "/allowed/")
            }
            "#,
        )),
    };

    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![WasmModule {
            name: "reader".to_string(),
            bytes: read_file_wat(),
        }])
        .with_wasm_host_config(config);

    let code = format!(
        r#"
        var path = "{}";
        var mem = new Uint8Array(reader.memory.buffer);
        for (var i = 0; i < path.length; i++) mem[i] = path.charCodeAt(i);
        reader.do_read(0, path.length);
        "#,
        test_file.display()
    );

    let result = engine.run_js(code, None, None, None, None).await;
    assert!(result.is_ok(), "should execute without crash: {:?}", result);
    // Returns 0 (null pointer) because policy denied the read
    assert_eq!(result.unwrap().output, "0");
}

// ── No policy = deny all ────────────────────────────────────────────────

#[tokio::test]
async fn test_host_read_file_no_policy_denied() {
    ensure_v8();

    // No policy configured — should deny all host calls.
    // But since has_imports() is false, the WASM module with imports
    // should fail to instantiate (no import object provided).
    // To test this properly, we need a policy but one that denies.
    let config = WasmHostConfig {
        policy_engine: Some(policy_engine(
            r#"
            package wasm.authz
            import rego.v1
            # No allow rule defined — everything denied
            "#,
        )),
    };

    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![WasmModule {
            name: "reader".to_string(),
            bytes: read_file_wat(),
        }])
        .with_wasm_host_config(config);

    let code = r#"
        var path = "/etc/passwd";
        var mem = new Uint8Array(reader.memory.buffer);
        for (var i = 0; i < path.length; i++) mem[i] = path.charCodeAt(i);
        reader.do_read(0, path.length);
    "#;

    let result = engine.run_js(code.to_string(), None, None, None, None).await;
    assert!(result.is_ok(), "should not crash: {:?}", result);
    assert_eq!(result.unwrap().output, "0"); // denied
}

// ── Write file tests ────────────────────────────────────────────────────

#[tokio::test]
async fn test_host_write_file_allowed() {
    ensure_v8();

    let tmp = tempfile::tempdir().unwrap();
    let out_file = tmp.path().join("output.txt");

    let config = WasmHostConfig {
        policy_engine: Some(policy_engine(
            r#"
            package wasm.authz
            import rego.v1
            allow if { input.action == "fs_write" }
            "#,
        )),
    };

    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![WasmModule {
            name: "writer".to_string(),
            bytes: write_file_wat(),
        }])
        .with_wasm_host_config(config);

    let code = format!(
        r#"
        var path = "{}";
        var data = "written by wasm";
        var mem = new Uint8Array(writer.memory.buffer);
        // Write path at offset 0
        for (var i = 0; i < path.length; i++) mem[i] = path.charCodeAt(i);
        // Write data at offset 512
        for (var j = 0; j < data.length; j++) mem[512 + j] = data.charCodeAt(j);
        writer.do_write(0, path.length, 512, data.length);
        "#,
        out_file.display()
    );

    let result = engine.run_js(code, None, None, None, None).await;
    assert!(result.is_ok(), "write should succeed: {:?}", result);
    assert_eq!(result.unwrap().output, "0"); // 0 = success

    let contents = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(contents, "written by wasm");
}

// ── Module-scoped policy tests ──────────────────────────────────────────

#[tokio::test]
async fn test_policy_module_name_check() {
    ensure_v8();

    let tmp = tempfile::tempdir().unwrap();
    let test_file = tmp.path().join("data.txt");
    std::fs::write(&test_file, "module-scoped").unwrap();

    // Only allow "trusted" module to read
    let config = WasmHostConfig {
        policy_engine: Some(policy_engine(
            r#"
            package wasm.authz
            import rego.v1
            allow if {
                input.action == "fs_read"
                input.module == "trusted"
            }
            "#,
        )),
    };

    // Module named "untrusted" — should be denied
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![WasmModule {
            name: "untrusted".to_string(),
            bytes: read_file_wat(),
        }])
        .with_wasm_host_config(config.clone());

    let code = format!(
        r#"
        var path = "{}";
        var mem = new Uint8Array(untrusted.memory.buffer);
        for (var i = 0; i < path.length; i++) mem[i] = path.charCodeAt(i);
        untrusted.do_read(0, path.length);
        "#,
        test_file.display()
    );

    let result = engine.run_js(code, None, None, None, None).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().output, "0"); // denied

    // Module named "trusted" — should be allowed
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![WasmModule {
            name: "trusted".to_string(),
            bytes: read_file_wat(),
        }])
        .with_wasm_host_config(config);

    let code = format!(
        r#"
        var path = "{}";
        var mem = new Uint8Array(trusted.memory.buffer);
        for (var i = 0; i < path.length; i++) mem[i] = path.charCodeAt(i);
        var ptr = trusted.do_read(0, path.length);
        if (ptr === 0) {{
            "DENIED";
        }} else {{
            var view = new DataView(trusted.memory.buffer);
            var len = view.getInt32(ptr, true);
            var bytes = new Uint8Array(trusted.memory.buffer, ptr + 4, len);
            var result = "";
            for (var j = 0; j < bytes.length; j++) result += String.fromCharCode(bytes[j]);
            result;
        }}
        "#,
        test_file.display()
    );

    let result = engine.run_js(code, None, None, None, None).await;
    assert!(result.is_ok(), "trusted read should succeed: {:?}", result);
    assert_eq!(result.unwrap().output, "module-scoped");
}

// ── host_last_error test ────────────────────────────────────────────────

#[tokio::test]
async fn test_host_last_error_after_denied_read() {
    ensure_v8();

    let config = WasmHostConfig {
        policy_engine: Some(policy_engine(
            r#"
            package wasm.authz
            import rego.v1
            # deny everything
            "#,
        )),
    };

    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![WasmModule {
            name: "errmod".to_string(),
            bytes: error_wat(),
        }])
        .with_wasm_host_config(config);

    let code = r#"
        var path = "/etc/passwd";
        var mem = new Uint8Array(errmod.memory.buffer);
        for (var i = 0; i < path.length; i++) mem[i] = path.charCodeAt(i);
        var readResult = errmod.do_read(0, path.length);
        var errPtr = errmod.get_error();
        if (errPtr === 0) {
            "no error";
        } else {
            var view = new DataView(errmod.memory.buffer);
            var len = view.getInt32(errPtr, true);
            var bytes = new Uint8Array(errmod.memory.buffer, errPtr + 4, len);
            var msg = "";
            for (var j = 0; j < bytes.length; j++) msg += String.fromCharCode(bytes[j]);
            msg;
        }
    "#;

    let result = engine
        .run_js(code.to_string(), None, None, None, None)
        .await;
    assert!(result.is_ok(), "should not crash: {:?}", result);
    let output = result.unwrap().output;
    assert!(
        output.contains("Policy denied"),
        "error should mention policy denial, got: {}",
        output
    );
}

// ── Zero-import modules still work alongside host-import modules ────────

#[tokio::test]
async fn test_no_imports_module_unaffected_by_config() {
    ensure_v8();

    // A plain WASM module (no imports) should work even when a policy is loaded.
    let add_bytes = wat::parse_str(
        r#"
        (module
            (func (export "add") (param i32 i32) (result i32)
                (i32.add (local.get 0) (local.get 1))
            )
        )
    "#,
    )
    .unwrap();

    let config = WasmHostConfig {
        policy_engine: Some(policy_engine(
            r#"
            package wasm.authz
            import rego.v1
            allow if { input.action == "fs_read" }
            "#,
        )),
    };

    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4)
        .with_wasm_modules(vec![WasmModule {
            name: "math".to_string(),
            bytes: add_bytes,
        }])
        .with_wasm_host_config(config);

    let result = engine
        .run_js("math.add(21, 21);".to_string(), None, None, None, None)
        .await;
    assert!(result.is_ok(), "plain module should work: {:?}", result);
    assert_eq!(result.unwrap().output, "42");
}
