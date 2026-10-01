//! Tests for the keyed artifact store: the `artifact(key, mime, bytes)` JS
//! global, per-execution artifact metadata, the `get_artifact`/`list_artifacts`
//! dispatch tools (image/* rendered as MCP image content), and the raw-bytes
//! REST endpoints.

use std::sync::{Arc, Once};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use reqwest::Client;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, sleep, timeout};

use server::engine::artifacts::ArtifactContent;
use server::engine::execution::ExecutionRegistry;
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn rand_id() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

/// Create a stateless engine with an execution registry.
fn create_test_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-artifacts-test-{}-{}",
        std::process::id(),
        rand_id()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(8 * 1024 * 1024, 30, 4).with_execution_registry(Arc::new(registry))
}

/// Run stateless run_js via the shared dispatcher, returning the full
/// `ToolResponse` (JSON body + rendered artifact content blocks).
async fn run_js(engine: &Engine, code: &str) -> server::mcp_dispatch::ToolResponse {
    let args = json!({ "code": code, "execution_timeout_secs": 30 });
    server::mcp_dispatch::run_js_blocking(engine, None, &args).await
}

/// PNG magic prefix plus a few extra bytes — deliberately not valid UTF-8.
const PNG_BYTES: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x01, 0x02];
const PNG_BYTES_JS: &str =
    "new Uint8Array([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x01, 0x02])";

// ── Dispatch-level tests ─────────────────────────────────────────────────

/// An image artifact emitted by stateless run_js is attached inline as an
/// Image content block (base64 data + mime type) with metadata in the JSON.
#[tokio::test]
async fn test_image_artifact_inline_on_stateless_run_js() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = run_js(
        &engine,
        &format!(r#"artifact("chart", "image/png", {PNG_BYTES_JS});"#),
    )
    .await;

    assert!(resp.json["error"].is_null(), "run failed: {:?}", resp.json);
    let metas = resp.json["artifacts"]
        .as_array()
        .expect("artifacts metadata list");
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0]["key"], "chart");
    assert_eq!(metas[0]["mime_type"], "image/png");
    assert_eq!(metas[0]["size_bytes"], PNG_BYTES.len() as u64);
    assert_eq!(metas[0]["inline"], true);

    assert_eq!(resp.artifacts.len(), 1);
    match &resp.artifacts[0] {
        ArtifactContent::Image {
            data_base64,
            mime_type,
        } => {
            assert_eq!(mime_type, "image/png");
            assert_eq!(data_base64, &BASE64.encode(PNG_BYTES));
        }
        other => panic!("expected Image content, got {:?}", other),
    }
}

/// get_artifact returns image/* as an Image block, UTF-8 as Text, other
/// binary as Base64 — with metadata (including encoding) in the JSON body.
#[tokio::test]
async fn test_get_artifact_rendering_by_mime() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = run_js(
        &engine,
        &format!(
            r#"
            artifact("img", "image/png", {PNG_BYTES_JS});
            artifact("csv", "text/csv", "a,b\n1,2\n");
            artifact("bin", "application/octet-stream", new Uint8Array([0xff, 0xfe, 0x00]));
            "#
        ),
    )
    .await;
    assert!(resp.json["error"].is_null(), "run failed: {:?}", resp.json);

    let img = server::mcp_dispatch::get_artifact(&engine, &json!({ "key": "img" }));
    assert_eq!(img.json["mime_type"], "image/png");
    assert_eq!(img.json["encoding"], "image");
    assert_eq!(img.json["size_bytes"], PNG_BYTES.len() as u64);
    assert!(img.json["execution_id"].is_string());
    assert!(matches!(&img.artifacts[0], ArtifactContent::Image { .. }));

    let csv = server::mcp_dispatch::get_artifact(&engine, &json!({ "key": "csv" }));
    assert_eq!(csv.json["encoding"], "utf-8");
    match &csv.artifacts[0] {
        ArtifactContent::Text(text) => assert_eq!(text, "a,b\n1,2\n"),
        other => panic!("expected Text content, got {:?}", other),
    }

    let bin = server::mcp_dispatch::get_artifact(&engine, &json!({ "key": "bin" }));
    assert_eq!(bin.json["encoding"], "base64");
    match &bin.artifacts[0] {
        ArtifactContent::Base64(data) => assert_eq!(data, &BASE64.encode([0xffu8, 0xfe, 0x00])),
        other => panic!("expected Base64 content, got {:?}", other),
    }
}

