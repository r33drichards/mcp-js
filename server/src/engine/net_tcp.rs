//! Loopback-only TCP ops backing `node:net` servers and client sockets.
//!
//! Raw TCP is deliberately absent from the default sandbox: outbound
//! networking goes through the policy-gated fetch / WebSocket / node:http2
//! capabilities (see the header of `node_compat/net.js`). These ops exist
//! for Node-compatibility harnesses whose tests talk to themselves over
//! 127.0.0.1 — the capability is off unless a `NetTcpConfig` is placed in
//! OpState, and every bind and connect is pinned to the loopback interface
//! regardless of the address JS asks for, so an enabled isolate still
//! cannot reach the network or accept remote connections.

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use super::fetch::{b64_decode, b64_encode};

// ── Configuration ────────────────────────────────────────────────────────

/// Presence of this config in OpState is what turns the TCP ops on.
/// All traffic is loopback-pinned; there is no wider mode.
///
/// `shutdown` matters for harnesses with a watchdog: a pending accept or
/// read op keeps the deno_core event loop alive, and
/// `terminate_execution` cannot wake it — cancelling this token resolves
/// every pending TCP op so the event loop can drain.
#[derive(Clone, Debug, Default)]
pub struct NetTcpConfig {
    pub shutdown: CancellationToken,
}

// ── Connection registry ──────────────────────────────────────────────────

#[derive(Clone)]
struct ListenerHandle {
    listener: Arc<TcpListener>,
    cancel: CancellationToken,
}

#[derive(Clone)]
struct StreamHandle {
    read: Arc<tokio::sync::Mutex<OwnedReadHalf>>,
    write: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    cancel: CancellationToken,
}

