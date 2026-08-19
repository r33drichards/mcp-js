/// Integration tests for the policy-gated HTTP/2 ops and the `node:http2`
/// shim — the transport for stock gRPC clients.
///
/// Uses an in-process `h2::server` that speaks gRPC-style framing (response
/// headers → DATA echo → trailers with grpc-status), plus local Rego
/// policies. JS runs through the real Engine and imports `node:http2` via
/// the module loader; thrown errors fail the execution.

use std::collections::HashMap;
use std::sync::{Arc, Once};

use bytes::Bytes;
use server::engine::execution::ExecutionRegistry;
use server::engine::fetch::HeaderRule;
use server::engine::http2::Http2Config;
use server::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

#[derive(Clone, Default)]
struct GrpcServerState {
    /// Request headers per accepted stream, name → value.
    requests: Arc<tokio::sync::Mutex<Vec<HashMap<String, String>>>>,
}

struct GrpcServer {
    addr: String,
    state: GrpcServerState,
}

impl GrpcServer {
    async fn requests(&self) -> Vec<HashMap<String, String>> {
        self.state.requests.lock().await.clone()
    }
}

/// gRPC-flavored h2 test server:
/// - `/echo.Service/Echo` — 200 + content-type, echoes DATA, trailers
///   `grpc-status: 0`, `grpc-message: ok`, echoes back `x-echo-meta` request
///   metadata as `x-echoed-meta` in the response headers.
/// - `/trailers.Only/Call` — trailers-only response: single HEADERS frame
///   with `grpc-status: 12`, end_stream.
/// - `/reset.Service/Refuse` — RST_STREAM with REFUSED_STREAM.
async fn start_grpc_server() -> GrpcServer {
    let state = GrpcServerState::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let accept_state = state.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                break;
            };
            let state = accept_state.clone();
            tokio::spawn(async move {
                let Ok(mut conn) = h2::server::handshake(tcp).await else {
                    return;
                };
                while let Some(Ok((request, mut respond))) = conn.accept().await {
                    let state = state.clone();
                    tokio::spawn(async move {
                        let mut headers: HashMap<String, String> = request
                            .headers()
                            .iter()
                            .map(|(k, v)| {
                                (
                                    k.as_str().to_string(),
                                    String::from_utf8_lossy(v.as_bytes()).to_string(),
                                )
                            })
                            .collect();
                        headers.insert(":path".into(), request.uri().path().to_string());
                        headers.insert(":method".into(), request.method().to_string());
                        let echo_meta = headers.get("x-echo-meta").cloned();
                        let path = request.uri().path().to_string();
                        state.requests.lock().await.push(headers);

                        match path.as_str() {
                            "/reset.Service/Refuse" => {
                                respond.send_reset(h2::Reason::REFUSED_STREAM);
                            }
                            "/trailers.Only/Call" => {
                                let response = http::Response::builder()
                                    .status(200)
                                    .header("content-type", "application/grpc")
                                    .header("grpc-status", "12")
                                    .header("grpc-message", "unimplemented")
                                    .body(())
                                    .unwrap();
                                let _ = respond.send_response(response, true);
                            }
                            _ => {
                                let mut body = request.into_body();
                                let mut data: Vec<u8> = Vec::new();
                                while let Some(chunk) =
                                    futures::future::poll_fn(|cx| body.poll_data(cx)).await
                                {
                                    let Ok(chunk) = chunk else { return };
                                    let _ =
                                        body.flow_control().release_capacity(chunk.len());
                                    data.extend_from_slice(&chunk);
                                }

                                let mut response = http::Response::builder()
                                    .status(200)
                                    .header("content-type", "application/grpc");
                                if let Some(meta) = echo_meta {
                                    response = response.header("x-echoed-meta", meta);
                                }
                                let Ok(mut send) =
                                    respond.send_response(response.body(()).unwrap(), false)
                                else {
                                    return;
                                };
                                if send.send_data(Bytes::from(data), false).is_err() {
                                    return;
                                }
                                let mut trailers = http::HeaderMap::new();
                                trailers.insert("grpc-status", "0".parse().unwrap());
                                trailers.insert("grpc-message", "ok".parse().unwrap());
                                let _ = send.send_trailers(trailers);
                            }
                        }
                    });
                }
            });
        }
    });

    GrpcServer { addr, state }
}

