/// Tests for ES module import support — verifies that `npm:`, `jsr:`, and
/// URL imports are resolved via the network module loader and executed.
///
/// Network-dependent tests are marked `#[ignore]` because they require
/// unrestricted HTTP access to esm.sh. Run them with:
///   cargo test --test module_imports -- --ignored

use std::sync::{Arc, Once};
use server::engine::{initialize_v8, Engine};
use server::engine::execution::ExecutionRegistry;
use server::engine::module_loader::ModuleLoaderConfig;

// ── Module specifier resolution unit tests ──────────────────────────────

#[test]
fn test_npm_specifier_resolves() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::new();
    let result = loader.resolve(
        "npm:cowsay@1.6.0",
        "file:///main.js",
        ResolutionKind::Import,
    );
    assert!(result.is_ok(), "npm specifier should resolve: {:?}", result);
    assert_eq!(result.unwrap().as_str(), "https://esm.sh/cowsay@1.6.0");
}

#[test]
fn test_jsr_specifier_resolves() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::new();
    let result = loader.resolve(
        "jsr:@luca/cases@1.0.0",
        "file:///main.js",
        ResolutionKind::Import,
    );
    assert!(result.is_ok(), "jsr specifier should resolve: {:?}", result);
    assert_eq!(
        result.unwrap().as_str(),
        "https://esm.sh/jsr/@luca/cases@1.0.0"
    );
}

#[test]
fn test_url_specifier_resolves() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::new();
    let result = loader.resolve(
        "https://deno.land/x/case/mod.ts",
        "file:///main.js",
        ResolutionKind::Import,
    );
    assert!(result.is_ok(), "URL specifier should resolve: {:?}", result);
    assert_eq!(
        result.unwrap().as_str(),
        "https://deno.land/x/case/mod.ts"
    );
}

#[test]
fn test_relative_specifier_resolves() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::new();
    let result = loader.resolve(
        "./utils.js",
        "https://esm.sh/cowsay@1.6.0/index.js",
        ResolutionKind::Import,
    );
    assert!(
        result.is_ok(),
        "Relative specifier should resolve: {:?}",
        result
    );
    assert_eq!(
        result.unwrap().as_str(),
        "https://esm.sh/cowsay@1.6.0/utils.js"
    );
}

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn create_test_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-module-test-{}-{}",
        std::process::id(),
        rand_id()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(16 * 1024 * 1024, 60, 4)
        .with_execution_registry(Arc::new(registry))
}

/// Create an engine with external modules explicitly allowed (for network-dependent tests).
fn create_test_engine_with_external_modules() -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-module-test-ext-{}-{}",
        std::process::id(),
        rand_id()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(16 * 1024 * 1024, 60, 4)
        .with_module_loader_config(ModuleLoaderConfig {
            allow_external: true,
            policy_chain: None,
            virtual_modules: None,
            virtual_commonjs_modules: None,
            virtual_files: None,
        })
        .with_execution_registry(Arc::new(registry))
}

/// Create an engine with external modules explicitly blocked.
fn create_test_engine_modules_blocked() -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-module-test-blocked-{}-{}",
        std::process::id(),
        rand_id()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(16 * 1024 * 1024, 60, 4)
        .with_module_loader_config(ModuleLoaderConfig {
            allow_external: false,
            policy_chain: None,
            virtual_modules: None,
            virtual_commonjs_modules: None,
            virtual_files: None,
        })
        .with_execution_registry(Arc::new(registry))
}

fn rand_id() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

async fn run_and_wait(engine: &Engine, code: &str) -> Result<String, String> {
    let exec_id = engine
        .run_js(code)
        .execution_timeout_secs(60)
        .execute()
        .await?;
    for _ in 0..1200 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return info.result.ok_or_else(|| "No result".to_string()),
                "failed" => {
                    return Err(info.error.unwrap_or_else(|| "Unknown error".to_string()));
                }
                "timed_out" => return Err("Timed out".to_string()),
                "cancelled" => return Err("Cancelled".to_string()),
                _ => continue,
            }
        }
    }
    Err("Execution did not complete within timeout".to_string())
}

// ── Top-level await execution ────────────────────────────────────────────

