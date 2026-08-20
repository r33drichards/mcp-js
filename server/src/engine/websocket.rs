//! Policy-gated WebSocket client for the JavaScript runtime.
//!
//! Exposes the WHATWG `WebSocket` global (the JS class lives in
//! `web_compat/websocket.js`) backed by four async ops over
//! tokio-tungstenite. Like `fetch`, the capability is off by default and
//! unlocked by a `websocket` section in `--policies-json`; the policy is
//! evaluated once per connection at handshake time with input
//! `{operation: "connect", url, protocols, headers, url_parsed}`.
//!
//! Fetch header-injection rules (`--fetch-header` / `--fetch-header-config`)
//! are applied to the handshake request for matching hosts, so credentials
//! can ride the `Upgrade` request without ever being visible to JS — the
//! browser WebSocket API has no way to set or read handshake headers, which
//! keeps the injected values structurally out of the isolate.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use super::fetch::{HeaderRule, apply_header_rules, b64_decode, b64_encode};
use super::opa::PolicyChain;

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for the WebSocket capability. Stored in deno_core's
/// `OpState`; its presence is what turns the ops on.
#[derive(Clone)]
pub struct WebSocketConfig {
    pub policy_chain: Arc<PolicyChain>,
    /// Handshake header injection rules (shared vocabulary with fetch).
    pub header_rules: Vec<HeaderRule>,
}

impl WebSocketConfig {
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

impl std::fmt::Debug for WebSocketConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Header rules can carry static credentials; keep them out of logs.
        f.debug_struct("WebSocketConfig")
            .field("header_rules", &format!("<{} rule(s)>", self.header_rules.len()))
            .finish_non_exhaustive()
    }
}

// ── Connection registry ──────────────────────────────────────────────────

type WsSink = Arc<tokio::sync::Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>;
type WsSource = Arc<tokio::sync::Mutex<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>>;

#[derive(Clone)]
struct WsHandle {
    sink: WsSink,
    source: WsSource,
    /// Cancelled by `op_ws_drop`; a pending `op_ws_recv` holds its own Arc
    /// clones, so removing the map entry alone would not wake it and the
    /// execution's event loop would hang until the timeout.
    cancel: tokio_util::sync::CancellationToken,
}

/// Live connections for one isolate, keyed by resource id. Connections are
/// per-execution: OpState is dropped when the run ends, which drops the
/// streams and closes the sockets. (A heap snapshot cannot carry a live
/// socket across turns; the JS side surfaces that as a closed socket.)
#[derive(Default)]
struct WsConnections {
    next_id: u32,
    conns: HashMap<u32, WsHandle>,
}

fn take_handle(state: &Rc<RefCell<OpState>>, rid: u32) -> Result<WsHandle, JsErrorBox> {
    let state = state.borrow();
    let conns = state
        .try_borrow::<WsConnections>()
        .ok_or_else(|| JsErrorBox::generic("websocket: no open connections"))?;
    conns
        .conns
        .get(&rid)
        .cloned()
        .ok_or_else(|| JsErrorBox::generic(format!("websocket: unknown connection id {rid}")))
}

// ── OPA policy input ─────────────────────────────────────────────────────

#[derive(Serialize)]
struct WsPolicyInput {
    operation: &'static str,
    url: String,
    protocols: Vec<String>,
    headers: HashMap<String, String>,
    url_parsed: WsUrlParsed,
}

#[derive(Serialize)]
struct WsUrlParsed {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
    query: String,
}

// ── Ops ──────────────────────────────────────────────────────────────────

const CONNECT_TIMEOUT_SECS: u64 = 30;

/// Async op: policy-gated WebSocket connect. Returns JSON
/// `{"rid": <u32>, "protocol": "<accepted subprotocol or empty>"}`.
#[op2]
#[string]
async fn op_ws_connect(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
    #[string] protocols_json: String,
) -> Result<String, JsErrorBox> {
    // Clone config out of OpState before any .await (Rc is !Send).
    let (policy_chain, header_rules) = {
        let state = state.borrow();
        let config = state.try_borrow::<WebSocketConfig>().ok_or_else(|| {
            JsErrorBox::generic("websocket: internal error — no websocket config available")
        })?;
        (config.policy_chain.clone(), config.header_rules.clone())
    };

    let protocols: Vec<String> = serde_json::from_str(&protocols_json)
        .map_err(|e| JsErrorBox::generic(format!("websocket: invalid protocols JSON: {e}")))?;

    // Same spawn pattern as op_fetch: keep the deeply nested async state
    // machine off deno_core's op driver (RefCell re-entrancy, see fetch.rs).
    let (ws, accepted_protocol) = tokio::spawn(async move {
        do_ws_connect(url, protocols, policy_chain, header_rules).await
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("websocket task join error: {e}")))?
    .map_err(JsErrorBox::generic)?;

    let (sink, source) = ws.split();
    let rid = {
        let mut state = state.borrow_mut();
        if state.try_borrow::<WsConnections>().is_none() {
            state.put(WsConnections::default());
        }
        let conns = state.borrow_mut::<WsConnections>();
        let rid = conns.next_id;
        conns.next_id += 1;
        conns.conns.insert(
            rid,
            WsHandle {
                sink: Arc::new(tokio::sync::Mutex::new(sink)),
                source: Arc::new(tokio::sync::Mutex::new(source)),
                cancel: tokio_util::sync::CancellationToken::new(),
            },
        );
        rid
    };

    Ok(serde_json::json!({ "rid": rid, "protocol": accepted_protocol }).to_string())
}