#[derive(Clone)]
struct UdpHandle {
    socket: Arc<tokio::net::UdpSocket>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct TcpRegistry {
    next_id: u32,
    listeners: HashMap<u32, ListenerHandle>,
    streams: HashMap<u32, StreamHandle>,
    udp: HashMap<u32, UdpHandle>,
}

fn ensure_enabled(state: &mut OpState) -> Result<&mut TcpRegistry, JsErrorBox> {
    if state.try_borrow::<NetTcpConfig>().is_none() {
        return Err(JsErrorBox::generic(
            "net is not enabled in this runtime: loopback TCP requires the \
             Node-compatibility harness configuration",
        ));
    }
    if state.try_borrow::<TcpRegistry>().is_none() {
        state.put(TcpRegistry::default());
    }
    Ok(state.borrow_mut::<TcpRegistry>())
}

fn register_stream(state: &Rc<RefCell<OpState>>, stream: TcpStream) -> Result<serde_json::Value, JsErrorBox> {
    let local = stream
        .local_addr()
        .map_err(|e| JsErrorBox::generic(format!("net: local_addr failed: {e}")))?;
    let peer = stream
        .peer_addr()
        .map_err(|e| JsErrorBox::generic(format!("net: peer_addr failed: {e}")))?;
    let _ = stream.set_nodelay(true);
    let (read, write) = stream.into_split();
    let mut state = state.borrow_mut();
    let registry = ensure_enabled(&mut state)?;
    let rid = registry.next_id;
    registry.next_id += 1;
    registry.streams.insert(
        rid,
        StreamHandle {
            read: Arc::new(tokio::sync::Mutex::new(read)),
            write: Arc::new(tokio::sync::Mutex::new(write)),
            cancel: CancellationToken::new(),
        },
    );
    Ok(addr_json(rid, &local, &peer))
}

fn addr_json(rid: u32, local: &SocketAddr, peer: &SocketAddr) -> serde_json::Value {
    serde_json::json!({
        "rid": rid,
        "localAddress": local.ip().to_string(),
        "localPort": local.port(),
        "localFamily": if local.is_ipv6() { "IPv6" } else { "IPv4" },
        "remoteAddress": peer.ip().to_string(),
        "remotePort": peer.port(),
        "remoteFamily": if peer.is_ipv6() { "IPv6" } else { "IPv4" },
    })
}

fn get_listener(
    state: &Rc<RefCell<OpState>>,
    rid: u32,
) -> Result<(ListenerHandle, CancellationToken), JsErrorBox> {
    let state = state.borrow();
    let shutdown = shutdown_token(&state)?;
    state
        .try_borrow::<TcpRegistry>()
        .and_then(|r| r.listeners.get(&rid).cloned())
        .map(|handle| (handle, shutdown))
        .ok_or_else(|| JsErrorBox::generic(format!("net: unknown listener id {rid}")))
}

fn get_stream(
    state: &Rc<RefCell<OpState>>,
    rid: u32,
) -> Result<(StreamHandle, CancellationToken), JsErrorBox> {
    let state = state.borrow();
    let shutdown = shutdown_token(&state)?;
    state
        .try_borrow::<TcpRegistry>()
        .and_then(|r| r.streams.get(&rid).cloned())
        .map(|handle| (handle, shutdown))
        .ok_or_else(|| JsErrorBox::generic(format!("net: unknown socket id {rid}")))
}

fn shutdown_token(state: &OpState) -> Result<CancellationToken, JsErrorBox> {
    state
        .try_borrow::<NetTcpConfig>()
        .map(|config| config.shutdown.clone())
        .ok_or_else(|| JsErrorBox::generic("net is not enabled in this runtime"))
}

/// Every bind and connect lands on loopback no matter what was asked for:
/// hostnames and wildcard addresses are rewritten, anything that names a
/// non-loopback IP is refused.
fn loopback_host(requested: &str, want_v6: bool) -> Result<std::net::IpAddr, JsErrorBox> {
    use std::net::IpAddr;
    let host = requested.trim();
    if host.is_empty()
        || host == "localhost"
        || host == "0.0.0.0"
        || host == "*"
    {
        return Ok(if want_v6 {
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        } else {
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        });
    }
    if host == "::" || host == "::0" {
        return Ok(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
    }
    match host.parse::<IpAddr>() {
        Ok(ip) if ip.is_loopback() => Ok(ip),
        Ok(ip) => Err(JsErrorBox::generic(format!(
            "net: address {ip} is not allowed — only loopback TCP is available in this runtime"
        ))),
        Err(_) => Err(JsErrorBox::generic(format!(
            "net: hostname '{host}' is not allowed — only loopback TCP is available in this runtime"
        ))),
    }
}

// ── Ops ──────────────────────────────────────────────────────────────────

/// Sync op: bind a loopback listener. Returns `{rid, address, port, family}`.
#[op2]
#[string]
fn op_tcp_listen(
    state: &mut OpState,
    #[string] host: String,
    #[smi] port: u32,
) -> Result<String, JsErrorBox> {
    ensure_enabled(state)?;
    let ip = loopback_host(&host, host.contains(':'))?;
    let addr = SocketAddr::new(ip, port as u16);
    let std_listener = std::net::TcpListener::bind(addr).map_err(|e| {
        let err = JsErrorBox::generic(format!("net: listen on {addr} failed: {e} ({})",
            match e.kind() {
                std::io::ErrorKind::AddrInUse => "EADDRINUSE",
                std::io::ErrorKind::PermissionDenied => "EACCES",
                _ => "EIO",
            }));
        err
    })?;
    std_listener
        .set_nonblocking(true)
        .map_err(|e| JsErrorBox::generic(format!("net: set_nonblocking failed: {e}")))?;
    let listener = TcpListener::from_std(std_listener)
        .map_err(|e| JsErrorBox::generic(format!("net: listener setup failed: {e}")))?;
    let local = listener
        .local_addr()
        .map_err(|e| JsErrorBox::generic(format!("net: local_addr failed: {e}")))?;

    let registry = ensure_enabled(state)?;
    let rid = registry.next_id;
    registry.next_id += 1;
    registry.listeners.insert(
        rid,
        ListenerHandle {
            listener: Arc::new(listener),
            cancel: CancellationToken::new(),
        },
    );
    Ok(serde_json::json!({
        "rid": rid,
        "address": local.ip().to_string(),
        "port": local.port(),
        "family": if local.is_ipv6() { "IPv6" } else { "IPv4" },
    })
    .to_string())
}

/// Async op: accept one connection. Returns the stream's address JSON, or
/// `{"closed": true}` once the listener is closed.
#[op2]
#[string]
async fn op_tcp_accept(state: Rc<RefCell<OpState>>, #[smi] rid: u32) -> Result<String, JsErrorBox> {
    let (ListenerHandle { listener, cancel, .. }, shutdown) = get_listener(&state, rid)?;
    let accepted = tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(None),
            _ = shutdown.cancelled() => Ok(None),
            accepted = listener.accept() => accepted.map(|(s, _)| Some(s)),
        }
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("net task join error: {e}")))?
    .map_err(|e| JsErrorBox::generic(format!("net: accept failed: {e}")))?;

    match accepted {
        None => Ok(serde_json::json!({"closed": true}).to_string()),
        Some(stream) => Ok(register_stream(&state, stream)?.to_string()),
    }
}