#[tokio::test]
async fn test_top_level_await_resolves() {
    ensure_v8();
    let engine = create_test_engine();

    let code = r#"
const result = await Promise.resolve(42);
console.log("got", result);
"#;

    let result = run_and_wait(&engine, code).await;
    assert!(result.is_ok(), "Top-level await should succeed: {:?}", result);
}

#[tokio::test]
async fn test_top_level_await_with_async_iife_also_works() {
    ensure_v8();
    let engine = create_test_engine();

    // The old workaround should still work
    let code = r#"
const result = await (async () => {
    return await Promise.resolve(99);
})();
console.log("got", result);
"#;

    let result = run_and_wait(&engine, code).await;
    assert!(result.is_ok(), "Top-level await with IIFE should succeed: {:?}", result);
}

// ── Plain JS unaffected ─────────────────────────────────────────────────

#[tokio::test]
async fn test_plain_js_unaffected_by_module_support() {
    ensure_v8();
    let engine = create_test_engine();

    let result = run_and_wait(&engine, "console.log(1 + 2);").await;
    assert!(result.is_ok(), "Plain JS should still work: {:?}", result);
}

#[tokio::test]
async fn test_plain_js_with_dynamic_import_keyword() {
    ensure_v8();
    let engine = create_test_engine();

    let result = run_and_wait(&engine, r#"const x = "import foo"; console.log(x);"#).await;
    assert!(result.is_ok(), "String with 'import' should work: {:?}", result);
}

// ── npm imports (network required) ──────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_npm_import_lodash_es() {
    ensure_v8();
    let engine = create_test_engine_with_external_modules();

    let code = r#"
import camelCase from "npm:lodash-es@4.17.21/camelCase";
console.log(camelCase("hello_world"));
"#;

    let result = run_and_wait(&engine, code).await;
    assert!(
        result.is_ok(),
        "npm lodash-es import should succeed, got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), "");
}

// ── jsr imports (network required) ──────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_jsr_import_cases() {
    ensure_v8();
    let engine = create_test_engine_with_external_modules();

    let code = r#"
import { camelCase } from "jsr:@luca/cases@1.0.0";
console.log(camelCase("hello_world"));
"#;

    let result = run_and_wait(&engine, code).await;
    assert!(
        result.is_ok(),
        "jsr @luca/cases import should succeed, got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), "");
}

// ── URL imports (network required) ──────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_url_import() {
    ensure_v8();
    let engine = create_test_engine_with_external_modules();

    let code = r#"
import { camelCase } from "https://esm.sh/jsr/@luca/cases@1.0.0";
console.log(camelCase("foo_bar"));
"#;

    let result = run_and_wait(&engine, code).await;
    assert!(
        result.is_ok(),
        "URL import should succeed, got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), "");
}

// ── Module with console output (network required) ───────────────────────

#[tokio::test]
#[ignore]
async fn test_module_console_log() {
    ensure_v8();
    let engine = create_test_engine_with_external_modules();

    let code = r#"
import camelCase from "npm:lodash-es@4.17.21/camelCase";
const result = camelCase("foo_bar_baz");
console.log("Result:", result);
"#;

    let exec_id = engine
        .run_js(code)
        .execution_timeout_secs(60)
        .execute()
        .await
        .expect("run_js should succeed");

    for _ in 0..1200 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            if info.status == "completed" {
                assert_eq!(info.result.as_deref(), Some(""));
                let output = engine
                    .get_execution_output(&exec_id, None, None, None, None)
                    .expect("should get output");
                assert!(
                    output.data.contains("fooBarBaz"),
                    "Console output should contain camelCased string, got: {}",
                    output.data
                );
                return;
            } else if info.status == "failed" || info.status == "timed_out" {
                panic!(
                    "Execution failed: {:?}",
                    info.error.unwrap_or_else(|| info.status.clone())
                );
            }
        }
    }
    panic!("Execution did not complete within timeout");
}

// ── npm cowsay (network required) ───────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_npm_cowsay() {
    ensure_v8();
    let engine = create_test_engine_with_external_modules();

    let code = r#"
