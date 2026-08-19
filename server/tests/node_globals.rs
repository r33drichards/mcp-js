use std::sync::{Arc, Once};

use axum::{Router, routing::get};
use server::engine::execution::ExecutionRegistry;
use server::engine::module_loader::ModuleLoaderConfig;
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(initialize_v8);
}

fn create_test_engine(node_globals: bool, allow_external: bool) -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-node-globals-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("create execution registry");
    Engine::new_stateless(32 * 1024 * 1024, 60, 4)
        .with_node_globals(node_globals)
        .with_module_loader_config(ModuleLoaderConfig {
            allow_external,
            policy_chain: None,
        })
        .with_execution_registry(Arc::new(registry))
}

fn create_stateful_test_engine(node_globals: bool) -> Engine {
    let root = std::env::temp_dir().join(format!(
        "mcp-node-globals-heap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let heap_storage = server::engine::heap_storage::AnyHeapStorage::File(
        server::engine::heap_storage::FileHeapStorage::new(&root),
    );
    let registry_path = root.join("executions");
    let registry = ExecutionRegistry::new(registry_path.to_str().unwrap())
        .expect("create execution registry");
    Engine::new_stateful(heap_storage, None, None, 32 * 1024 * 1024, 60, 1)
        .with_node_globals(node_globals)
        .with_execution_registry(Arc::new(registry))
}

async fn run_and_read(engine: &Engine, code: &str) -> Result<String, String> {
    let execution_id = engine
        .run_js(code)
        .execution_timeout_secs(60)
        .execute()
        .await?;

    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let info = engine.get_execution(&execution_id)?;
        match info.status.as_str() {
            "completed" => {
                return engine
                    .get_execution_output(&execution_id, None, None, None, None)
                    .map(|output| output.data);
            }
            "failed" | "timed_out" | "cancelled" => {
                return Err(info.error.unwrap_or(info.status));
            }
            _ => {}
        }
    }

    Err("execution did not complete".to_string())
}

#[tokio::test]
async fn node_globals_are_disabled_by_default() {
    ensure_v8();
    let engine = create_test_engine(false, false);
    let output = run_and_read(
        &engine,
        r#"console.log(JSON.stringify({ buffer: typeof Buffer, process: typeof process }));"#,
    )
    .await
    .expect("execution succeeds");

    assert!(output.contains(r#"{"buffer":"undefined","process":"undefined"}"#));
}

#[tokio::test]
async fn node_globals_reuse_the_builtin_compatibility_values() {
    ensure_v8();
    let engine = create_test_engine(true, false);
    let output = run_and_read(
        &engine,
        r#"
import { Buffer as ImportedBuffer } from 'node:buffer';
import importedProcess from 'node:process';
console.log(JSON.stringify({
  bufferType: typeof Buffer,
  processType: typeof process,
  sameBuffer: Buffer === ImportedBuffer,
  sameProcess: process === importedProcess,
  envKeys: Object.keys(process.env).length,
}));
"#,
    )
    .await
    .expect("execution succeeds");

    assert!(output.contains(
        r#"{"bufferType":"function","processType":"object","sameBuffer":true,"sameProcess":true,"envKeys":0}"#
    ));
}

#[tokio::test]
async fn node_globals_are_enabled_for_stateful_execution() {
    ensure_v8();
    let engine = create_stateful_test_engine(true);
    let output = run_and_read(
        &engine,
        r#"console.log(JSON.stringify({ buffer: typeof Buffer, process: typeof process }));"#,
    )
    .await
    .expect("stateful execution succeeds");

    assert!(output.contains(r#"{"buffer":"function","process":"object"}"#));
}

#[tokio::test]
async fn node_globals_exist_before_static_dependencies_evaluate() {
    ensure_v8();
    let app = Router::new().route(
        "/dependency.js",
        get(|| async {
            (
                [("content-type", "application/javascript")],
                r#"
if (typeof Buffer === 'undefined' || typeof process === 'undefined') {
  throw new Error('node globals missing during dependency evaluation');
}
export const ready = Buffer.from('ready').toString() === 'ready' && process.env !== undefined;
"#,
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind module server");
    let address = listener.local_addr().expect("module server address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve module");
    });

    let engine = create_test_engine(true, true);
    let code = format!(
        "import {{ ready }} from 'http://{address}/dependency.js'; console.log(ready);"
    );
    let output = run_and_read(&engine, &code)
        .await
        .expect("static dependency sees globals");
    server.abort();

    assert!(output.contains("true"));
}