/// Async op: connect to a loopback address. Returns the stream's address JSON.
#[op2]
#[string]
async fn op_tcp_connect(
    state: Rc<RefCell<OpState>>,
    #[string] host: String,
    #[smi] port: u32,
    #[string] local_host: String,
    #[smi] local_port: u32,
) -> Result<String, JsErrorBox> {
    let shutdown = {
        let mut st = state.borrow_mut();
        ensure_enabled(&mut st)?;
        shutdown_token(&st)?
    };
    let ip = loopback_host(&host, host.contains(':'))?;
    let addr = SocketAddr::new(ip, port as u16);
    // Optional local bind (Node's localAddress/localPort connect options);
    // pinned to loopback like every other bind in this module.
    let local = if local_host.is_empty() && local_port == 0 {
        None
    } else {
        let bind_host = if local_host.is_empty() {
            "127.0.0.1".to_string()
        } else {
            local_host
        };
        let local_ip = loopback_host(&bind_host, bind_host.contains(':'))?;
        Some(SocketAddr::new(local_ip, local_port as u16))
    };
    let stream = tokio::spawn(async move {
        let connect = async move {
            match local {
                None => TcpStream::connect(addr).await,
                Some(local_addr) => {
                    let socket = if addr.is_ipv6() {
                        tokio::net::TcpSocket::new_v6()?
                    } else {
                        tokio::net::TcpSocket::new_v4()?
                    };
                    socket.bind(local_addr)?;
                    socket.connect(addr).await
                }
            }
        };
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => Ok(Err(std::io::Error::other("net: shutting down"))),
            connected = tokio::time::timeout(
                std::time::Duration::from_secs(30), connect) => connected,
        }
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("net task join error: {e}")))?
    .map_err(|_| JsErrorBox::generic(format!("net: connect to {addr} timed out")))?
    .map_err(|e| {
        let code = match e.kind() {
            std::io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
            std::io::ErrorKind::AddrInUse => "EADDRINUSE",
            std::io::ErrorKind::AddrNotAvailable => "EADDRNOTAVAIL",
            std::io::ErrorKind::TimedOut => "ETIMEDOUT",
            _ => "EIO",
        };
        JsErrorBox::generic(format!("net: connect to {addr} failed: {e} ({code})"))
    })?;
    Ok(register_stream(&state, stream)?.to_string())
}