import { say } from "npm:cowsay@1.6.0";
const result = say({ text: "Hello from mcp-js!" });
console.log(result);
"#;

    let exec_id = engine
        .run_js(code)
        .execution_timeout_secs(60)
        .execute()
        .await
        .expect("run_js should succeed");

    for _ in 0..1200 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            if info.status == "completed" {
                let output = engine
                    .get_execution_output(&exec_id, None, None, None, None)
                    .expect("should get output");
                assert!(
                    output.data.contains("Hello from mcp-js!"),
                    "Console output should contain 'Hello from mcp-js!', got: {}",
                    output.data
                );
                assert!(
                    output.data.contains("<") || output.data.contains("("),
                    "Cowsay output should contain cow art, got: {}",
                    output.data
                );
                return;
            } else if info.status == "failed" || info.status == "timed_out" {
                panic!(
                    "Execution failed: {:?}",
                    info.error.unwrap_or_else(|| info.status.clone())
                );
            }
        }
    }
    panic!("Execution did not complete within timeout");
}

// ── Deno-style URL import of TypeScript (network required) ──────────────

#[tokio::test]
#[ignore]
async fn test_url_import_typescript() {
    ensure_v8();
    let engine = create_test_engine_with_external_modules();

    let code = r#"
import { pascalCase } from "https://deno.land/x/case/mod.ts";
console.log(pascalCase("hello_world"));
"#;

    let exec_id = engine
        .run_js(code)
        .execution_timeout_secs(60)
        .execute()
        .await
        .expect("run_js should succeed");

    for _ in 0..1200 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            if info.status == "completed" {
                let output = engine
                    .get_execution_output(&exec_id, None, None, None, None)
                    .expect("should get output");
                assert!(
                    output.data.contains("HelloWorld"),
                    "Console output should contain 'HelloWorld', got: {}",
                    output.data
                );
                return;
            } else if info.status == "failed" || info.status == "timed_out" {
                panic!(
                    "Execution failed: {:?}",
                    info.error.unwrap_or_else(|| info.status.clone())
                );
            }
        }
    }
    panic!("Execution did not complete within timeout");
}

// ══════════════════════════════════════════════════════════════════════════
// External module blocking tests (no network required)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn test_data_url_source_preserves_query_characters() {
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let code = r#"
        const module = await import('data:text/javascript,export default "?"');
        if (module.default !== '?') {
            throw new Error(`unexpected data URL value: ${module.default}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .main_module_specifier("file:///main.mjs"),
    );
    assert!(result.is_ok(), "data URL source was truncated: {result:?}");
}

#[test]
fn test_data_url_unknown_format_uses_node_error_code() {
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let code = r#"
        try {
            await import('data:text/css,', { with: { type: 'css' } });
            throw new Error('import unexpectedly succeeded');
        } catch (error) {
            if (error.code !== 'ERR_UNKNOWN_MODULE_FORMAT') {
                throw new Error(`unexpected error code: ${error.code}: ${error.message}`);
            }
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .main_module_specifier("file:///main.mjs"),
    );
    assert!(result.is_ok(), "data URL import failed incorrectly: {result:?}");
}

#[test]
fn test_virtual_module_rejects_unsupported_type_attribute() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([(
        "file:///dep.js".to_string(),
        "export default 42;".to_string(),
    )]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: None,
        virtual_files: None,
    };
    let code = r#"
        try {
            await import('./dep.js', { with: { type: 'unsupported' } });
            throw new Error('import unexpectedly succeeded');
        } catch (error) {
            if (error.code !== 'ERR_IMPORT_ATTRIBUTE_UNSUPPORTED') {
                throw new Error(`unexpected error code: ${error.code}: ${error.message}`);
            }
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///main.mjs"),
    );
    assert!(result.is_ok(), "virtual module accepted invalid type: {result:?}");
}

#[test]
fn test_virtual_json_module_preserves_json_type() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([(
        "file:///data.json".to_string(),
        r#"{"value":42}"#.to_string(),
    )]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: None,
        virtual_files: None,
    };
    let code = r#"
        import data from './data.json' with { type: 'json' };
        if (data.value !== 42) throw new Error('wrong JSON module value');
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///main.mjs"),
    );
    assert!(result.is_ok(), "virtual JSON import failed: {result:?}");
}

#[test]
fn test_virtual_package_exports_import() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([
        (
            "file:///app/node_modules/example/package.json".to_string(),
            r#"{"exports":{"./feature":"./feature.js"}}"#.to_string(),
        ),
        (
            "file:///app/node_modules/example/feature.js".to_string(),
            "export default 42;".to_string(),
        ),
    ]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: None,
        virtual_files: None,
    };
    let code = r#"
        import value from 'example/feature';
        if (value !== 42) throw new Error(`wrong package export: ${value}`);
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "virtual package import failed: {result:?}");
}

#[test]
fn test_virtual_package_exports_reject_hidden_subpath() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([
        (
            "file:///app/node_modules/example/package.json".to_string(),
            r#"{"exports":"./index.js"}"#.to_string(),
        ),
        (
            "file:///app/node_modules/example/index.js".to_string(),
            "export default 42;".to_string(),
        ),
        (
            "file:///app/node_modules/example/hidden.js".to_string(),
            "export default 7;".to_string(),
        ),
    ]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: None,
        virtual_files: None,
    };
    let code = r#"
        let caught;
        try { await import('example/hidden.js'); } catch (error) { caught = error; }
        if (caught?.code !== 'ERR_PACKAGE_PATH_NOT_EXPORTED') {
            throw new Error(`wrong package error: ${caught?.code || 'resolved'}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "hidden package export was not rejected: {result:?}");
}

