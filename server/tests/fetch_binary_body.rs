/// Regression tests for binary body handling in `Blob`, `fetch().blob()` and
/// `FormData`.
///
/// The payload is every byte value `0x00..=0xFF` exactly once. Most of those
/// are not valid UTF-8 on their own, so a body that round-trips through
/// `TextDecoder` comes back as 158 bytes instead of 256 — this runtime's
/// decoder is lenient and swallows stray continuation bytes, where a spec
/// decoder would inflate them to U+FFFD instead. Either way the length moves,
/// which makes the corruption deterministic and detectable without a network
/// call or a real binary asset.

use std::sync::{Arc, Once};

use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
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

/// Every byte value once, in order.
fn all_bytes() -> Vec<u8> {
    (0u8..=255u8).collect()
}

// ── Test server ─────────────────────────────────────────────────────────────

#[derive(Clone)]
struct BinaryServer {
    base_url: String,
    uploads: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
}

impl BinaryServer {
    fn binary_url(&self) -> String {
        format!("{}/binary", self.base_url)
    }

    fn text_url(&self) -> String {
        format!("{}/text", self.base_url)
    }

    fn upload_url(&self) -> String {
        format!("{}/upload", self.base_url)
    }

    async fn uploads(&self) -> Vec<Vec<u8>> {
        self.uploads.lock().await.clone()
    }
}

/// A body that is valid UTF-8 and uses multi-byte sequences, so `text()`
/// regressions show up as mojibake rather than as an unchanged ASCII string.
const UTF8_BODY: &str = "héllo → 世界 🌍";

async fn start_binary_server() -> BinaryServer {
    async fn binary_handler() -> impl IntoResponse {
        (
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            all_bytes(),
        )
    }

    async fn text_handler() -> impl IntoResponse {
        (
            StatusCode::OK,
            [("content-type", "text/plain; charset=utf-8")],
            UTF8_BODY,
        )
    }

    async fn upload_handler(
        axum::extract::State(uploads): axum::extract::State<Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>>,
        _headers: HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        uploads.lock().await.push(body.to_vec());
        (StatusCode::OK, "ok")
    }

    let uploads: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let app = Router::new()
        .route("/binary", get(binary_handler))
        .route("/text", get(text_handler))
        .route(
            "/upload",
            post(upload_handler).with_state(Arc::clone(&uploads)),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    BinaryServer {
        base_url: format!("http://{}", address),
        uploads,
    }
}

// ── Engine harness ──────────────────────────────────────────────────────────

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
        .with_fetch_config(FetchConfig::new_with_chain(allow_all_chain()))
        .with_execution_registry(Arc::new(registry))
}

/// Run JS through the shared dispatcher, which reports console output.
async fn run_js(engine: &Engine, code: String) -> serde_json::Value {
    let mut args = serde_json::Map::new();
    args.insert("code".into(), serde_json::Value::String(code));
    args.insert("execution_timeout_secs".into(), serde_json::json!(30));
    server::mcp_dispatch::run_js_blocking(engine, None, &serde_json::Value::Object(args)).await
}

/// Run an async JS expression and return what it resolved to, by logging the
/// value and reading it back out of the captured console output.
async fn eval(engine: &Engine, code: String) -> String {
    let wrapped = format!("Promise.resolve({code}).then(function(v) {{ console.log(v); }});");
    let resp = run_js(engine, wrapped).await;
    assert!(resp["error"].is_null(), "execution failed: {resp:?}");
    resp["output"]
        .as_str()
        .expect("dispatcher should return an output field")
        .to_string()
}

// ── fetch().blob() ──────────────────────────────────────────────────────────

#[tokio::test]
async fn blob_preserves_binary_body_length() {
    ensure_v8();
    let server = start_binary_server().await;
    let engine = build_engine();

    let out = eval(
        &engine,
        format!(
            r#"
            (async () => {{
                const resp = await fetch("{url}");
                const blob = await resp.blob();
                return String(blob.size);
            }})()
            "#,
            url = server.binary_url()
        ),
    )
    .await;

    assert_eq!(
        out.trim(),
        "256",
        "blob() changed the body length, so the bytes were decoded as text"
    );
}