/// Async op: read up to 64KB. Returns `{"data": <b64>}`, `{"eof": true}`
/// at end of stream, or `{"closed": true}` if the socket was dropped
/// mid-read.
#[op2]
#[string]
async fn op_tcp_read(state: Rc<RefCell<OpState>>, #[smi] rid: u32) -> Result<String, JsErrorBox> {
    let (StreamHandle { read, cancel, .. }, shutdown) = get_stream(&state, rid)?;
    let result = tokio::spawn(async move {
        let mut guard = read.lock().await;
        let mut buf = vec![0u8; 64 * 1024];
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(None),
            _ = shutdown.cancelled() => Ok(None),
            n = guard.read(&mut buf) => n.map(|n| { buf.truncate(n); Some(buf) }),
        }
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("net task join error: {e}")))?;

    match result {
        Ok(None) => Ok(serde_json::json!({"closed": true}).to_string()),
        Ok(Some(bytes)) if bytes.is_empty() => Ok(serde_json::json!({"eof": true}).to_string()),
        Ok(Some(bytes)) => {
            Ok(serde_json::json!({"data": b64_encode(&bytes)}).to_string())
        }
        Err(e) => {
            let code = match e.kind() {
                std::io::ErrorKind::ConnectionReset => "ECONNRESET",
                _ => "EIO",
            };
            Ok(serde_json::json!({"error": e.to_string(), "code": code}).to_string())
        }
    }
}

/// Async op: write all bytes (base64 payload).
#[op2]
async fn op_tcp_write(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: u32,
    #[string] data: String,
) -> Result<(), JsErrorBox> {
    let (StreamHandle { write, .. }, shutdown) = get_stream(&state, rid)?;
    let bytes = b64_decode(&data).map_err(JsErrorBox::generic)?;
    tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => Ok(()),
            written = async { write.lock().await.write_all(&bytes).await } => written,
        }
    })
    .await
        .map_err(|e| JsErrorBox::generic(format!("net task join error: {e}")))?
        .map_err(|e| {
            let code = match e.kind() {
                std::io::ErrorKind::BrokenPipe => "EPIPE",
                std::io::ErrorKind::ConnectionReset => "ECONNRESET",
                _ => "EIO",
            };
            JsErrorBox::generic(format!("net: write failed: {e} ({code})"))
        })
}

/// Async op: half-close the write side (FIN), as `socket.end()` does.
#[op2]
async fn op_tcp_shutdown(state: Rc<RefCell<OpState>>, #[smi] rid: u32) -> Result<(), JsErrorBox> {
    let (StreamHandle { write, .. }, _) = get_stream(&state, rid)?;
    tokio::spawn(async move { write.lock().await.shutdown().await })
        .await
        .map_err(|e| JsErrorBox::generic(format!("net task join error: {e}")))?
        // Peer may already be gone; a failed FIN is not worth surfacing.
        .or(Ok(()))
}

/// Fast op: drop a stream, waking any pending read.
#[op2(fast)]
fn op_tcp_close_stream(state: &mut OpState, #[smi] rid: u32) {
    if let Some(registry) = state.try_borrow_mut::<TcpRegistry>() {
        if let Some(handle) = registry.streams.remove(&rid) {
            handle.cancel.cancel();
        }
    }
}

/// Fast op: drop a listener, waking any pending accept.
#[op2(fast)]
fn op_tcp_close_listener(state: &mut OpState, #[smi] rid: u32) {
    if let Some(registry) = state.try_borrow_mut::<TcpRegistry>() {
        if let Some(handle) = registry.listeners.remove(&rid) {
            handle.cancel.cancel();
        }
    }
}

// ── UDP (node:dgram) ────────────────────────────────────────────────────

fn get_udp(
    state: &Rc<RefCell<OpState>>,
    rid: u32,
) -> Result<(UdpHandle, CancellationToken), JsErrorBox> {
    let state = state.borrow();
    let shutdown = shutdown_token(&state)?;
    state
        .try_borrow::<TcpRegistry>()
        .and_then(|r| r.udp.get(&rid).cloned())
        .map(|handle| (handle, shutdown))
        .ok_or_else(|| JsErrorBox::generic(format!("dgram: unknown socket id {rid}")))
}