#[test]
fn test_virtual_unknown_extension_uses_node_error() {
    use std::collections::{HashMap, HashSet};
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(Arc::new(HashMap::new())),
        virtual_commonjs_modules: None,
        virtual_files: Some(Arc::new(HashSet::from([
            "file:///app/file.unknown".to_owned(),
        ]))),
    };
    let code = r#"
        let caught;
        try { await import('./file.unknown'); } catch (error) { caught = error; }
        if (caught?.code !== 'ERR_UNKNOWN_FILE_EXTENSION') {
            throw new Error(`wrong unknown-extension error: ${caught?.code || caught}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "unknown extension used the wrong error: {result:?}");
}

#[test]
fn test_virtual_missing_package_uses_node_error() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(Arc::new(HashMap::new())),
        virtual_commonjs_modules: None,
        virtual_files: None,
    };
    let code = r#"
        let caught;
        try { await import('nonexistent/file.mjs'); } catch (error) { caught = error; }
        if (caught?.code !== 'ERR_MODULE_NOT_FOUND') {
            throw new Error(`wrong missing-package error: ${caught?.code || caught}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "missing package used the wrong error: {result:?}");
}

#[test]
fn test_virtual_package_exports_reject_missing_target() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([(
        "file:///app/node_modules/example/package.json".to_string(),
        r#"{"exports":{"./missing":"./missing.js"}}"#.to_string(),
    )]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: None,
        virtual_files: None,
    };
    let code = r#"
        let caught;
        try { await import('example/missing'); } catch (error) { caught = error; }
        if (caught?.code !== 'ERR_MODULE_NOT_FOUND') {
            throw new Error(`wrong missing-target error: ${caught?.code || 'resolved'}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "missing package target was not rejected: {result:?}");
}

#[test]
fn test_concurrent_virtual_package_errors_reject_independently() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([
        (
            "file:///app/node_modules/first/package.json".to_string(),
            r#"{"exports":"./index.js"}"#.to_string(),
        ),
        (
            "file:///app/node_modules/first/index.js".to_string(),
            "export default 1;".to_string(),
        ),
        (
            "file:///app/node_modules/second/package.json".to_string(),
            r#"{"exports":"./index.js"}"#.to_string(),
        ),
        (
            "file:///app/node_modules/second/index.js".to_string(),
            "export default 2;".to_string(),
        ),
        (
            "file:///app/node_modules/third/package.json".to_string(),
            r#"{"exports":{"./missing":"./missing.js"}}"#.to_string(),
        ),
    ]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: None,
        virtual_files: None,
    };
    let code = r#"
        const results = await Promise.allSettled([
            import('first/hidden.js'),
            import('second/hidden.js'),
            import('third/missing'),
        ]);
        const fulfilled = results
            .map((result, index) => result.status === 'fulfilled' ? index : -1)
            .filter((index) => index >= 0);
        if (fulfilled.length) throw new Error(`package errors resolved: ${fulfilled}`);
        const codes = results.map((result) => result.reason?.code);
        if (codes.join(',') !==
            'ERR_PACKAGE_PATH_NOT_EXPORTED,ERR_PACKAGE_PATH_NOT_EXPORTED,ERR_MODULE_NOT_FOUND') {
            throw new Error(`wrong concurrent package errors: ${codes}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "concurrent package errors failed: {result:?}");
}

