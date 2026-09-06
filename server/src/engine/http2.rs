//! Structured HTTP/2 session/stream ops for the `node:http2` shim.
//!
//! This is the transport that lets stock gRPC clients (`@grpc/grpc-js`) run
//! inside the sandbox without raw sockets. Streams are opened with an
//! explicit header map, so the two properties of the fetch/WebSocket
//! capability model survive:
//!
//! 1. **Per-stream policy** — every request is OPA-evaluated under the
//!    `http2` chain with `{operation: "request", authority, method, path,
//!    headers}` (sessions are additionally gated at connect time with
//!    `{operation: "connect", url, url_parsed}`).
//! 2. **Server-side header injection** — `--fetch-header` rules are applied
//!    per stream (gRPC metadata is just HTTP/2 headers), so credentials such
//!    as `x-modal-token-*` never exist inside the isolate.
//!
//! Backed by the `h2` crate (hyper's HTTP/2 layer). TLS uses the same rustls
//! stack as fetch/WebSocket with ALPN `h2`; plaintext `http://` authorities
//! use h2c prior knowledge, which is the normal insecure-gRPC arrangement.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use bytes::Bytes;
use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::fetch::{HeaderRule, apply_header_rules, b64_decode, b64_encode};
use super::opa::PolicyChain;

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for the HTTP/2 capability. Stored in deno_core's `OpState`;
/// its presence is what turns the ops (and the `node:http2` shim) on.
#[derive(Clone)]
pub struct Http2Config {
    pub policy_chain: Arc<PolicyChain>,
    /// Per-stream header injection rules (shared vocabulary with fetch).
    pub header_rules: Vec<HeaderRule>,
}

impl Http2Config {
    pub fn new_with_chain(chain: Arc<PolicyChain>) -> Self {
        Self {
            policy_chain: chain,
            header_rules: Vec::new(),
        }
    }

    pub fn with_header_rules(mut self, rules: Vec<HeaderRule>) -> Self {
        self.header_rules = rules;
        self
    }
}

impl std::fmt::Debug for Http2Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Header rules can carry static credentials; keep them out of logs.
        f.debug_struct("Http2Config")
            .field("header_rules", &format!("<{} rule(s)>", self.header_rules.len()))
            .finish_non_exhaustive()
    }
}

// ── Session / stream registries ──────────────────────────────────────────

#[derive(Clone)]
struct H2Session {
    send_request: h2::client::SendRequest<Bytes>,
    ping_pong: Arc<tokio::sync::Mutex<Option<h2::PingPong>>>,
    /// Cancels the connection driver task, tearing the session down.
    cancel: CancellationToken,
    scheme: String,
    authority: String,
    host: String,
}

#[derive(Clone)]
struct H2StreamHandle {
    response: Arc<tokio::sync::Mutex<Option<h2::client::ResponseFuture>>>,
    send: Arc<tokio::sync::Mutex<h2::SendStream<Bytes>>>,
    recv: Arc<tokio::sync::Mutex<Option<h2::RecvStream>>>,
    /// Session cancel token: a torn-down session must wake pending reads.
    cancel: CancellationToken,
    /// Per-stream cancel token, fired by cancel/drop. A pending read holds
    /// its own Arc clones, so removing the map entry alone would not wake
    /// it — the op would stay pending and the execution's event loop could
    /// never drain (a cancelled gRPC call would hang the whole run).
    stream_cancel: CancellationToken,
}

/// Map-owned session entry. The `DropGuard` cancels the connection driver
/// when the entry is removed (explicit close) or when OpState is dropped at
/// the end of the execution — without it the spawned driver task would
/// outlive the isolate and leak the TCP/TLS connection.
struct H2SessionEntry {
    session: H2Session,
    _guard: tokio_util::sync::DropGuard,
}

#[derive(Default)]
struct H2State {
    next_id: u32,
    sessions: HashMap<u32, H2SessionEntry>,
    streams: HashMap<u32, H2StreamHandle>,
}

