/// Tests for ES module import support — verifies that `npm:`, `jsr:`, and
/// URL imports are resolved via the network module loader and executed.
///
/// Network-dependent tests are marked `#[ignore]` because they require
/// unrestricted HTTP access to esm.sh. Run them with:
///   cargo test --test module_imports -- --ignored

use std::sync::{Arc, Once};
use server::engine::{initialize_v8, has_module_syntax, Engine};
use server::engine::execution::ExecutionRegistry;
use server::engine::module_loader::ModuleLoaderConfig;
use server::engine::opa::OpaClient;

// ── has_module_syntax unit tests ────────────────────────────────────────

#[test]
fn test_detect_import_declaration() {
    assert!(has_module_syntax(r#"import { foo } from "bar";"#));
    assert!(has_module_syntax(r#"import foo from "bar";"#));
    assert!(has_module_syntax(r#"import "side-effect";"#));
    assert!(has_module_syntax(r#"import{foo}from"bar";"#));
}

#[test]
fn test_detect_export_declaration() {
    assert!(has_module_syntax("export const foo = 1;"));
    assert!(has_module_syntax("export default function() {}"));
    assert!(has_module_syntax(r#"export { foo } from "bar";"#));
    assert!(has_module_syntax(r#"export* from "bar";"#));
}

#[test]
fn test_no_module_syntax_in_plain_js() {
    assert!(!has_module_syntax("const x = 1 + 2;"));
    assert!(!has_module_syntax("function foo() { return 42; }"));
    assert!(!has_module_syntax(r#"const s = "import is a keyword";"#));
}

#[test]
fn test_dynamic_import_not_detected() {
    // dynamic import() is an expression, not a declaration
    assert!(!has_module_syntax(r#"import("./foo.js");"#));
    assert!(!has_module_syntax(r#"const m = import("./foo.js");"#));
}

#[test]
fn test_npm_specifier_detected() {
    assert!(has_module_syntax(
        r#"import { camelCase } from "npm:lodash-es@4.17.21";"#
    ));
}

#[test]
fn test_jsr_specifier_detected() {
    assert!(has_module_syntax(
        r#"import { camelCase } from "jsr:@luca/cases@1.0.0";"#
    ));
}

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
            opa_client: None,
            opa_module_policy: None,
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
            opa_client: None,
            opa_module_policy: None,
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
        .run_js(code.to_string(), None, None, None, Some(60), None)
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

// ── Plain JS unaffected ─────────────────────────────────────────────────

#[tokio::test]
async fn test_plain_js_unaffected_by_module_support() {
    ensure_v8();
    let engine = create_test_engine();

    let result = run_and_wait(&engine, "1 + 2;").await;
    assert!(result.is_ok(), "Plain JS should still work: {:?}", result);
    assert_eq!(result.unwrap(), "3");
}

#[tokio::test]
async fn test_plain_js_with_dynamic_import_keyword() {
    // dynamic import() is an expression, not module syntax
    ensure_v8();
    let engine = create_test_engine();

    // The word "import" in a string should not trigger module mode.
    let result = run_and_wait(&engine, r#"const x = "import foo"; x;"#).await;
    assert!(result.is_ok(), "String with 'import' should work: {:?}", result);
    assert_eq!(result.unwrap(), "import foo");
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
    assert_eq!(result.unwrap(), "undefined");
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
    assert_eq!(result.unwrap(), "undefined");
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
    assert_eq!(result.unwrap(), "undefined");
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
        .run_js(code.to_string(), None, None, None, Some(60), None)
        .await
        .expect("run_js should succeed");

    for _ in 0..1200 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            if info.status == "completed" {
                assert_eq!(info.result.as_deref(), Some("undefined"));
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
        .run_js(code.to_string(), None, None, None, Some(60), None)
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
        .run_js(code.to_string(), None, None, None, Some(60), None)
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
fn test_resolve_npm_blocked_when_external_disabled() {
    use deno_core::ResolutionKind;
    use server::engine::module_loader::NetworkModuleLoader;
    use deno_core::ModuleLoader;

    let loader = NetworkModuleLoader::with_config(ModuleLoaderConfig {
        allow_external: false,
        opa_client: None,
        opa_module_policy: None,
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
        opa_client: None,
        opa_module_policy: None,
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
        opa_client: None,
        opa_module_policy: None,
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
        opa_client: None,
        opa_module_policy: None,
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
        opa_client: None,
        opa_module_policy: None,
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

    let result = run_and_wait(&engine, "1 + 2;").await;
    assert!(result.is_ok(), "Plain JS should work when external modules blocked: {:?}", result);
    assert_eq!(result.unwrap(), "3");
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