#[tokio::test]
async fn blob_preserves_binary_body_contents() {
    ensure_v8();
    let server = start_binary_server().await;
    let engine = build_engine();

    // Length alone can be preserved by accident; compare byte for byte.
    let out = eval(
        &engine,
        format!(
            r#"
            (async () => {{
                const resp = await fetch("{url}");
                const bytes = new Uint8Array(await (await resp.blob()).arrayBuffer());
                if (bytes.length !== 256) return "length " + bytes.length;
                for (let i = 0; i < 256; i++) {{
                    if (bytes[i] !== i) return "byte " + i + " is " + bytes[i];
                }}
                return "IDENTICAL";
            }})()
            "#,
            url = server.binary_url()
        ),
    )
    .await;

    assert!(
        out.contains("IDENTICAL"),
        "blob() did not round-trip the body: {out}"
    );
}

#[tokio::test]
async fn blob_type_comes_from_content_type() {
    ensure_v8();
    let server = start_binary_server().await;
    let engine = build_engine();

    let out = eval(
        &engine,
        format!(
            r#"
            (async () => {{
                const resp = await fetch("{url}");
                return (await resp.blob()).type;
            }})()
            "#,
            url = server.binary_url()
        ),
    )
    .await;

    assert!(
        out.contains("application/octet-stream"),
        "blob().type should mirror content-type, got: {out}"
    );
}

/// `arrayBuffer()` and `bytes()` were already lossless — pin them so they
/// cannot regress alongside a future change to `blob()`.
#[tokio::test]
async fn array_buffer_and_bytes_preserve_binary_body() {
    ensure_v8();
    let server = start_binary_server().await;
    let engine = build_engine();

    let out = eval(
        &engine,
        format!(
            r#"
            (async () => {{
                const viaBuffer = new Uint8Array(await (await fetch("{url}")).arrayBuffer());
                const viaBytes = await (await fetch("{url}")).bytes();
                if (viaBuffer.length !== 256) return "arrayBuffer length " + viaBuffer.length;
                if (viaBytes.length !== 256) return "bytes length " + viaBytes.length;
                for (let i = 0; i < 256; i++) {{
                    if (viaBuffer[i] !== i) return "arrayBuffer byte " + i;
                    if (viaBytes[i] !== i) return "bytes byte " + i;
                }}
                return "IDENTICAL";
            }})()
            "#,
            url = server.binary_url()
        ),
    )
    .await;

    assert!(out.contains("IDENTICAL"), "binary accessor regressed: {out}");
}

/// The binary fix must not disturb the text path.
#[tokio::test]
async fn text_and_blob_text_still_round_trip_utf8() {
    ensure_v8();
    let server = start_binary_server().await;
    let engine = build_engine();

    let out = eval(
        &engine,
        format!(
            r#"
            (async () => {{
                const expected = {expected};
                const viaText = await (await fetch("{url}")).text();
                const viaBlob = await (await (await fetch("{url}")).blob()).text();
                if (viaText !== expected) return "text: " + viaText;
                if (viaBlob !== expected) return "blob.text: " + viaBlob;
                return "UTF8_OK";
            }})()
            "#,
            url = server.text_url(),
            expected = serde_json::to_string(UTF8_BODY).unwrap(),
        ),
    )
    .await;

    assert!(out.contains("UTF8_OK"), "UTF-8 text round-trip broke: {out}");
}

// ── Blob constructor ────────────────────────────────────────────────────────