fn with_h2_state<R>(
    state: &Rc<RefCell<OpState>>,
    f: impl FnOnce(&mut H2State) -> R,
) -> R {
    let mut state = state.borrow_mut();
    if state.try_borrow::<H2State>().is_none() {
        state.put(H2State::default());
    }
    f(state.borrow_mut::<H2State>())
}

fn get_session(state: &Rc<RefCell<OpState>>, rid: u32) -> Result<H2Session, JsErrorBox> {
    with_h2_state(state, |h2| h2.sessions.get(&rid).map(|e| e.session.clone()))
        .ok_or_else(|| JsErrorBox::generic(format!("http2: unknown session id {rid}")))
}

fn get_stream(state: &Rc<RefCell<OpState>>, rid: u32) -> Result<H2StreamHandle, JsErrorBox> {
    with_h2_state(state, |h2| h2.streams.get(&rid).cloned())
        .ok_or_else(|| JsErrorBox::generic(format!("http2: unknown stream id {rid}")))
}

// ── OPA policy input ─────────────────────────────────────────────────────

#[derive(Serialize)]
struct H2ConnectPolicyInput {
    operation: &'static str,
    url: String,
    url_parsed: H2UrlParsed,
}

#[derive(Serialize)]
struct H2UrlParsed {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
    query: String,
}

#[derive(Serialize)]
struct H2RequestPolicyInput {
    operation: &'static str,
    scheme: String,
    authority: String,
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

// ── op_h2_connect ────────────────────────────────────────────────────────

const CONNECT_TIMEOUT_SECS: u64 = 30;

/// Async op: policy-gated HTTP/2 session connect. `url` is an
/// `http://host[:port]` or `https://host[:port]` authority. Returns JSON
/// `{"rid": <u32>}`.
#[op2(async)]
#[string]
async fn op_h2_connect(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
) -> Result<String, JsErrorBox> {
    let (policy_chain, _) = config_from_state(&state)?;

    // Same spawn pattern as op_fetch: keep the nested async state machine
    // off deno_core's op driver (RefCell re-entrancy, see fetch.rs).
    let session = tokio::spawn(async move { do_h2_connect(url, policy_chain).await })
        .await
        .map_err(|e| JsErrorBox::generic(format!("http2 task join error: {e}")))?
        .map_err(JsErrorBox::generic)?;

    let guard = session.cancel.clone().drop_guard();
    let rid = with_h2_state(&state, |h2| {
        let rid = h2.next_id;
        h2.next_id += 1;
        h2.sessions.insert(rid, H2SessionEntry { session, _guard: guard });
        rid
    });

    Ok(serde_json::json!({ "rid": rid }).to_string())
}

fn config_from_state(
    state: &Rc<RefCell<OpState>>,
) -> Result<(Arc<PolicyChain>, Vec<HeaderRule>), JsErrorBox> {
    let state = state.borrow();
    let config = state.try_borrow::<Http2Config>().ok_or_else(|| {
        JsErrorBox::generic("http2: internal error — no http2 config available")
    })?;
    Ok((config.policy_chain.clone(), config.header_rules.clone()))
}

async fn do_h2_connect(
    url_str: String,
    policy_chain: Arc<PolicyChain>,
) -> Result<H2Session, String> {
    let parsed = url::Url::parse(&url_str)
        .map_err(|e| format!("http2: invalid URL '{url_str}': {e}"))?;
    let scheme = parsed.scheme().to_string();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "http2: unsupported scheme '{scheme}' (expected http or https)"
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("http2: URL '{url_str}' has no host"))?
        .to_string();
    let port = parsed
        .port()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let authority = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.clone(),
    };

    let policy_input = H2ConnectPolicyInput {
        operation: "connect",
        url: url_str.clone(),
        url_parsed: H2UrlParsed {
            scheme: scheme.clone(),
            host: host.clone(),
            port: parsed.port(),
            path: parsed.path().to_string(),
            query: parsed.query().unwrap_or("").to_string(),
        },
    };
    let input_value = serde_json::to_value(&policy_input)
        .map_err(|e| format!("http2: failed to serialize policy input: {e}"))?;
    if !policy_chain.evaluate(&input_value).await? {
        return Err(format!(
            "http2 denied by policy: connect to {url_str} is not allowed"
        ));
    }

    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS),
        tokio::net::TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| format!("http2: connect to {url_str} timed out"))?
    .map_err(|e| format!("http2: connect to {url_str} failed: {e}"))?;
    let _ = tcp.set_nodelay(true);

    let cancel = CancellationToken::new();

    // The h2 handshake is generic over the IO type, so each arm completes it
    // and spawns its own connection driver; only the uniform SendRequest and
    // PingPong handles leave the arm.
    let (send_request, ping_pong) = if scheme == "https" {
        use tokio_rustls::rustls;

        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls_config.alpn_protocols = vec![b"h2".to_vec()];
        let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config));
        let server_name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|e| format!("http2: invalid TLS server name '{host}': {e}"))?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| format!("http2: TLS handshake with {url_str} failed: {e}"))?;

        let (send_request, mut connection) = h2::client::handshake(tls)
            .await
            .map_err(|e| format!("http2: h2 handshake with {url_str} failed: {e}"))?;
        let ping_pong = connection.ping_pong();
        let driver_cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = driver_cancel.cancelled() => {}
                _ = &mut connection => {}
            }
        });
        (send_request, ping_pong)
    } else {
        let (send_request, mut connection) = h2::client::handshake(tcp)
            .await
            .map_err(|e| format!("http2: h2 handshake with {url_str} failed: {e}"))?;
        let ping_pong = connection.ping_pong();
        let driver_cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = driver_cancel.cancelled() => {}
                _ = &mut connection => {}
            }
        });
        (send_request, ping_pong)
    };

    Ok(H2Session {
        send_request,
        ping_pong: Arc::new(tokio::sync::Mutex::new(ping_pong)),
        cancel,
        scheme,
        authority,
        host,
    })
}