#[test]
fn test_create_require_enforces_package_exports() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([
        (
            "file:///app/node_modules/hidden/package.json".to_string(),
            r#"{"exports":"./index.cjs"}"#.to_string(),
        ),
        (
            "file:///app/node_modules/exact/package.json".to_string(),
            r#"{"exports":{"./no-ext":"./value"}}"#.to_string(),
        ),
    ]));
    let commonjs_modules = Arc::new(HashMap::from([
        (
            "file:///app/node_modules/hidden/index.cjs".to_string(),
            "module.exports = 42;".to_string(),
        ),
        (
            "file:///app/node_modules/hidden/private.cjs".to_string(),
            "module.exports = 7;".to_string(),
        ),
        (
            "file:///app/node_modules/exact/value.js".to_string(),
            "module.exports = 9;".to_string(),
        ),
    ]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: Some(commonjs_modules),
        virtual_files: None,
    };
    let code = r#"
        import { createRequire } from 'node:module';
        const require = createRequire(import.meta.url);
        for (const [specifier, expectedCode] of [
            ['hidden/private.cjs', 'ERR_PACKAGE_PATH_NOT_EXPORTED'],
            ['exact/no-ext', 'MODULE_NOT_FOUND'],
        ]) {
            let error;
            try { require(specifier); } catch (caught) { error = caught; }
            if (error?.code !== expectedCode) {
                throw new Error(`${specifier}: expected ${expectedCode}, got ${error?.code || 'resolved'}`);
            }
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "createRequire exports enforcement failed: {result:?}");
}