/// Sync op: bind a loopback UDP socket. Returns `{rid, address, port, family}`.
#[op2]
#[string]
fn op_udp_bind(
    state: &mut OpState,
    #[string] host: String,
    #[smi] port: u32,
) -> Result<String, JsErrorBox> {
    ensure_enabled(state)?;
    let ip = loopback_host(&host, host.contains(':'))?;
    let addr = SocketAddr::new(ip, port as u16);
    let std_socket = std::net::UdpSocket::bind(addr).map_err(|e| {
        JsErrorBox::generic(format!(
            "dgram: bind on {addr} failed: {e} ({})",
            match e.kind() {
                std::io::ErrorKind::AddrInUse => "EADDRINUSE",
                std::io::ErrorKind::PermissionDenied => "EACCES",
                _ => "EIO",
            }
        ))
    })?;
    std_socket
        .set_nonblocking(true)
        .map_err(|e| JsErrorBox::generic(format!("dgram: set_nonblocking failed: {e}")))?;
    let socket = tokio::net::UdpSocket::from_std(std_socket)
        .map_err(|e| JsErrorBox::generic(format!("dgram: socket setup failed: {e}")))?;
    let local = socket
        .local_addr()
        .map_err(|e| JsErrorBox::generic(format!("dgram: local_addr failed: {e}")))?;
    let registry = ensure_enabled(state)?;
    let rid = registry.next_id;
    registry.next_id += 1;
    registry.udp.insert(
        rid,
        UdpHandle {
            socket: Arc::new(socket),
            cancel: CancellationToken::new(),
        },
    );
    Ok(serde_json::json!({
        "rid": rid,
        "address": local.ip().to_string(),
        "port": local.port(),
        "family": if local.is_ipv6() { "IPv6" } else { "IPv4" },
    })
    .to_string())
}

/// Async op: send one datagram to a loopback target (base64 payload).
#[op2]
async fn op_udp_send(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: u32,
    #[string] host: String,
    #[smi] port: u32,
    #[string] data: String,
) -> Result<(), JsErrorBox> {
    let (UdpHandle { socket, .. }, _) = get_udp(&state, rid)?;
    let ip = loopback_host(&host, host.contains(':'))?;
    let addr = SocketAddr::new(ip, port as u16);
    let bytes = b64_decode(&data).map_err(JsErrorBox::generic)?;
    tokio::spawn(async move { socket.send_to(&bytes, addr).await })
        .await
        .map_err(|e| JsErrorBox::generic(format!("net task join error: {e}")))?
        .map_err(|e| {
            let code = match e.raw_os_error() {
                Some(90) => "EMSGSIZE",
                Some(13) => "EACCES",
                _ => match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
                    _ => "EIO",
                },
            };
            JsErrorBox::generic(format!("dgram: send failed: {e} ({code})"))
        })?;
    Ok(())
}

/// Async op: receive one datagram. Returns
/// `{"data": <b64>, "address": ..., "port": ..., "family": ...}` or
/// `{"closed": true}` once the socket is dropped.
#[op2]
#[string]
async fn op_udp_recv(state: Rc<RefCell<OpState>>, #[smi] rid: u32) -> Result<String, JsErrorBox> {
    let (UdpHandle { socket, cancel }, shutdown) = get_udp(&state, rid)?;
    let result = tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(None),
            _ = shutdown.cancelled() => Ok(None),
            received = socket.recv_from(&mut buf) => {
                received.map(|(n, from)| { buf.truncate(n); Some((buf, from)) })
            }
        }
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("net task join error: {e}")))?;

    match result {
        Ok(None) => Ok(serde_json::json!({"closed": true}).to_string()),
        Ok(Some((bytes, from))) => Ok(serde_json::json!({
            "data": b64_encode(&bytes),
            "address": from.ip().to_string(),
            "port": from.port(),
            "family": if from.is_ipv6() { "IPv6" } else { "IPv4" },
        })
        .to_string()),
        Err(e) => Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    }
}