// ── op_h2_request ────────────────────────────────────────────────────────

/// Async op: open a stream on a session. `headers_json` maps header names to
/// values; `:method` and `:path` pseudo-headers are required, `:authority`
/// and `:scheme` default to the session's. Header-injection rules and the
/// per-stream policy run here. Returns JSON `{"rid": <u32>}`.
#[op2(async)]
#[string]
async fn op_h2_request(
    state: Rc<RefCell<OpState>>,
    #[smi] session_rid: u32,
    #[string] headers_json: String,
    end_stream: bool,
) -> Result<String, JsErrorBox> {
    let (policy_chain, header_rules) = config_from_state(&state)?;
    let session = get_session(&state, session_rid)?;

    let (response, send) = tokio::spawn(async move {
        do_h2_request(session, headers_json, end_stream, policy_chain, header_rules).await
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("http2 task join error: {e}")))?
    .map_err(JsErrorBox::generic)?;

    let session_cancel = get_session(&state, session_rid)?.cancel;
    let rid = with_h2_state(&state, |h2| {
        let rid = h2.next_id;
        h2.next_id += 1;
        h2.streams.insert(
            rid,
            H2StreamHandle {
                response: Arc::new(tokio::sync::Mutex::new(Some(response))),
                send: Arc::new(tokio::sync::Mutex::new(send)),
                recv: Arc::new(tokio::sync::Mutex::new(None)),
                cancel: session_cancel,
                stream_cancel: CancellationToken::new(),
            },
        );
        rid
    });

    Ok(serde_json::json!({ "rid": rid }).to_string())
}