async fn do_ws_connect(
    url_str: String,
    protocols: Vec<String>,
    policy_chain: Arc<PolicyChain>,
    header_rules: Vec<HeaderRule>,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, String), String> {
    let parsed_url = url::Url::parse(&url_str)
        .map_err(|e| format!("websocket: invalid URL '{url_str}': {e}"))?;
    if parsed_url.scheme() != "ws" && parsed_url.scheme() != "wss" {
        return Err(format!(
            "websocket: unsupported scheme '{}' (expected ws or wss)",
            parsed_url.scheme()
        ));
    }
    let url_host = parsed_url.host_str().unwrap_or("").to_string();

    // Handshake header injection. The JS WebSocket API cannot set headers, so
    // this map is exclusively server-side rule output; user precedence rules
    // are moot but apply_header_rules is reused for the host/method matching.
    let mut headers: HashMap<String, String> = HashMap::new();
    apply_header_rules(&header_rules, &url_host, "GET", &mut headers)
        .await
        .map_err(|e| format!("websocket: credential injection failed for host '{url_host}': {e}"))?;

    let policy_input = WsPolicyInput {
        operation: "connect",
        url: url_str.clone(),
        protocols: protocols.clone(),
        headers: headers.clone(),
        url_parsed: WsUrlParsed {
            scheme: parsed_url.scheme().to_string(),
            host: url_host,
            port: parsed_url.port(),
            path: parsed_url.path().to_string(),
            query: parsed_url.query().unwrap_or("").to_string(),
        },
    };
    let input_value = serde_json::to_value(&policy_input)
        .map_err(|e| format!("websocket: failed to serialize policy input: {e}"))?;
    let allowed = policy_chain.evaluate(&input_value).await?;
    if !allowed {
        return Err(format!(
            "websocket denied by policy: connect to {url_str} is not allowed"
        ));
    }

    let mut request = url_str
        .as_str()
        .into_client_request()
        .map_err(|e| format!("websocket: invalid URL '{url_str}': {e}"))?;
    if !protocols.is_empty() {
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            protocols
                .join(", ")
                .parse()
                .map_err(|e| format!("websocket: invalid protocol list: {e}"))?,
        );
    }
    for (name, value) in &headers {
        let name: tokio_tungstenite::tungstenite::http::HeaderName = name
            .parse()
            .map_err(|e| format!("websocket: invalid injected header name '{name}': {e}"))?;
        let value = value
            .parse()
            .map_err(|e| format!("websocket: invalid injected header value for '{name}': {e}"))?;
        request.headers_mut().insert(name, value);
    }

    let (ws, response) = tokio::time::timeout(
        std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS),
        connect_async(request),
    )
    .await
    .map_err(|_| format!("websocket: connect to {url_str} timed out"))?
    .map_err(|e| format!("websocket: connect to {url_str} failed: {e}"))?;

    let accepted_protocol = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    Ok((ws, accepted_protocol))
}

/// Async op: send one frame. `kind` is "text" (data is the text) or
/// "binary" (data is base64).
#[op2]
async fn op_ws_send(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: u32,
    #[string] kind: String,
    #[string] data: String,
) -> Result<(), JsErrorBox> {
    let WsHandle { sink, .. } = take_handle(&state, rid)?;
    let message = match kind.as_str() {
        "text" => Message::Text(data.into()),
        "binary" => Message::Binary(b64_decode(&data).map_err(JsErrorBox::generic)?.into()),
        other => {
            return Err(JsErrorBox::generic(format!(
                "websocket: unknown frame kind '{other}'"
            )));
        }
    };
    tokio::spawn(async move { sink.lock().await.send(message).await })
        .await
        .map_err(|e| JsErrorBox::generic(format!("websocket task join error: {e}")))?
        .map_err(|e| JsErrorBox::generic(format!("websocket: send failed: {e}")))
}