/// Fast op: drop a UDP socket, waking any pending recv.
#[op2(fast)]
fn op_udp_close(state: &mut OpState, #[smi] rid: u32) {
    if let Some(registry) = state.try_borrow_mut::<TcpRegistry>() {
        if let Some(handle) = registry.udp.remove(&rid) {
            handle.cancel.cancel();
        }
    }
}

/// Async op: hard-reset a stream. Cancels in-flight I/O, then reunites the
/// halves and closes with SO_LINGER=0 so the peer sees a real RST
/// (ECONNRESET) rather than a graceful FIN. Best effort: if a half is still
/// held by an unfinished task after a short grace period, the stream is
/// simply dropped (plain close).
#[op2]
async fn op_tcp_reset(state: Rc<RefCell<OpState>>, #[smi] rid: u32) -> Result<(), JsErrorBox> {
    let handle = {
        let mut st = state.borrow_mut();
        let registry = ensure_enabled(&mut st)?;
        registry.streams.remove(&rid)
    };
    let Some(handle) = handle else {
        return Ok(());
    };
    handle.cancel.cancel();
    let StreamHandle { read, write, .. } = handle;
    for _ in 0..100 {
        if Arc::strong_count(&read) == 1 && Arc::strong_count(&write) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    if let (Ok(read_mutex), Ok(write_mutex)) = (Arc::try_unwrap(read), Arc::try_unwrap(write)) {
        let read_half = read_mutex.into_inner();
        let write_half = write_mutex.into_inner();
        if let Ok(stream) = read_half.reunite(write_half) {
            let _ = stream.set_linger(Some(std::time::Duration::from_secs(0)));
        }
    }
    Ok(())
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    net_tcp_ext,
    ops = [
        op_tcp_listen,
        op_tcp_accept,
        op_tcp_connect,
        op_tcp_read,
        op_tcp_write,
        op_tcp_shutdown,
        op_tcp_reset,
        op_tcp_close_stream,
        op_tcp_close_listener,
        op_udp_bind,
        op_udp_send,
        op_udp_recv,
        op_udp_close,
    ],
);

pub fn create_extension() -> deno_core::Extension {
    net_tcp_ext::init()
}

/// Expose the op table to the `node:net` shim. The shim is an ES module
/// served by the module loader, which cannot reach `Deno.core` after
/// hardening freezes it — so the ops are captured onto a hidden global,
/// mirroring the http2 binding.
const NET_BINDING_JS: &str = r#"
(function () {
    var ops = Deno.core.ops;
    Object.defineProperty(globalThis, '__mcpV8NetOps', {
        value: Object.freeze({
            listen: ops.op_tcp_listen,
            accept: ops.op_tcp_accept,
            connect: ops.op_tcp_connect,
            read: ops.op_tcp_read,
            write: ops.op_tcp_write,
            shutdown: ops.op_tcp_shutdown,
            reset: ops.op_tcp_reset,
            closeStream: ops.op_tcp_close_stream,
            closeListener: ops.op_tcp_close_listener,
            udpBind: ops.op_udp_bind,
            udpSend: ops.op_udp_send,
            udpRecv: ops.op_udp_recv,
            udpClose: ops.op_udp_close,
            unrefOpPromise: Deno.core.unrefOpPromise,
            refOpPromise: Deno.core.refOpPromise,
        }),
        writable: false, enumerable: false, configurable: false,
    });
})();
"#;

pub fn inject_net_tcp(runtime: &mut deno_core::JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<net-tcp-setup>", NET_BINDING_JS.to_string())
        .map_err(|e| format!("Failed to install net binding: {e}"))?;
    Ok(())
}