async fn do_h2_request(
    session: H2Session,
    headers_json: String,
    end_stream: bool,
    policy_chain: Arc<PolicyChain>,
    header_rules: Vec<HeaderRule>,
) -> Result<(h2::client::ResponseFuture, h2::SendStream<Bytes>), String> {
    let raw_headers: HashMap<String, String> = serde_json::from_str(&headers_json)
        .map_err(|e| format!("http2: invalid headers JSON: {e}"))?;

    let mut method = String::from("POST");
    let mut path = String::from("/");
    let mut scheme = session.scheme.clone();
    let mut authority = session.authority.clone();
    let mut headers: HashMap<String, String> = HashMap::new();
    for (name, value) in raw_headers {
        match name.as_str() {
            ":method" => method = value,
            ":path" => path = value,
            ":scheme" => scheme = value,
            ":authority" => authority = value,
            _ => {
                headers.insert(name.to_ascii_lowercase(), value);
            }
        }
    }

    // Server-side credential injection, scoped by the session's host — gRPC
    // metadata rides plain HTTP/2 headers, so this is where x-api-key-style
    // rules land without the isolate ever seeing the values.
    apply_header_rules(&header_rules, &session.host, &method, &mut headers)
        .await
        .map_err(|e| {
            format!(
                "http2: credential injection failed for host '{}': {e}",
                session.host
            )
        })?;

    let policy_input = H2RequestPolicyInput {
        operation: "request",
        scheme: scheme.clone(),
        authority: authority.clone(),
        method: method.clone(),
        path: path.clone(),
        headers: headers.clone(),
    };
    let input_value = serde_json::to_value(&policy_input)
        .map_err(|e| format!("http2: failed to serialize policy input: {e}"))?;
    if !policy_chain.evaluate(&input_value).await? {
        return Err(format!(
            "http2 denied by policy: {method} {scheme}://{authority}{path} is not allowed"
        ));
    }

    let uri = format!("{scheme}://{authority}{path}");
    let mut builder = http::Request::builder()
        .method(
            method
                .parse::<http::Method>()
                .map_err(|e| format!("http2: invalid method '{method}': {e}"))?,
        )
        .uri(&uri)
        .version(http::Version::HTTP_2);
    for (name, value) in &headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let request = builder
        .body(())
        .map_err(|e| format!("http2: invalid request for '{uri}': {e}"))?;

    let mut send_request = session
        .send_request
        .ready()
        .await
        .map_err(|e| format!("http2: session not ready: {}", describe_h2_error(&e)))?;
    send_request
        .send_request(request, end_stream)
        .map_err(|e| format!("http2: request failed: {}", describe_h2_error(&e)))
}

// ── Stream data ops ──────────────────────────────────────────────────────

/// Async op: send one DATA chunk (base64), respecting h2 flow control.
#[op2(async)]
async fn op_h2_send_data(
    state: Rc<RefCell<OpState>>,
    #[smi] stream_rid: u32,
    #[string] data_b64: String,
    end_stream: bool,
) -> Result<(), JsErrorBox> {
    let stream = get_stream(&state, stream_rid)?;
    tokio::spawn(async move {
        let mut send = stream.send.lock().await;
        let data = Bytes::from(b64_decode(&data_b64)?);
        send_all(&mut send, data, end_stream).await
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("http2 task join error: {e}")))?
    .map_err(JsErrorBox::generic)
}

/// Send `data`, chunked to the flow-control capacity the peer grants.
async fn send_all(
    send: &mut h2::SendStream<Bytes>,
    mut data: Bytes,
    end_stream: bool,
) -> Result<(), String> {
    if data.is_empty() {
        return send
            .send_data(data, end_stream)
            .map_err(|e| format!("http2: send failed: {}", describe_h2_error(&e)));
    }
    while !data.is_empty() {
        send.reserve_capacity(data.len());
        let granted = futures::future::poll_fn(|cx| send.poll_capacity(cx))
            .await
            .ok_or_else(|| "http2: stream closed while sending".to_string())?
            .map_err(|e| format!("http2: send failed: {}", describe_h2_error(&e)))?;
        let chunk = data.split_to(granted.min(data.len()));
        let is_last = data.is_empty() && end_stream;
        send.send_data(chunk, is_last)
            .map_err(|e| format!("http2: send failed: {}", describe_h2_error(&e)))?;
    }
    Ok(())
}