/// Async op: receive the next data or close event as JSON. Ping/pong frames
/// are handled by tungstenite and skipped. Shapes:
/// `{"kind":"text","data":...}`, `{"kind":"binary","data":<b64>}`,
/// `{"kind":"close","code":u16,"reason":...,"wasClean":bool}`,
/// `{"kind":"error","message":...}`.
#[op2]
#[string]
async fn op_ws_recv(state: Rc<RefCell<OpState>>, #[smi] rid: u32) -> Result<String, JsErrorBox> {
    let WsHandle { source, cancel, .. } = take_handle(&state, rid)?;
    let event = tokio::spawn(async move {
        let mut source = source.lock().await;
        loop {
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    // op_ws_drop: surface as an abnormal close; the JS side
                    // swallows it when it initiated the drop itself.
                    return serde_json::json!({
                        "kind": "close", "code": 1006, "reason": "", "wasClean": false,
                    });
                }
                next = source.next() => next,
            };
            match next {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::json!({"kind": "text", "data": text.as_str()});
                }
                Some(Ok(Message::Binary(bytes))) => {
                    return serde_json::json!({"kind": "binary", "data": b64_encode(&bytes)});
                }
                // Ping/pong are auto-answered by tungstenite; not surfaced to
                // JS (the browser API has no ping/pong access either).
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = match frame {
                        Some(CloseFrame { code, reason }) => (u16::from(code), reason.to_string()),
                        None => (1005, String::new()),
                    };
                    return serde_json::json!({
                        "kind": "close", "code": code, "reason": reason, "wasClean": true,
                    });
                }
                Some(Ok(Message::Frame(_))) => continue,
                Some(Err(e)) => {
                    return serde_json::json!({"kind": "error", "message": e.to_string()});
                }
                // Stream ended without a close frame: abnormal closure.
                None => {
                    return serde_json::json!({
                        "kind": "close", "code": 1006, "reason": "", "wasClean": false,
                    });
                }
            }
        }
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("websocket task join error: {e}")))?;

    Ok(event.to_string())
}

/// Async op: send a close frame. `code == 0` means "no code" (bare close).
#[op2]
async fn op_ws_close(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: u32,
    #[smi] code: u32,
    #[string] reason: String,
) -> Result<(), JsErrorBox> {
    let WsHandle { sink, .. } = take_handle(&state, rid)?;
    let frame = if code == 0 {
        None
    } else {
        Some(CloseFrame {
            code: CloseCode::from(code as u16),
            reason: reason.into(),
        })
    };
    tokio::spawn(async move { sink.lock().await.send(Message::Close(frame)).await })
        .await
        .map_err(|e| JsErrorBox::generic(format!("websocket task join error: {e}")))?
        // A close race (peer already closed) is not an error worth surfacing.
        .or(Ok(()))
}

/// Fast op: drop a connection's handles, closing the socket if it is still
/// open. Called by the JS side once the close event has been delivered.
#[op2(fast)]
fn op_ws_drop(state: &mut OpState, #[smi] rid: u32) {
    if let Some(conns) = state.try_borrow_mut::<WsConnections>() {
        if let Some(handle) = conns.conns.remove(&rid) {
            // Wake any pending recv so the execution's event loop can drain.
            handle.cancel.cancel();
        }
    }
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    websocket_ext,
    ops = [op_ws_connect, op_ws_send, op_ws_recv, op_ws_close, op_ws_drop],
);

/// Create the websocket extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    websocket_ext::init()
}

// ── Inject the WebSocket class into the global scope ────────────────────

const WEBSOCKET_JS: &str = include_str!("web_compat/websocket.js");

/// Inject the `globalThis.WebSocket` class. Must run after the web-compat
/// layer (EventTarget, MessageEvent, DOMException, Blob) and before sandbox
/// hardening.
pub fn inject_websocket(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<websocket-setup>", WEBSOCKET_JS.to_string())
        .map_err(|e| format!("Failed to install WebSocket: {e}"))?;
    Ok(())
}

/// Snapshot-path twin of [`inject_websocket`].
pub fn inject_websocket_snapshot(
    runtime: &mut deno_core::JsRuntimeForSnapshot,
) -> Result<(), String> {
    runtime
        .execute_script("<websocket-setup>", WEBSOCKET_JS.to_string())
        .map_err(|e| format!("Failed to install WebSocket: {e}"))?;
    Ok(())
}