fn chain_from_rego(rego: &str) -> Arc<PolicyChain> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("http2.rego");
    std::fs::write(&path, rego).unwrap();
    std::mem::forget(dir);

    let evaluator =
        LocalPolicyEvaluator::from_file(&path, "data.mcp.http2.allow".to_string()).unwrap();
    Arc::new(PolicyChain::new(
        vec![PolicyEvaluatorKind::Local(evaluator)],
        EvalMode::All,
    ))
}

fn allow_all_chain() -> Arc<PolicyChain> {
    chain_from_rego("package mcp.http2\ndefault allow = true\n")
}

fn build_engine(config: Http2Config) -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-h2-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");

    Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_http2_config(config)
        .with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: String) -> Result<String, String> {
    let exec_id = engine
        .run_js(code)
        .execute()
        .await
        .map_err(|error| format!("submit should succeed: {error}"))?;

    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return Ok(info.result.unwrap_or_default()),
                "failed" => return Err(info.error.unwrap_or_default()),
                "timed_out" => return Err("execution timed out".to_string()),
                _ => continue,
            }
        }
    }

    Err("timeout waiting for execution".to_string())
}

/// JS prelude shared by tests: gRPC frame helpers over node:buffer.
const GRPC_HELPERS: &str = r#"
import http2 from 'node:http2';
import { Buffer } from 'node:buffer';

function grpcFrame(payload) {
    const frame = Buffer.alloc(5 + payload.length);
    frame.writeUInt8(0, 0);
    frame.writeUInt32BE(payload.length, 1);
    payload.copy(frame, 5);
    return frame;
}

function parseGrpcFrame(buffer) {
    if (buffer.length < 5) throw new Error("short gRPC frame: " + buffer.length);
    const compressed = buffer.readUInt8(0);
    const length = buffer.readUInt32BE(1);
    if (buffer.length !== 5 + length) throw new Error("bad gRPC frame length");
    return { compressed, payload: buffer.subarray(5) };
}

function collectCall(stream) {
    return new Promise((resolve, reject) => {
        let response = null;
        let trailers = null;
        const chunks = [];
        stream.on('response', (headers) => { response = headers; });
        stream.on('data', (chunk) => { chunks.push(chunk); });
        stream.on('trailers', (t) => { trailers = t; });
        stream.on('end', () => resolve({ response, trailers, body: Buffer.concat(chunks) }));
        stream.on('error', (err) => reject(err));
    });
}
"#;

#[tokio::test]
async fn grpc_style_unary_round_trip_with_trailers() {
    ensure_v8();
    let server = start_grpc_server().await;
    let engine = build_engine(Http2Config::new_with_chain(allow_all_chain()));

    let code = format!(
        r#"{helpers}
        const session = http2.connect("http://{addr}");
        const stream = session.request({{
            ':method': 'POST',
            ':path': '/echo.Service/Echo',
            'content-type': 'application/grpc',
            'te': 'trailers',
            'x-echo-meta': 'meta-value',
        }});
        const done = collectCall(stream);
        stream.write(grpcFrame(Buffer.from("hello grpc")));
        stream.end();
        const result = await done;

        if (result.response[':status'] !== 200) throw new Error("bad status: " + result.response[':status']);
        if (result.response['content-type'] !== 'application/grpc') {{
            throw new Error("bad content-type: " + result.response['content-type']);
        }}
        if (result.response['x-echoed-meta'] !== 'meta-value') {{
            throw new Error("metadata not echoed: " + result.response['x-echoed-meta']);
        }}
        const frame = parseGrpcFrame(result.body);
        if (frame.compressed !== 0) throw new Error("unexpected compressed flag");
        if (frame.payload.toString('utf8') !== "hello grpc") {{
            throw new Error("bad echo payload: " + frame.payload.toString('utf8'));
        }}
        if (result.trailers['grpc-status'] !== '0') {{
            throw new Error("bad grpc-status: " + result.trailers['grpc-status']);
        }}
        if (result.trailers['grpc-message'] !== 'ok') {{
            throw new Error("bad grpc-message: " + result.trailers['grpc-message']);
        }}
        session.close();
        "#,
        helpers = GRPC_HELPERS,
        addr = server.addr,
    );

    run_js(&engine, code).await.expect("gRPC unary round trip should succeed");

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].get("te").map(String::as_str), Some("trailers"));
}