#[test]
fn test_create_require_resolves_package_imports() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([
        (
            "file:///app/package.json".to_string(),
            r##"{"imports":{"#test":"./test.js","#branch":{"require":"./require.js","default":"./default.js"},"#sub/*":"./src/*.js","#external":"dep/value"}}"##.to_string(),
        ),
        (
            "file:///app/node_modules/dep/package.json".to_string(),
            r#"{"exports":{"./value":"./value.js"}}"#.to_string(),
        ),
    ]));
    let commonjs_modules = Arc::new(HashMap::from([
        ("file:///app/test.js".to_string(), "module.exports = 'test';".to_string()),
        ("file:///app/require.js".to_string(), "module.exports = 'require';".to_string()),
        ("file:///app/src/item.js".to_string(), "module.exports = 'item';".to_string()),
        ("file:///app/node_modules/dep/value.js".to_string(), "module.exports = 'external';".to_string()),
    ]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: Some(commonjs_modules),
        virtual_files: None,
    };
    let code = r#"
        import { createRequire } from 'node:module';
        const require = createRequire(import.meta.url);
        const values = [require('#test'), require('#branch'), require('#sub/item'), require('#external')];
        if (values.join(',') !== 'test,require,item,external') {
            throw new Error(`wrong package imports: ${values}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "package imports require failed: {result:?}");
}

#[test]
fn test_package_import_deprecation_uses_node_test_flags() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([(
        "file:///app/package.json".to_string(),
        r##"{"imports":{"#double":"./sub//missing.js"}}"##.to_string(),
    )]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: Some(Arc::new(HashMap::new())),
        virtual_files: None,
    };
    let code = r#"
        import { createRequire } from 'node:module';
        import process from 'node:process';
        globalThis.__NODE_TEST_FLAGS__ = ['--pending-deprecation'];
        const warnings = [];
        process.removeAllListeners('warning');
        process.on('warning', (warning) => warnings.push(warning));
        const require = createRequire(import.meta.url);
        try { require('#double'); } catch (error) {
            if (error?.code !== 'MODULE_NOT_FOUND') throw error;
        }
        await new Promise((resolve) => setTimeout(resolve, 0));
        if (warnings.length !== 1 || warnings[0].code !== 'DEP0166' ||
            !warnings[0].stack.includes('./sub//missing.js')) {
            throw new Error(`missing package import deprecation warning: ${warnings}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "package imports warning failed: {result:?}");
}


#[test]
fn test_dynamic_import_legacy_package_main_warnings() {
    use std::collections::HashMap;
    use server::engine::{ExecutionConfig, execute_stateless};

    ensure_v8();
    let modules = Arc::new(HashMap::from([
        (
            "file:///app/node_modules/no_exports/package.json".to_string(),
            r#"{"type":"module"}"#.to_string(),
        ),
        (
            "file:///app/node_modules/default_index/package.json".to_string(),
            r#"{"main":"index","type":"module"}"#.to_string(),
        ),
    ]));
    let commonjs_modules = Arc::new(HashMap::from([
        (
            "file:///app/node_modules/no_exports/index.js".to_string(),
            "module.exports = 'index';".to_string(),
        ),
        (
            "file:///app/node_modules/default_index/index.js".to_string(),
            "module.exports = 'main';".to_string(),
        ),
    ]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: Some(commonjs_modules),
        virtual_files: None,
    };
    let code = r#"
        import process from 'node:process';
        await import('node:module');
        const warnings = [];
        process.removeAllListeners('warning');
        process.on('warning', (warning) => warnings.push(warning));
        await globalThis.__mcpV8ImportVirtualModule('no_exports', import.meta.url);
        await globalThis.__mcpV8ImportVirtualModule('default_index', import.meta.url);
        await new Promise((resolve) => setTimeout(resolve, 0));
        if (warnings.length !== 2 || warnings.some((warning) => warning.code !== 'DEP0151') ||
            !warnings[0].stack.includes('no_exports') ||
            !warnings[1].stack.includes('default_index')) {
            throw new Error(`unexpected package warnings: ${warnings.map((w) => w.stack)}`);
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "legacy package warnings failed: {result:?}");
}

#[test]
fn test_create_require_resolves_hash_prefixed_legacy_package() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let commonjs_modules = Arc::new(HashMap::from([(
        "file:///app/node_modules/%23cjs/index.js".to_string(),
        "module.exports = 'cjs backcompat';".to_string(),
    )]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(Arc::new(HashMap::new())),
        virtual_commonjs_modules: Some(commonjs_modules),
        virtual_files: None,
    };
    let code = r#"
        import { createRequire } from 'node:module';
        const require = createRequire(import.meta.url);
        const value = require('#cjs');
        if (value !== 'cjs backcompat') throw new Error(`wrong legacy package value: ${value}`);
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "hash-prefixed package require failed: {result:?}");
}

#[test]
fn test_create_require_virtual_package_exports() {
    use std::collections::HashMap;
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([(
        "file:///app/node_modules/example/package.json".to_string(),
        r#"{"exports":{"./feature":"./feature.cjs"}}"#.to_string(),
    )]));
    let commonjs_modules = Arc::new(HashMap::from([(
        "file:///app/node_modules/example/feature.cjs".to_string(),
        "module.exports = 42;".to_string(),
    )]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: Some(commonjs_modules),
        virtual_files: None,
    };
    let code = r#"
        import { createRequire } from 'node:module';
        const require = createRequire(import.meta.url);
        const value = require('example/feature');
        if (value !== 42) throw new Error(`wrong required package export: ${value}`);
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "virtual package require failed: {result:?}");
}