/// `new Blob([bytes])` used to fall through to `String(part)` and yield the
/// literal ASCII `"[object Uint8Array]"`, whatever the input was.
#[tokio::test]
async fn blob_constructor_accepts_buffer_source() {
    ensure_v8();
    let engine = build_engine();

    let out = eval(
        &engine,
        r#"
        (async () => {
            const bytes = new Uint8Array(256);
            for (let i = 0; i < 256; i++) bytes[i] = i;

            const cases = {
                uint8array: new Blob([bytes]),
                arraybuffer: new Blob([bytes.buffer]),
                dataview: new Blob([new DataView(bytes.buffer)]),
                subarray: new Blob([bytes.subarray(0, 128), bytes.subarray(128)]),
                nestedBlob: new Blob([new Blob([bytes])]),
            };

            for (const [name, blob] of Object.entries(cases)) {
                if (blob.size !== 256) return name + " size " + blob.size;
                const out = new Uint8Array(await blob.arrayBuffer());
                for (let i = 0; i < 256; i++) {
                    if (out[i] !== i) return name + " byte " + i + " is " + out[i];
                }
            }
            return "ALL_LOSSLESS";
        })()
        "#
        .to_string(),
    )
    .await;

    assert!(
        out.contains("ALL_LOSSLESS"),
        "Blob constructor lost binary parts: {out}"
    );
}

#[tokio::test]
async fn blob_string_parts_are_utf8_encoded() {
    ensure_v8();
    let engine = build_engine();

    // "é" is one UTF-16 code unit but two UTF-8 bytes; Blob measures bytes.
    let out = eval(
        &engine,
        r#"
        (async () => {
            const b = new Blob(['é']);
            const bytes = new Uint8Array(await b.arrayBuffer());
            return b.size + "|" + bytes[0] + "," + bytes[1] + "|" + (await b.text());
        })()
        "#
        .to_string(),
    )
    .await;

    assert!(
        out.contains("2|195,169|é"),
        "Blob should hold UTF-8 bytes for string parts: {out}"
    );
}

#[tokio::test]
async fn blob_slice_and_bytes_operate_on_bytes() {
    ensure_v8();
    let engine = build_engine();

    let out = eval(
        &engine,
        r#"
        (async () => {
            const bytes = new Uint8Array(256);
            for (let i = 0; i < 256; i++) bytes[i] = i;
            const sliced = new Blob([bytes], { type: 'application/octet-stream' }).slice(250, 256);
            if (sliced.size !== 6) return "size " + sliced.size;
            if (sliced.type !== 'application/octet-stream') return "type " + sliced.type;
            const out = await sliced.bytes();
            for (let i = 0; i < 6; i++) {
                if (out[i] !== 250 + i) return "byte " + i + " is " + out[i];
            }
            return "SLICE_OK";
        })()
        "#
        .to_string(),
    )
    .await;

    assert!(out.contains("SLICE_OK"), "Blob.slice regressed: {out}");
}

// ── FormData ────────────────────────────────────────────────────────────────

/// `FormData._serialize` splices file parts in from `Blob._data`, so a binary
/// upload is corrupted by any UTF-8 encode on the way to the wire.
#[tokio::test]
async fn formdata_uploads_binary_file_part_intact() {
    ensure_v8();
    let server = start_binary_server().await;
    let engine = build_engine();

    eval(
        &engine,
        format!(
            r#"
            (async () => {{
                const bytes = new Uint8Array(256);
                for (let i = 0; i < 256; i++) bytes[i] = i;
                const fd = new FormData();
                fd.append('field', 'value');
                fd.append('f', new File([bytes], 'payload.bin', {{ type: 'application/octet-stream' }}));
                const resp = await fetch("{url}", {{ method: 'POST', body: fd }});
                return String(resp.status);
            }})()
            "#,
            url = server.upload_url()
        ),
    )
    .await;

    let uploads = server.uploads().await;
    assert_eq!(uploads.len(), 1, "expected exactly one upload");
    let body = &uploads[0];

    let expected = all_bytes();
    assert!(
        body
            .windows(expected.len())
            .any(|window| window == expected.as_slice()),
        "the 256-byte file part did not survive multipart serialization \
         (body was {} bytes)",
        body.len()
    );
    assert!(
        !body.windows(19).any(|w| w == b"[object Uint8Array]"),
        "the file part was stringified instead of sent as bytes"
    );
    assert!(
        body.windows(5).any(|w| w == b"value"),
        "the plain text field did not survive multipart serialization"
    );
}