#[tokio::test]
async fn trailers_only_response_surfaces_grpc_status_in_headers() {
    ensure_v8();
    let server = start_grpc_server().await;
    let engine = build_engine(Http2Config::new_with_chain(allow_all_chain()));

    let code = format!(
        r#"{helpers}
        const session = http2.connect("http://{addr}");
        const stream = session.request(
            {{ ':method': 'POST', ':path': '/trailers.Only/Call', 'content-type': 'application/grpc' }},
            {{ endStream: true }});
        const result = await collectCall(stream);
        if (result.response['grpc-status'] !== '12') {{
            throw new Error("trailers-only grpc-status missing: " + JSON.stringify(result.response));
        }}
        if (result.body.length !== 0) throw new Error("trailers-only should have no body");
        if (result.trailers !== null) throw new Error("trailers-only should have no trailer frame");
        session.close();
        "#,
        helpers = GRPC_HELPERS,
        addr = server.addr,
    );

    run_js(&engine, code).await.expect("trailers-only call should succeed");
}

#[tokio::test]
async fn rst_stream_surfaces_as_stream_error_with_code() {
    ensure_v8();
    let server = start_grpc_server().await;
    let engine = build_engine(Http2Config::new_with_chain(allow_all_chain()));

    let code = format!(
        r#"{helpers}
        const session = http2.connect("http://{addr}");
        const stream = session.request(
            {{ ':method': 'POST', ':path': '/reset.Service/Refuse', 'content-type': 'application/grpc' }},
            {{ endStream: true }});
        const outcome = await collectCall(stream).then(
            () => "completed",
            (err) => "error:" + stream.rstCode);
        // NGHTTP2_REFUSED_STREAM = 7
        if (outcome !== "error:7") throw new Error("expected refused stream, got " + outcome);
        session.close();
        "#,
        helpers = GRPC_HELPERS,
        addr = server.addr,
    );

    run_js(&engine, code).await.expect("reset should surface as an error with rstCode");
}

#[tokio::test]
async fn stream_denied_by_policy_while_connect_allowed() {
    ensure_v8();
    let server = start_grpc_server().await;
    // Connects are allowed; only the /echo.Service methods may be called.
    let engine = build_engine(Http2Config::new_with_chain(chain_from_rego(
        r#"
package mcp.http2

default allow = false

allow if {
    input.operation == "connect"
}

allow if {
    input.operation == "request"
    startswith(input.path, "/echo.Service/")
}
"#,
    )));

    let code = format!(
        r#"{helpers}
        const session = http2.connect("http://{addr}");

        const allowed = session.request(
            {{ ':method': 'POST', ':path': '/echo.Service/Echo', 'content-type': 'application/grpc' }});
        const allowedDone = collectCall(allowed);
        allowed.write(grpcFrame(Buffer.from("ok")));
        allowed.end();
        const allowedResult = await allowedDone;
        if (allowedResult.trailers['grpc-status'] !== '0') throw new Error("allowed call should succeed");

        const denied = session.request(
            {{ ':method': 'POST', ':path': '/forbidden.Service/Call', 'content-type': 'application/grpc' }},
            {{ endStream: true }});
        const deniedOutcome = await collectCall(denied).then(
            () => "completed",
            (err) => String(err.message || err));
        if (!deniedOutcome.includes("denied by policy")) {{
            throw new Error("expected policy denial, got: " + deniedOutcome);
        }}
        session.close();
        "#,
        helpers = GRPC_HELPERS,
        addr = server.addr,
    );

    run_js(&engine, code).await.expect("per-stream policy should gate individual calls");

    // The denied stream never reached the server.
    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].get(":path").map(String::as_str),
        Some("/echo.Service/Echo")
    );
}