#[test]
fn test_internal_legacy_main_resolve_uses_virtual_files() {
    use std::collections::{HashMap, HashSet};
    use server::engine::{execute_stateless, ExecutionConfig};

    ensure_v8();
    let modules = Arc::new(HashMap::from([
        (
            "file:///fixture/package.json".to_string(),
            r#"{"main":"./index-js/index"}"#.to_string(),
        ),
        (
            "file:///fixture/index-json/package.json".to_string(),
            r#"{}"#.to_string(),
        ),
    ]));
    let files = Arc::new(HashSet::from([
        "file:///fixture/package.json".to_string(),
        "file:///fixture/index-js/index.js".to_string(),
        "file:///fixture/index-json/package.json".to_string(),
        "file:///fixture/index-json/index.json".to_string(),
        "file:///fixture/index-node/index.node".to_string(),
        "file:///folder%2525with%20percentage%23/index.js".to_string(),
    ]));
    let loader = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: Some(modules),
        virtual_commonjs_modules: Some(Arc::new(HashMap::new())),
        virtual_files: Some(files),
    };
    let code = r#"
        import { createRequire } from 'node:module';
        const require = createRequire(import.meta.url);
        const { legacyMainResolve } = require('node:internal/modules/esm/resolve');
        const packageJsonUrl = new URL('file:///fixture/package.json');
        if ('__mcpV8VirtualFiles' in globalThis) {
            throw new Error('virtual file inventory must not be exposed globally');
        }

        const fromMain = legacyMainResolve(
            packageJsonUrl,
            { main: './index-js/index' },
            '/fixture',
        );
        if (fromMain.href !== 'file:///fixture/index-js/index.js') {
            throw new Error(`wrong main resolution: ${fromMain.href}`);
        }

        const fromIndex = legacyMainResolve(
            new URL('file:///fixture/index-json/package.json'),
            { main: undefined },
            '/fixture',
        );
        if (fromIndex.href !== 'file:///fixture/index-json/index.json') {
            throw new Error(`wrong index resolution: ${fromIndex.href}`);
        }

        const nativeAddon = legacyMainResolve(
            packageJsonUrl,
            { main: './index-node/index' },
            '/fixture',
        );
        if (nativeAddon.href !== 'file:///fixture/index-node/index.node') {
            throw new Error(`wrong native addon resolution: ${nativeAddon.href}`);
        }

        const special = legacyMainResolve(
            packageJsonUrl,
            { main: '../folder%25with percentage#/' },
            packageJsonUrl,
        );
        if (special.href !== 'file:///folder%2525with%20percentage%23/index.js') {
            throw new Error(`wrong special path resolution: ${special.href}`);
        }

        let packageConfigError;
        try { legacyMainResolve(packageJsonUrl, undefined, packageJsonUrl); }
        catch (caught) { packageConfigError = caught; }
        if (!(packageConfigError instanceof TypeError) || packageConfigError.code) {
            throw new Error(`expected native TypeError for missing packageConfig, got ${packageConfigError?.code}: ${packageConfigError}`);
        }

        for (const [invoke, code, message] of [
            [() => legacyMainResolve('/fixture/package.json', {}, ''), 'ERR_INTERNAL_ASSERTION'],
            [() => legacyMainResolve(packageJsonUrl, { main: './missing.node' }, packageJsonUrl), 'ERR_MODULE_NOT_FOUND', /missing\.node/],
            [() => legacyMainResolve(packageJsonUrl, { main: null }, undefined), 'ERR_INVALID_ARG_TYPE', /"base" argument must be/],
        ]) {
            let error;
            try { invoke(); } catch (caught) { error = caught; }
            if (error?.code !== code || (message && !message.test(error.message))) {
                throw new Error(`expected ${code}, got ${error?.code}: ${error?.message}`);
            }
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .module_loader_config(&loader)
            .main_module_specifier("file:///app/main.mjs"),
    );
    assert!(result.is_ok(), "legacyMainResolve compatibility failed: {result:?}");
}

#[test]
fn test_resolve_npm_blocked_when_external_disabled() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::with_config(ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: None,
        virtual_commonjs_modules: None,
        virtual_files: None,
    });
    let result = loader.resolve("npm:lodash-es@4.17.21", "file:///main.js", ResolutionKind::Import);
    assert!(result.is_err(), "npm specifier should be rejected when external modules disabled");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("External module imports are disabled"),
        "Error should mention disabled imports, got: {}",
        err
    );
}

#[test]
fn test_resolve_jsr_blocked_when_external_disabled() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::with_config(ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: None,
        virtual_commonjs_modules: None,
        virtual_files: None,
    });
    let result = loader.resolve("jsr:@luca/cases@1.0.0", "file:///main.js", ResolutionKind::Import);
    assert!(result.is_err(), "jsr specifier should be rejected when external modules disabled");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("External module imports are disabled"), "got: {}", err);
}

