/// Regression tests for binary response bodies.
///
/// A body that is not valid UTF-8 must survive `fetch()` byte for byte. The
/// failure this guards against is silent: decoding to a string replaces every
/// invalid sequence with U+FFFD, which changes the length but leaves the
/// leading magic bytes intact, so a corrupted wasm or font still looks valid
/// and only fails later at use.

use std::sync::{Arc, Once};

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use server::engine::execution::ExecutionRegistry;
use server::engine::fetch::FetchConfig;
use server::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};
use server::engine::{initialize_v8, Engine};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

/// Every byte value 0x00..=0xFF exactly once. 0xC0, 0xF5..0xFF and lone
/// continuation bytes are all invalid UTF-8, so any text round-trip mangles
/// this and changes its length.
fn all_byte_values() -> Vec<u8> {
    (0u16..=255).map(|value| value as u8).collect()
}

async fn binary_handler() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    (StatusCode::OK, headers, all_byte_values())
}

async fn spawn_binary_server() -> String {
    let app = Router::new().route("/binary", get(binary_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{}/binary", address)
}

fn allow_all_chain() -> Arc<PolicyChain> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("allow_all.rego");
    std::fs::write(&path, "package mcp.fetch\ndefault allow = true\n").unwrap();
    std::mem::forget(dir);

    let evaluator =
        LocalPolicyEvaluator::from_file(&path, "data.mcp.fetch.allow".to_string()).unwrap();
    Arc::new(PolicyChain::new(
        vec![PolicyEvaluatorKind::Local(evaluator)],
        EvalMode::All,
    ))
}

fn build_engine() -> Engine {
    let fetch_config = FetchConfig::new_with_chain(allow_all_chain());
    let tmp = std::env::temp_dir().join(format!(
        "mcp-fetch-binary-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");

    Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_fetch_config(fetch_config)
        .with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: String) -> serde_json::Value {
    let mut args = serde_json::Map::new();
    args.insert("code".into(), serde_json::Value::String(code));
    server::mcp_dispatch::run_js_blocking(engine, None, &serde_json::Value::Object(args)).await
}

fn output(value: &serde_json::Value) -> String {
    assert!(value["error"].is_null(), "Should not have error: {:?}", value);
    value["output"]
        .as_str()
        .expect("Should have output field")
        .trim()
        .to_string()
}

/// blob() must preserve the body length. This is the regression: building the
/// Blob from decodeText() inflated 256 bytes to 388, because each of the 132
/// invalid sequences became a 3-byte U+FFFD.
#[tokio::test]
async fn blob_preserves_binary_body_length() {
    ensure_v8();
    let url = spawn_binary_server().await;
    let engine = build_engine();

    let result = run_js(
        &engine,
        format!(
            r#"const r = await fetch("{url}");
               const b = await r.blob();
               console.log(String(b.size));"#
        ),
    )
    .await;

    assert_eq!(
        output(&result),
        "256",
        "blob() changed the body length, so the bytes were decoded as text"
    );
}

/// The bytes themselves must be identical, not merely the same count.
#[tokio::test]
async fn blob_preserves_binary_body_contents() {
    ensure_v8();
    let url = spawn_binary_server().await;
    let engine = build_engine();

    let result = run_js(
        &engine,
        format!(
            r#"const r = await fetch("{url}");
               const b = await r.blob();
               const bytes = new Uint8Array(await b.arrayBuffer());
               let mismatched = 0;
               for (let i = 0; i < 256; i++) {{
                   if (bytes[i] !== i) mismatched++;
               }}
               console.log(String(mismatched));"#
        ),
    )
    .await;

    assert_eq!(
        output(&result),
        "0",
        "blob() returned different bytes than the server sent"
    );
}

/// arrayBuffer() and bytes() were already lossless; pin them so a future change
/// to the shared decode path cannot regress them unnoticed.
#[tokio::test]
async fn array_buffer_and_bytes_preserve_binary_body() {
    ensure_v8();
    let url = spawn_binary_server().await;
    let engine = build_engine();

    let result = run_js(
        &engine,
        format!(
            r#"const viaArrayBuffer = new Uint8Array(await (await fetch("{url}")).arrayBuffer());
               const viaBytes = await (await fetch("{url}")).bytes();
               console.log(JSON.stringify({{
                   arrayBuffer: viaArrayBuffer.length,
                   bytes: viaBytes.length,
                   firstByte: viaBytes[0],
                   lastByte: viaBytes[255],
               }}));"#
        ),
    )
    .await;

    assert_eq!(
        output(&result),
        r#"{"arrayBuffer":256,"bytes":256,"firstByte":0,"lastByte":255}"#
    );
}

/// text() legitimately lossy for binary — that is what TextDecoder does — but a
/// UTF-8 body must still round-trip exactly, so the fix must not touch it.
#[tokio::test]
async fn text_still_round_trips_utf8() {
    ensure_v8();

    let app = Router::new().route(
        "/text",
        get(|| async { (StatusCode::OK, "héllo wörld ✓").into_response() }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("http://{}/text", address);

    let engine = build_engine();
    let result = run_js(
        &engine,
        format!(r#"console.log(await (await fetch("{url}")).text());"#),
    )
    .await;

    assert_eq!(output(&result), "héllo wörld ✓");
}