/// Async op: await the response HEADERS frame. Returns JSON
/// `{"kind":"response","status":<u16>,"headers":{..},"endStream":bool}`, or
/// — when the stream dies before headers arrive — `{"kind":"reset","code":..}`
/// / `{"kind":"error","message":..}` so RST_STREAM keeps its error code.
#[op2(async)]
#[string]
async fn op_h2_response(
    state: Rc<RefCell<OpState>>,
    #[smi] stream_rid: u32,
) -> Result<String, JsErrorBox> {
    let stream = get_stream(&state, stream_rid)?;
    let event = tokio::spawn(async move {
        let Some(response_future) = stream.response.lock().await.take() else {
            return serde_json::json!({"kind": "error", "message": "http2: response already consumed"});
        };
        let response = tokio::select! {
            biased;
            _ = stream.cancel.cancelled() => {
                return serde_json::json!({"kind": "error", "message": "http2: session closed"});
            }
            _ = stream.stream_cancel.cancelled() => {
                return serde_json::json!({"kind": "error", "message": "http2: stream closed"});
            }
            r = response_future => match r {
                Ok(response) => response,
                Err(e) => return h2_error_event(&e),
            },
        };

        let status = response.status().as_u16();
        let headers = header_map_to_json(response.headers());
        let body = response.into_body();
        let end_stream = body.is_end_stream();
        *stream.recv.lock().await = Some(body);
        serde_json::json!({
            "kind": "response", "status": status, "headers": headers, "endStream": end_stream,
        })
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("http2 task join error: {e}")))?;
    Ok(event.to_string())
}

/// Async op: read the next body event. Shapes:
/// `{"kind":"data","data":<b64>}`, `{"kind":"trailers","trailers":{..}}`,
/// `{"kind":"end"}`, `{"kind":"reset","code":<u32>}`, `{"kind":"error","message":..}`.
#[op2(async)]
#[string]
async fn op_h2_read(
    state: Rc<RefCell<OpState>>,
    #[smi] stream_rid: u32,
) -> Result<String, JsErrorBox> {
    let stream = get_stream(&state, stream_rid)?;
    let event = tokio::spawn(async move {
        let mut recv_guard = stream.recv.lock().await;
        let recv = match recv_guard.as_mut() {
            Some(recv) => recv,
            None => return serde_json::json!({"kind": "end"}),
        };

        let next = tokio::select! {
            biased;
            _ = stream.cancel.cancelled() => {
                return serde_json::json!({"kind": "error", "message": "http2: session closed"});
            }
            _ = stream.stream_cancel.cancelled() => {
                return serde_json::json!({"kind": "error", "message": "http2: stream closed"});
            }
            d = futures::future::poll_fn(|cx| recv.poll_data(cx)) => d,
        };

        match next {
            Some(Ok(bytes)) => {
                let _ = recv.flow_control().release_capacity(bytes.len());
                serde_json::json!({"kind": "data", "data": b64_encode(&bytes)})
            }
            Some(Err(e)) => h2_error_event(&e),
            None => {
                match futures::future::poll_fn(|cx| recv.poll_trailers(cx)).await {
                    Ok(Some(trailers)) => {
                        *recv_guard = None;
                        serde_json::json!({"kind": "trailers", "trailers": header_map_to_json(&trailers)})
                    }
                    Ok(None) => {
                        *recv_guard = None;
                        serde_json::json!({"kind": "end"})
                    }
                    Err(e) => h2_error_event(&e),
                }
            }
        }
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("http2 task join error: {e}")))?;

    Ok(event.to_string())
}

/// Fast op: reset the stream (RST_STREAM) and drop its handles.
#[op2(fast)]
fn op_h2_cancel_stream(state: &mut OpState, #[smi] stream_rid: u32, #[smi] code: u32) {
    if let Some(h2) = state.try_borrow_mut::<H2State>() {
        if let Some(stream) = h2.streams.remove(&stream_rid) {
            if let Ok(mut send) = stream.send.try_lock() {
                send.send_reset(h2::Reason::from(code));
            }
            // Wake any pending response/read so the event loop can drain.
            stream.stream_cancel.cancel();
        }
    }
}

/// Fast op: drop a stream's handles without a reset (after a clean end).
#[op2(fast)]
fn op_h2_drop_stream(state: &mut OpState, #[smi] stream_rid: u32) {
    if let Some(h2) = state.try_borrow_mut::<H2State>() {
        if let Some(stream) = h2.streams.remove(&stream_rid) {
            stream.stream_cancel.cancel();
        }
    }
}

// ── Session lifecycle ops ────────────────────────────────────────────────