#[tokio::test]
async fn header_injection_applies_per_stream_for_matching_host() {
    ensure_v8();
    let server = start_grpc_server().await;
    let rule = HeaderRule::static_header(
        "127.0.0.1".to_string(),
        vec![],
        "x-api-key".to_string(),
        "grpc-credential".to_string(),
    )
    .expect("rule should be valid");
    let engine = build_engine(
        Http2Config::new_with_chain(allow_all_chain()).with_header_rules(vec![rule]),
    );

    let code = format!(
        r#"{helpers}
        const session = http2.connect("http://{addr}");
        const stream = session.request(
            {{ ':method': 'POST', ':path': '/echo.Service/Echo', 'content-type': 'application/grpc' }});
        const done = collectCall(stream);
        stream.write(grpcFrame(Buffer.from("x")));
        stream.end();
        const result = await done;
        if (result.trailers['grpc-status'] !== '0') throw new Error("call should succeed");
        // The injected credential must not be observable in the response
        // (the server does not echo x-api-key), and there is no request
        // header read-back API — nothing to assert JS-side by design.
        session.close();
        "#,
        helpers = GRPC_HELPERS,
        addr = server.addr,
    );

    run_js(&engine, code).await.expect("call should succeed");

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].get("x-api-key").map(String::as_str),
        Some("grpc-credential"),
        "matching host should receive the injected header on the stream"
    );
}

#[tokio::test]
async fn secret_never_sent_to_non_matching_host() {
    ensure_v8();
    let server = start_grpc_server().await;
    let rule = HeaderRule::static_header(
        "api.example.com".to_string(),
        vec![],
        "authorization".to_string(),
        "Bearer super-secret".to_string(),
    )
    .expect("rule should be valid");
    let engine = build_engine(
        Http2Config::new_with_chain(allow_all_chain()).with_header_rules(vec![rule]),
    );

    let code = format!(
        r#"{helpers}
        const session = http2.connect("http://{addr}");
        const stream = session.request(
            {{ ':method': 'POST', ':path': '/echo.Service/Echo', 'content-type': 'application/grpc' }},
            {{ endStream: true }});
        const result = await collectCall(stream);
        if (result.trailers['grpc-status'] !== '0') throw new Error("call should succeed");
        session.close();
        "#,
        helpers = GRPC_HELPERS,
        addr = server.addr,
    );

    run_js(&engine, code).await.expect("call should succeed");

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert!(
        !requests[0].contains_key("authorization"),
        "secret scoped to another host must not leak to this one"
    );
}

#[tokio::test]
async fn http2_shim_reports_capability_disabled_without_config() {
    ensure_v8();
    let tmp = std::env::temp_dir().join(format!(
        "mcp-h2-absent-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    let engine =
        Engine::new_stateless(64 * 1024 * 1024, 30, 4).with_execution_registry(Arc::new(registry));

    let code = r#"
        import http2 from 'node:http2';
        const outcome = await new Promise((resolve) => {
            const session = http2.connect("http://127.0.0.1:1");
            session.on('error', (err) => resolve(String(err.message || err)));
            session.on('connect', () => resolve("connected"));
        });
        if (!outcome.includes("http2 is not enabled")) {
            throw new Error("expected capability-disabled error, got: " + outcome);
        }
    "#
    .to_string();

    run_js(&engine, code).await.expect("check should succeed");
}

/// Regression test for the rustls CryptoProvider panic. This dependency
/// graph compiles rustls 0.23 with both backends (aws-lc-rs via
/// tokio-rustls's defaults, ring via reqwest's rustls-tls), and with both
/// present `ClientConfig::builder()` panics — "Could not automatically
/// determine the process-level CryptoProvider" — unless a default provider
/// was installed first. `initialize_v8()` installs ring, so an `https://`
/// connect must get past the config builder and fail at the TLS handshake
/// (the listener below speaks no TLS), not die in a panicked task.
#[tokio::test]
async fn https_connect_reaches_tls_handshake_instead_of_crypto_provider_panic() {
    ensure_v8();

    // Plain TCP listener that accepts and immediately closes: enough to get
    // the client past TCP connect and into the rustls config builder.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            drop(stream);
        }
    });

    let engine = build_engine(Http2Config::new_with_chain(allow_all_chain()));
    let code = format!(
        r#"
        import http2 from 'node:http2';
        const outcome = await new Promise((resolve) => {{
            const session = http2.connect("https://{addr}");
            session.on('error', (err) => resolve(String(err.message || err)));
            session.on('connect', () => resolve("connected"));
        }});
        if (!outcome.includes("TLS handshake")) {{
            throw new Error("expected a TLS handshake error, got: " + outcome);
        }}
        "#
    );

    run_js(&engine, code)
        .await
        .expect("https connect should surface a TLS handshake error");
}