/// Unknown keys produce a JSON error and no content blocks.
#[tokio::test]
async fn test_get_artifact_unknown_key() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = server::mcp_dispatch::get_artifact(&engine, &json!({ "key": "nope" }));
    assert!(resp.json["error"].as_str().unwrap().contains("not found"));
    assert!(resp.artifacts.is_empty());
}

/// The same key overwrites; artifacts persist across executions and
/// list_artifacts reflects the latest state.
#[tokio::test]
async fn test_artifact_overwrite_and_persistence_across_executions() {
    ensure_v8();
    let engine = create_test_engine();

    let first = run_js(&engine, r#"artifact("report", "text/plain", "v1");"#).await;
    assert!(
        first.json["error"].is_null(),
        "run failed: {:?}",
        first.json
    );
    let second = run_js(&engine, r#"artifact("report", "text/plain", "v2");"#).await;
    assert!(
        second.json["error"].is_null(),
        "run failed: {:?}",
        second.json
    );

    let got = server::mcp_dispatch::get_artifact(&engine, &json!({ "key": "report" }));
    match &got.artifacts[0] {
        ArtifactContent::Text(text) => assert_eq!(text, "v2"),
        other => panic!("expected Text content, got {:?}", other),
    }

    let listed = server::mcp_dispatch::list_artifacts(&engine);
    let artifacts = listed["artifacts"].as_array().unwrap();
    assert_eq!(
        artifacts.len(),
        1,
        "same key should overwrite, not duplicate"
    );
    assert_eq!(artifacts[0]["key"], "report");
}

/// String payloads are UTF-8 encoded; ArrayBuffer payloads are accepted.
#[tokio::test]
async fn test_artifact_input_types() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = run_js(
        &engine,
        r#"
        artifact("str", "text/plain", "héllo");
        artifact("buf", "application/wasm", new Uint8Array([1, 2, 3]).buffer);
        "#,
    )
    .await;
    assert!(resp.json["error"].is_null(), "run failed: {:?}", resp.json);

    let s = server::mcp_dispatch::get_artifact(&engine, &json!({ "key": "str" }));
    match &s.artifacts[0] {
        ArtifactContent::Text(text) => assert_eq!(text, "héllo"),
        other => panic!("expected Text content, got {:?}", other),
    }
    let b = server::mcp_dispatch::get_artifact(&engine, &json!({ "key": "buf" }));
    assert_eq!(b.json["size_bytes"], 3);
}

/// Invalid arguments throw a catchable JS error and store nothing.
#[tokio::test]
async fn test_artifact_validation_errors_in_js() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = run_js(
        &engine,
        r#"
        let errors = [];
        try { artifact("", "image/png", new Uint8Array([1])); } catch (e) { errors.push(String(e)); }
        try { artifact("k", "png", new Uint8Array([1])); } catch (e) { errors.push(String(e)); }
        try { artifact("k", "image/png", 42); } catch (e) { errors.push(String(e)); }
        console.log(JSON.stringify(errors));
        "#,
    )
    .await;
    assert!(resp.json["error"].is_null(), "run failed: {:?}", resp.json);
    let output = resp.json["output"].as_str().unwrap();
    let errors: Vec<String> = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(
        errors.len(),
        3,
        "all three calls should throw: {:?}",
        errors
    );
    assert!(errors.iter().all(|e| e.contains("artifact")));

    let listed = server::mcp_dispatch::list_artifacts(&engine);
    assert!(listed["artifacts"].as_array().unwrap().is_empty());
}

/// Artifacts written before a script failure are still recorded and fetchable.
#[tokio::test]
async fn test_artifacts_recorded_on_failed_execution() {
    ensure_v8();
    let engine = create_test_engine();
    let resp = run_js(
        &engine,
        r#"
        artifact("partial", "text/plain", "saved before crash");
        throw new Error("boom");
        "#,
    )
    .await;
    assert!(resp.json["error"].as_str().unwrap_or("").contains("boom"));
    let metas = resp.json["artifacts"]
        .as_array()
        .expect("artifacts metadata list");
    assert_eq!(metas[0]["key"], "partial");

    let got = server::mcp_dispatch::get_artifact(&engine, &json!({ "key": "partial" }));
    match &got.artifacts[0] {
        ArtifactContent::Text(text) => assert_eq!(text, "saved before crash"),
        other => panic!("expected Text content, got {:?}", other),
    }
}

// ── Stateful (heap snapshot) path ────────────────────────────────────────