/// Async op: HTTP/2 PING round trip (used by gRPC keepalive).
#[op2(async)]
async fn op_h2_ping(
    state: Rc<RefCell<OpState>>,
    #[smi] session_rid: u32,
) -> Result<(), JsErrorBox> {
    let session = get_session(&state, session_rid)?;
    tokio::spawn(async move {
        let mut ping_pong = session.ping_pong.lock().await;
        match ping_pong.as_mut() {
            Some(pp) => pp
                .ping(h2::Ping::opaque())
                .await
                .map(|_| ())
                .map_err(|e| format!("http2: ping failed: {}", describe_h2_error(&e))),
            None => Err("http2: ping unavailable on this session".to_string()),
        }
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("http2 task join error: {e}")))?
    .map_err(JsErrorBox::generic)
}

/// Fast op: tear down a session — cancels the connection driver (dropping
/// the TCP/TLS connection) and wakes every pending stream op.
#[op2(fast)]
fn op_h2_close_session(state: &mut OpState, #[smi] session_rid: u32) {
    if let Some(h2) = state.try_borrow_mut::<H2State>() {
        if let Some(entry) = h2.sessions.remove(&session_rid) {
            entry.session.cancel.cancel();
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn header_map_to_json(headers: &http::HeaderMap) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        let value = String::from_utf8_lossy(value.as_bytes()).to_string();
        // Repeated headers join with ", " (gRPC metadata semantics are
        // handled in the shim; binary metadata is base64 on the wire already).
        match map.get_mut(name.as_str()) {
            Some(serde_json::Value::String(existing)) => {
                existing.push_str(", ");
                existing.push_str(&value);
            }
            _ => {
                map.insert(name.as_str().to_string(), serde_json::Value::String(value));
            }
        }
    }
    map
}

fn h2_error_event(error: &h2::Error) -> serde_json::Value {
    if let Some(reason) = error.reason() {
        serde_json::json!({"kind": "reset", "code": u32::from(reason)})
    } else {
        serde_json::json!({"kind": "error", "message": describe_h2_error(error)})
    }
}

fn describe_h2_error(error: &h2::Error) -> String {
    match error.reason() {
        Some(reason) => format!("{reason:?}"),
        None => error.to_string(),
    }
}

// ── Extension registration / JS injection ───────────────────────────────

deno_core::extension!(
    http2_ext,
    ops = [
        op_h2_connect,
        op_h2_request,
        op_h2_send_data,
        op_h2_response,
        op_h2_read,
        op_h2_cancel_stream,
        op_h2_drop_stream,
        op_h2_ping,
        op_h2_close_session,
    ],
);

/// Create the http2 extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    http2_ext::init()
}

/// Expose the op table to the `node:http2` shim. The shim is an ES module
/// served by the module loader, which cannot reach `Deno.core` after
/// hardening freezes it — so the ops are captured onto a hidden global here,
/// before hardening, mirroring how the other wrappers bind at inject time.
const HTTP2_BINDING_JS: &str = r#"
(function () {
    var ops = Deno.core.ops;
    Object.defineProperty(globalThis, '__mcpV8Http2Ops', {
        value: Object.freeze({
            connect: ops.op_h2_connect,
            request: ops.op_h2_request,
            sendData: ops.op_h2_send_data,
            response: ops.op_h2_response,
            read: ops.op_h2_read,
            cancelStream: ops.op_h2_cancel_stream,
            dropStream: ops.op_h2_drop_stream,
            ping: ops.op_h2_ping,
            closeSession: ops.op_h2_close_session,
        }),
        writable: false, enumerable: false, configurable: false,
    });
})();
"#;

pub fn inject_http2(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<http2-setup>", HTTP2_BINDING_JS.to_string())
        .map_err(|e| format!("Failed to install http2 binding: {e}"))?;
    Ok(())
}

pub fn inject_http2_snapshot(
    runtime: &mut deno_core::JsRuntimeForSnapshot,
) -> Result<(), String> {
    runtime
        .execute_script("<http2-setup>", HTTP2_BINDING_JS.to_string())
        .map_err(|e| format!("Failed to install http2 binding: {e}"))?;
    Ok(())
}