#[test]
fn test_resolve_url_blocked_when_external_disabled() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::with_config(ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: None,
        virtual_commonjs_modules: None,
        virtual_files: None,
    });
    let result = loader.resolve(
        "https://esm.sh/jsr/@luca/cases@1.0.0",
        "file:///main.js",
        ResolutionKind::Import,
    );
    assert!(result.is_err(), "URL specifier should be rejected when external modules disabled");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("External module imports are disabled"), "got: {}", err);
}

#[test]
fn test_resolve_relative_allowed_when_external_disabled() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::with_config(ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
        virtual_modules: None,
        virtual_commonjs_modules: None,
        virtual_files: None,
    });
    let result = loader.resolve(
        "./utils.js",
        "https://esm.sh/cowsay@1.6.0/index.js",
        ResolutionKind::Import,
    );
    assert!(result.is_ok(), "Relative specifier should resolve even when external disabled: {:?}", result);
}

#[test]
fn test_resolve_npm_allowed_when_external_enabled() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::with_config(ModuleLoaderConfig {
        allow_external: true,
        policy_chain: None,
        virtual_modules: None,
        virtual_commonjs_modules: None,
        virtual_files: None,
    });
    let result = loader.resolve("npm:lodash-es@4.17.21", "file:///main.js", ResolutionKind::Import);
    assert!(result.is_ok(), "npm specifier should resolve when external enabled: {:?}", result);
    assert_eq!(result.unwrap().as_str(), "https://esm.sh/lodash-es@4.17.21");
}

// ══════════════════════════════════════════════════════════════════════════
// Engine-level blocking tests (no network required)
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_engine_blocks_npm_import_by_default() {
    ensure_v8();
    let engine = create_test_engine_modules_blocked();

    let code = r#"import { camelCase } from "npm:lodash-es@4.17.21";
camelCase("hello_world");"#;

    let result = run_and_wait(&engine, code).await;
    assert!(result.is_err(), "npm import should fail when external modules blocked");
    let err = result.unwrap_err();
    assert!(
        err.contains("External module imports are disabled"),
        "Error should mention disabled imports, got: {}",
        err
    );
}

#[tokio::test]
async fn test_engine_blocks_jsr_import_by_default() {
    ensure_v8();
    let engine = create_test_engine_modules_blocked();

    let code = r#"import { camelCase } from "jsr:@luca/cases@1.0.0";
camelCase("hello_world");"#;

    let result = run_and_wait(&engine, code).await;
    assert!(result.is_err(), "jsr import should fail when external modules blocked");
    let err = result.unwrap_err();
    assert!(err.contains("External module imports are disabled"), "got: {}", err);
}

#[tokio::test]
async fn test_engine_blocks_url_import_by_default() {
    ensure_v8();
    let engine = create_test_engine_modules_blocked();

    let code = r#"import { camelCase } from "https://esm.sh/jsr/@luca/cases@1.0.0";
camelCase("hello_world");"#;

    let result = run_and_wait(&engine, code).await;
    assert!(result.is_err(), "URL import should fail when external modules blocked");
    let err = result.unwrap_err();
    assert!(err.contains("External module imports are disabled"), "got: {}", err);
}

#[tokio::test]
async fn test_engine_plain_js_works_when_modules_blocked() {
    ensure_v8();
    let engine = create_test_engine_modules_blocked();

    let result = run_and_wait(&engine, "console.log(1 + 2);").await;
    assert!(result.is_ok(), "Plain JS should work when external modules blocked: {:?}", result);
}

#[tokio::test]
async fn test_default_engine_blocks_external_modules() {
    ensure_v8();
    let engine = create_test_engine(); // uses default (blocked)

    let code = r#"import { camelCase } from "npm:lodash-es@4.17.21";
camelCase("hello_world");"#;

    let result = run_and_wait(&engine, code).await;
    assert!(result.is_err(), "Default engine should block external modules");
    let err = result.unwrap_err();
    assert!(err.contains("External module imports are disabled"), "got: {}", err);
}