/// Poll an execution to a terminal state and return its info.
async fn wait_for_execution(engine: &Engine, id: &str) -> server::engine::execution::ExecutionInfo {
    for _ in 0..600 {
        sleep(Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(id.to_string()) {
            if info.status != "running" {
                return info;
            }
        }
    }
    panic!("Execution {} did not finish", id);
}

/// `artifact()` works on a fresh stateful runtime AND on a restored heap
/// snapshot — the restore path skips JS injection (the global is baked into
/// the snapshot) while the op is re-registered per runtime.
#[tokio::test]
async fn test_artifact_on_fresh_and_restored_heap_snapshot() {
    ensure_v8();
    let heap_dir = std::env::temp_dir().join(format!(
        "mcp-artifacts-heap-{}-{}",
        std::process::id(),
        rand_id()
    ));
    let heap_storage = server::engine::heap_storage::AnyHeapStorage::File(
        server::engine::heap_storage::FileHeapStorage::new(heap_dir.to_str().unwrap()),
    );
    let tmp = std::env::temp_dir().join(format!(
        "mcp-artifacts-stateful-{}-{}",
        std::process::id(),
        rand_id()
    ));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("registry");
    let engine = Engine::new_stateful(heap_storage, None, None, 64 * 1024 * 1024, 30, 4)
        .with_execution_registry(Arc::new(registry));

    // Fresh runtime: the artifact wrapper is injected and snapshotted.
    let exec1 = engine
        .run_js(r#"artifact("stateful-1", "text/plain", "first");"#)
        .execute()
        .await
        .expect("submit run 1");
    let info1 = wait_for_execution(&engine, &exec1).await;
    assert_eq!(info1.status, "completed", "run 1 failed: {:?}", info1.error);
    assert_eq!(info1.artifacts.len(), 1);
    assert_eq!(info1.artifacts[0].key, "stateful-1");
    let heap = info1.heap.expect("stateful run should produce a heap hash");

    // Restored runtime: injection is skipped; the global comes from the
    // snapshot and must still reach the freshly registered op.
    let exec2 = engine
        .run_js(r#"artifact("stateful-2", "image/png", new Uint8Array([0x89, 0x50]));"#)
        .heap(&heap)
        .execute()
        .await
        .expect("submit run 2");
    let info2 = wait_for_execution(&engine, &exec2).await;
    assert_eq!(info2.status, "completed", "run 2 failed: {:?}", info2.error);
    assert_eq!(info2.artifacts.len(), 1);
    assert_eq!(info2.artifacts[0].key, "stateful-2");
    assert_eq!(info2.artifacts[0].mime_type, "image/png");

    let one = engine
        .get_artifact("stateful-1")
        .expect("stateful-1 stored");
    assert_eq!(one.bytes, b"first");
    let two = engine
        .get_artifact("stateful-2")
        .expect("stateful-2 stored");
    assert_eq!(two.bytes, &[0x89, 0x50]);
    assert!(matches!(two.content(), ArtifactContent::Image { .. }));
}

// ── REST e2e ─────────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<tokio::process::Child>,
    base_url: String,
}

impl HttpServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")?
            .local_addr()?
            .port();
        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(&["--http-port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        let base_url = format!("http://127.0.0.1:{}", port);

        let client = Client::new();
        let health_url = format!("{}/api/executions", base_url);
        for _ in 0..150 {
            if client
                .get(&health_url)
                .timeout(Duration::from_millis(100))
                .send()
                .await
                .is_ok()
            {
                return Ok(HttpServer {
                    child: Some(child),
                    base_url,
                });
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err("Server did not become ready within 15s".into())
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }
}

/// Full REST round trip: run_js emits an image artifact; the execution lists
/// it; `/api/artifacts/{key}` serves the raw bytes with the stored mime type.
#[tokio::test]
async fn test_artifact_rest_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();

    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .json(&json!({ "code": format!(r#"artifact("rest-img", "image/png", {PNG_BYTES_JS});"#) }))
        .send()
        .await?;
    assert_eq!(resp.status(), 202);
    let body: Value = resp.json().await?;
    let id = body["execution_id"].as_str().unwrap().to_string();

    // Poll to terminal status and check the artifacts metadata on the record.
    let exec = timeout(Duration::from_secs(10), async {
        loop {
            let body: Value = client
                .get(format!("{}/api/executions/{}", server.base_url, id))
                .send()
                .await
                .expect("GET execution failed")
                .json()
                .await
                .expect("Invalid JSON");
            if body["status"] != "running" {
                return body;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    assert_eq!(exec["status"], "completed", "execution failed: {:?}", exec);
    assert_eq!(exec["artifacts"][0]["key"], "rest-img");
    assert_eq!(exec["artifacts"][0]["mime_type"], "image/png");

    // Raw bytes with the stored mime type as Content-Type.
    let resp = client
        .get(format!("{}/api/artifacts/rest-img", server.base_url))
        .send()
        .await?;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap().to_str()?,
        "image/png"
    );
    assert_eq!(resp.bytes().await?.as_ref(), PNG_BYTES);

    // Metadata list.
    let listed: Value = client
        .get(format!("{}/api/artifacts", server.base_url))
        .send()
        .await?
        .json()
        .await?;
    let keys: Vec<&str> = listed["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["key"].as_str())
        .collect();
    assert!(keys.contains(&"rest-img"));

    // Unknown key → 404 JSON error.
    let resp = client
        .get(format!("{}/api/artifacts/no-such-key", server.base_url))
        .send()
        .await?;
    assert_eq!(resp.status(), 404);

    server.stop().await;
    Ok(())
}

/// The canonical invocation path used by both MCP transports must retain media,
/// while the string-returning UniFFI API retains its JSON metadata contract.
#[tokio::test]
async fn test_canonical_invocation_preserves_artifact_content() {
    use server::engine::ToolCallRequest;
    ensure_v8();
    let engine = create_test_engine();
    let request = |name: &str, args: Value| ToolCallRequest {
        name: name.to_string(),
        arguments_json: args.to_string(),
        session_id: None,
        mcp_headers: None,
    };
    let response = engine
        .invoke_tool_response(request(
            "run_js",
            json!({
                "code": format!(r#"artifact("canonical", "image/png", {PNG_BYTES_JS});"#),
            }),
        ))
        .await
        .unwrap();
    assert!(response.json["error"].is_null(), "{:?}", response.json);
    assert!(matches!(
        response.artifacts.first(),
        Some(ArtifactContent::Image { .. })
    ));
    let fetched = engine
        .invoke_tool_response(request("get_artifact", json!({"key": "canonical"})))
        .await
        .unwrap();
    assert!(matches!(
        fetched.artifacts.first(),
        Some(ArtifactContent::Image { .. })
    ));
    let listed: Value = serde_json::from_str(
        &engine
            .invoke_tool(request("list_artifacts", json!({})))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(listed["artifacts"][0]["key"], "canonical");
    engine.shutdown().await;
    assert!(
        engine
            .invoke_tool_response(request("get_artifact", json!({"key": "canonical"})))
            .await
            .is_err()
    );
}

// ── Uploads and JS read-back ─────────────────────────────────────────────

/// `PUT /api/artifacts/{key}` stores the raw body with the request's
/// Content-Type (parameters dropped); a script then reads it with
/// artifact.get and GET serves the same bytes back.
#[tokio::test]
async fn test_artifact_rest_upload() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = HttpServer::start().await?;
    let client = Client::new();
    let url = format!("{}/api/artifacts/rest-upload", server.base_url);

    let resp = client
        .put(&url)
        .header("content-type", "application/pdf; name=form.pdf")
        .body(PNG_BYTES.to_vec())
        .send()
        .await?;
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await?;
    assert_eq!(meta["key"], "rest-upload");
    assert_eq!(meta["mime_type"], "application/pdf");
    assert_eq!(meta["size_bytes"], PNG_BYTES.len() as u64);

    let resp = client.get(&url).send().await?;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap().to_str()?,
        "application/pdf"
    );
    assert_eq!(resp.bytes().await?.as_ref(), PNG_BYTES);

    // A script sees the upload.
    let resp = client
        .post(format!("{}/api/exec", server.base_url))
        .json(&json!({ "code": r#"console.log(artifact.get("rest-upload").bytes.length);"# }))
        .send()
        .await?;
    let id = resp.json::<Value>().await?["execution_id"]
        .as_str()
        .unwrap()
        .to_string();
    let output = timeout(Duration::from_secs(10), async {
        loop {
            let exec: Value = client
                .get(format!("{}/api/executions/{}", server.base_url, id))
                .send()
                .await
                .expect("GET execution failed")
                .json()
                .await
                .expect("Invalid JSON");
            if exec["status"] != "running" {
                assert_eq!(exec["status"], "completed", "execution failed: {:?}", exec);
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        client
            .get(format!("{}/api/executions/{}/output", server.base_url, id))
            .send()
            .await
            .expect("GET output failed")
            .json::<Value>()
            .await
            .expect("Invalid JSON")
    })
    .await?;
    assert_eq!(output["data"].as_str().unwrap().trim(), PNG_BYTES.len().to_string());

    // No Content-Type → application/octet-stream; over the cap → rejected.
    let resp = client.put(&url).body(vec![1u8, 2, 3]).send().await?;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<Value>().await?["mime_type"], "application/octet-stream");

    let resp = client
        .put(&url)
        .body(vec![0u8; 16 * 1024 * 1024 + 1])
        .send()
        .await?;
    assert_eq!(resp.status(), 400);

    server.stop().await;
    Ok(())
}

// ── One-time upload URLs ─────────────────────────────────────────────────

/// Serve only the upload router for `engine` on an ephemeral port.
async fn serve_upload_router(engine: Arc<Engine>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = server::api::artifact_upload_router(engine);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

/// get_artifact_upload_url hands out a URL; PUTting raw bytes to it stores
/// the artifact for scripts to read, and the URL then stops working.
#[tokio::test]
async fn test_upload_url_roundtrip_is_single_use() {
    ensure_v8();
    let engine = Arc::new(create_test_engine());
    let base = serve_upload_router(engine.clone()).await;
    let client = Client::new();

    let issued = server::mcp_dispatch::get_artifact_upload_url(
        &engine,
        &json!({ "key": "form.pdf", "mime_type": "application/pdf" }),
    );
    assert!(issued["error"].is_null(), "issue failed: {:?}", issued);
    assert_eq!(issued["method"], "PUT");
    assert_eq!(issued["key"], "form.pdf");
    assert_eq!(issued["max_bytes"], 16 * 1024 * 1024);
    // No --public-url on this engine: path only, with a note.
    assert!(issued["url"].is_null());
    assert!(issued["note"].is_string());
    let url = format!("{}{}", base, issued["path"].as_str().unwrap());

    // The issue-time mime type wins over the request's Content-Type.
    let resp = client
        .put(&url)
        .header("content-type", "text/plain")
        .body(PNG_BYTES.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    assert_eq!(meta["key"], "form.pdf");
    assert_eq!(meta["mime_type"], "application/pdf");
    assert_eq!(meta["size_bytes"], PNG_BYTES.len() as u64);

    let resp = run_js(
        &engine,
        r#"
        const f = artifact.get("form.pdf");
        if (!(f.bytes instanceof Uint8Array)) throw new Error("bytes is not a Uint8Array");
        if (artifact.get("nope") !== null) throw new Error("missing key should be null");
        if (artifact.list().map((a) => a.key).join() !== "form.pdf") throw new Error("list");
        console.log(f.mime_type, Array.from(f.bytes).join(","));
        "#,
    )
    .await;
    assert!(resp.json["error"].is_null(), "run failed: {:?}", resp.json);
    // Reading an upload doesn't make it an artifact this execution emitted.
    assert!(resp.json["artifacts"].is_null());
    let expected = format!(
        "application/pdf {}",
        PNG_BYTES.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")
    );
    assert_eq!(resp.json["output"].as_str().unwrap().trim(), expected);

    // Second use of the same URL is refused and changes nothing.
    let resp = client.put(&url).body(vec![9u8]).send().await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(engine.get_artifact("form.pdf").unwrap().bytes, PNG_BYTES);
}

/// Without an issue-time mime type the upload's Content-Type is used; bogus
/// tokens are 404; an oversized body is 413 and leaves the URL usable; and a
/// configured public URL yields an absolute `url`.
#[tokio::test]
async fn test_upload_url_mime_fallback_limits_and_public_url() {
    ensure_v8();
    let engine = Arc::new(create_test_engine().with_public_url("https://mcp.example.com/".into()));
    let base = serve_upload_router(engine.clone()).await;
    let client = Client::new();

    let issued =
        server::mcp_dispatch::get_artifact_upload_url(&engine, &json!({ "key": "data.csv" }));
    let path = issued["path"].as_str().unwrap();
    assert_eq!(issued["url"], format!("https://mcp.example.com{path}"));
    assert!(issued["upload_example"].as_str().unwrap().contains("curl"));
    assert!(issued["note"].is_null());
    let url = format!("{base}{path}");

    let resp = client
        .put(format!("{base}/api/artifact-uploads/{}", "0".repeat(64)))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let resp = client
        .put(&url)
        .body(vec![0u8; 16 * 1024 * 1024 + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413);
    assert!(engine.get_artifact("data.csv").is_err());

    let resp = client
        .put(&url)
        .header("content-type", "text/csv; charset=utf-8")
        .body("a,b\n")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<Value>().await.unwrap()["mime_type"], "text/csv");

    // Invalid requests fail at issue time, before anything is uploaded.
    let bad = server::mcp_dispatch::get_artifact_upload_url(&engine, &json!({ "key": "" }));
    assert!(bad["error"].is_string());
    let bad = server::mcp_dispatch::get_artifact_upload_url(
        &engine,
        &json!({ "key": "k", "mime_type": "nope" }),
    );
    assert!(bad["error"].is_string());
}
