/// Official gRPC interoperability test cases, driven by stock
/// `@grpc/grpc-js` running unmodified inside the sandbox.
///
/// The server side is an in-process `h2::server` implementing
/// `grpc.testing.TestService` semantics (messages hand-encoded/decoded —
/// the wire format is what matters, not codegen): EmptyCall, UnaryCall with
/// response sizes and EchoStatus, StreamingInputCall, StreamingOutputCall,
/// FullDuplexCall, custom-metadata echo, and unimplemented-method handling,
/// per https://github.com/grpc/grpc/blob/master/doc/interop-test-descriptions.md
///
/// The client side imports `npm:@grpc/grpc-js?target=node` through the
/// module loader (esm.sh) and builds a client with
/// `makeGenericClientConstructor` + hand protobuf serializers, then runs the
/// official case list. Network-dependent tests are `#[ignore]`d, matching
/// this repo's convention (see module_imports.rs); run them with
/// `cargo test --test grpc_interop -- --ignored`.

use std::sync::{Arc, Once};

use bytes::Bytes;
use server::engine::execution::ExecutionRegistry;
use server::engine::http2::Http2Config;
use server::engine::module_loader::ModuleLoaderConfig;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

// ── Minimal protobuf wire helpers ────────────────────────────────────────

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

fn write_len_delim(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    write_varint(out, (field << 3) | 2);
    write_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn write_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    write_varint(out, field << 3);
    write_varint(out, value);
}

enum ProtoValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

/// Iterate `(field_number, value)` pairs, skipping unknown wire types.
fn proto_fields<'a>(buf: &'a [u8]) -> Vec<(u64, ProtoValue<'a>)> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let Some(key) = read_varint(buf, &mut pos) else { break };
        let field = key >> 3;
        match key & 0x7 {
            0 => {
                let Some(v) = read_varint(buf, &mut pos) else { break };
                fields.push((field, ProtoValue::Varint(v)));
            }
            2 => {
                let Some(len) = read_varint(buf, &mut pos) else { break };
                let len = len as usize;
                if pos + len > buf.len() {
                    break;
                }
                fields.push((field, ProtoValue::Bytes(&buf[pos..pos + len])));
                pos += len;
            }
            5 => pos += 4,
            1 => pos += 8,
            _ => break,
        }
    }
    fields
}

#[derive(Default, Debug)]
struct EchoStatus {
    code: u64,
    message: String,
}

fn parse_echo_status(buf: &[u8]) -> EchoStatus {
    let mut status = EchoStatus::default();
    for (field, value) in proto_fields(buf) {
        match (field, value) {
            (1, ProtoValue::Varint(v)) => status.code = v,
            (2, ProtoValue::Bytes(b)) => status.message = String::from_utf8_lossy(b).to_string(),
            _ => {}
        }
    }
    status
}

fn payload_body_len(buf: &[u8]) -> usize {
    // Payload { bytes body = 2; }
    proto_fields(buf)
        .into_iter()
        .find_map(|(field, value)| match (field, value) {
            (2, ProtoValue::Bytes(b)) => Some(b.len()),
            _ => None,
        })
        .unwrap_or(0)
}

/// SimpleRequest → (response_size, response_status)
fn parse_simple_request(buf: &[u8]) -> (usize, Option<EchoStatus>) {
    let mut size = 0;
    let mut status = None;
    for (field, value) in proto_fields(buf) {
        match (field, value) {
            (2, ProtoValue::Varint(v)) => size = v as usize,
            (7, ProtoValue::Bytes(b)) => status = Some(parse_echo_status(b)),
            _ => {}
        }
    }
    (size, status)
}

/// StreamingOutputCallRequest → (response sizes, response_status)
fn parse_streaming_output_request(buf: &[u8]) -> (Vec<usize>, Option<EchoStatus>) {
    let mut sizes = Vec::new();
    let mut status = None;
    for (field, value) in proto_fields(buf) {
        match (field, value) {
            (2, ProtoValue::Bytes(params)) => {
                // ResponseParameters { int32 size = 1; }
                for (pfield, pvalue) in proto_fields(params) {
                    if let (1, ProtoValue::Varint(v)) = (pfield, pvalue) {
                        sizes.push(v as usize);
                    }
                }
            }
            (7, ProtoValue::Bytes(b)) => status = Some(parse_echo_status(b)),
            _ => {}
        }
    }
    (sizes, status)
}

/// StreamingInputCallRequest → payload body length
fn parse_streaming_input_request(buf: &[u8]) -> usize {
    proto_fields(buf)
        .into_iter()
        .find_map(|(field, value)| match (field, value) {
            (1, ProtoValue::Bytes(b)) => Some(payload_body_len(b)),
            _ => None,
        })
        .unwrap_or(0)
}

fn encode_payload_message(outer_field: u64, body_len: usize) -> Vec<u8> {
    // <outer> { Payload { type = 1 (COMPRESSABLE=0, omitted); bytes body = 2; } }
    let mut payload = Vec::with_capacity(body_len + 8);
    write_len_delim(&mut payload, 2, &vec![0u8; body_len]);
    let mut out = Vec::with_capacity(payload.len() + 8);
    write_len_delim(&mut out, outer_field, &payload);
    out
}

fn grpc_frame(message: &[u8]) -> Bytes {
    let mut frame = Vec::with_capacity(message.len() + 5);
    frame.push(0);
    frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
    frame.extend_from_slice(message);
    Bytes::from(frame)
}

/// Incremental gRPC length-prefixed message parser.
#[derive(Default)]
struct FrameReader {
    buffer: Vec<u8>,
}

impl FrameReader {
    fn feed(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(chunk);
        let mut messages = Vec::new();
        loop {
            if self.buffer.len() < 5 {
                break;
            }
            let len = u32::from_be_bytes([
                self.buffer[1],
                self.buffer[2],
                self.buffer[3],
                self.buffer[4],
            ]) as usize;
            if self.buffer.len() < 5 + len {
                break;
            }
            messages.push(self.buffer[5..5 + len].to_vec());
            self.buffer.drain(..5 + len);
        }
        messages
    }
}

// ── grpc.testing.TestService server on h2 ───────────────────────────────

async fn send_all_data(
    send: &mut h2::SendStream<Bytes>,
    mut data: Bytes,
    end_stream: bool,
) -> Result<(), h2::Error> {
    if data.is_empty() {
        return send.send_data(data, end_stream);
    }
    while !data.is_empty() {
        send.reserve_capacity(data.len());
        let Some(granted) = futures::future::poll_fn(|cx| send.poll_capacity(cx)).await else {
            return Ok(()); // stream gone (cancelled); nothing to do
        };
        let granted = granted?;
        let chunk = data.split_to(granted.min(data.len()));
        let is_last = data.is_empty() && end_stream;
        send.send_data(chunk, is_last)?;
    }
    Ok(())
}

fn trailers(status: u64, message: &str, trailing_bin: Option<&str>) -> http::HeaderMap {
    let mut map = http::HeaderMap::new();
    map.insert("grpc-status", status.to_string().parse().unwrap());
    if !message.is_empty() {
        // Values here are ASCII in the interop cases; percent-encoding of
        // exotic characters is not needed.
        map.insert("grpc-message", message.parse().unwrap());
    }
    if let Some(value) = trailing_bin {
        map.insert("x-grpc-test-echo-trailing-bin", value.parse().unwrap());
    }
    map
}

async fn start_test_service() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let Ok(mut conn) = h2::server::handshake(tcp).await else { return };
                while let Some(Ok((request, respond))) = conn.accept().await {
                    tokio::spawn(handle_stream(request, respond));
                }
            });
        }
    });

    addr
}

async fn handle_stream(
    request: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
) {
    let path = request.uri().path().to_string();
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned)
    };
    let echo_initial = header("x-grpc-test-echo-initial");
    let echo_trailing = header("x-grpc-test-echo-trailing-bin");

    let known = matches!(
        path.as_str(),
        "/grpc.testing.TestService/EmptyCall"
            | "/grpc.testing.TestService/UnaryCall"
            | "/grpc.testing.TestService/StreamingInputCall"
            | "/grpc.testing.TestService/StreamingOutputCall"
            | "/grpc.testing.TestService/FullDuplexCall"
    );
    if !known {
        // Trailers-only UNIMPLEMENTED, the official unimplemented_method shape.
        let response = http::Response::builder()
            .status(200)
            .header("content-type", "application/grpc")
            .header("grpc-status", "12")
            .body(())
            .unwrap();
        let _ = respond.send_response(response, true);
        // Drain the request body so dropping it does not reset the stream
        // before the client processes the trailers-only response.
        let mut body = request.into_body();
        while let Some(chunk) = futures::future::poll_fn(|cx| body.poll_data(cx)).await {
            let Ok(chunk) = chunk else { break };
            let _ = body.flow_control().release_capacity(chunk.len());
        }
        return;
    }

    let mut response = http::Response::builder()
        .status(200)
        .header("content-type", "application/grpc");
    if let Some(value) = &echo_initial {
        response = response.header("x-grpc-test-echo-initial", value.as_str());
    }
    let Ok(mut send) = respond.send_response(response.body(()).unwrap(), false) else {
        return;
    };

    let mut body = request.into_body();
    let mut reader = FrameReader::default();
    let mut input_total: usize = 0;

    // Deliver messages as they arrive (FullDuplexCall must answer each
    // request before the next one is sent — the ping_pong case).
    loop {
        let data = futures::future::poll_fn(|cx| body.poll_data(cx)).await;
        let end = match data {
            Some(Ok(chunk)) => {
                let _ = body.flow_control().release_capacity(chunk.len());
                for message in reader.feed(&chunk) {
                    match path.as_str() {
                        "/grpc.testing.TestService/EmptyCall" => {
                            if send_all_data(&mut send, grpc_frame(&[]), false).await.is_err() {
                                return;
                            }
                            let _ = send.send_trailers(trailers(0, "", echo_trailing.as_deref()));
                            return;
                        }
                        "/grpc.testing.TestService/UnaryCall" => {
                            let (size, status) = parse_simple_request(&message);
                            if let Some(status) = status {
                                let _ = send.send_trailers(trailers(
                                    status.code,
                                    &status.message,
                                    echo_trailing.as_deref(),
                                ));
                                return;
                            }
                            let reply = encode_payload_message(1, size);
                            if send_all_data(&mut send, grpc_frame(&reply), false).await.is_err() {
                                return;
                            }
                            let _ = send.send_trailers(trailers(0, "", echo_trailing.as_deref()));
                            return;
                        }
                        "/grpc.testing.TestService/StreamingInputCall" => {
                            input_total += parse_streaming_input_request(&message);
                        }
                        "/grpc.testing.TestService/StreamingOutputCall"
                        | "/grpc.testing.TestService/FullDuplexCall" => {
                            let (sizes, status) = parse_streaming_output_request(&message);
                            if let Some(status) = status {
                                let _ = send.send_trailers(trailers(
                                    status.code,
                                    &status.message,
                                    echo_trailing.as_deref(),
                                ));
                                return;
                            }
                            for size in sizes {
                                let reply = encode_payload_message(1, size);
                                if send_all_data(&mut send, grpc_frame(&reply), false)
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            if path.ends_with("StreamingOutputCall") {
                                let _ =
                                    send.send_trailers(trailers(0, "", echo_trailing.as_deref()));
                                return;
                            }
                        }
                        _ => {}
                    }
                }
                false
            }
            Some(Err(_)) => return,
            None => true,
        };
        if end {
            break;
        }
    }

    // Client half-closed.
    match path.as_str() {
        "/grpc.testing.TestService/StreamingInputCall" => {
            // StreamingInputCallResponse { int32 aggregated_payload_size = 1; }
            let mut reply = Vec::new();
            write_varint_field(&mut reply, 1, input_total as u64);
            if send_all_data(&mut send, grpc_frame(&reply), false).await.is_err() {
                return;
            }
            let _ = send.send_trailers(trailers(0, "", echo_trailing.as_deref()));
        }
        _ => {
            // EmptyCall/FullDuplexCall (and empty_stream): finish OK.
            let _ = send.send_trailers(trailers(0, "", echo_trailing.as_deref()));
        }
    }
}

// ── Engine plumbing ─────────────────────────────────────────────────────

fn build_engine(allow_external_modules: bool) -> Engine {
    let chain = Arc::new(PolicyChain::new(vec![], EvalMode::All));
    let tmp = std::env::temp_dir().join(format!(
        "mcp-grpc-interop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");

    Engine::new_stateless(256 * 1024 * 1024, 420, 4)
        .with_http2_config(Http2Config::new_with_chain(chain))
        .with_module_loader_config(ModuleLoaderConfig {
            allow_external: allow_external_modules,
            policy_chain: None,
        })
        .with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: String) -> Result<String, String> {
    let exec_id = engine
        .run_js(code)
        .execute()
        .await
        .map_err(|error| format!("submit should succeed: {error}"))?;

    for _ in 0..12000 {
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

// ── Hermetic: the new node compat shims load and behave ─────────────────

#[tokio::test]
async fn node_compat_shims_smoke() {
    ensure_v8();
    let engine = build_engine(false);

    let code = r#"
        import net from 'node:net';
        import tls from 'node:tls';
        import dns from 'node:dns';
        import fs from 'node:fs';
        import http from 'node:http';
        import zlib from 'node:zlib';
        import { Readable, Writable, Duplex } from 'node:stream';
        import { Buffer } from 'node:buffer';
        import { promisify } from 'node:util';

        if (!net.isIPv4('10.0.0.1')) throw new Error('isIPv4');
        if (net.isIPv4('256.0.0.1')) throw new Error('isIPv4 range');
        if (!net.isIPv6('::1')) throw new Error('isIPv6');
        if (net.isIP('example.com') !== 0) throw new Error('isIP hostname');

        if (typeof tls.createSecureContext({}).context !== 'object') throw new Error('tls ctx');
        if (tls.checkServerIdentity('h', {}) !== undefined) throw new Error('tls csi');

        const addr = await dns.promises.lookup('svc.example');
        if (addr.address !== 'svc.example') throw new Error('dns passthrough: ' + addr.address);
        const txt = await dns.promises.resolveTxt('svc.example');
        if (txt.length !== 0) throw new Error('dns txt');

        if (fs.existsSync('/etc/passwd') !== false) throw new Error('fs existsSync');
        if (typeof http.globalAgent !== 'object') throw new Error('http agent');

        // Object-mode Writable: serial _write with callbacks, then _final.
        const written = [];
        let finalized = false;
        const w = new (class extends Writable {
            _write(chunk, _enc, cb) { written.push(chunk); setTimeout(cb, 1); }
            _final(cb) { finalized = true; cb(); }
        })({ objectMode: true });
        const finished = new Promise((resolve) => w.on('finish', resolve));
        w.write('a'); w.write('b'); w.end('c');
        await finished;
        if (written.join('') !== 'abc') throw new Error('writable order: ' + written.join(''));
        if (!finalized) throw new Error('writable _final');

        // Object-mode Readable: push + flowing data + end.
        const r = new Readable({ objectMode: true, read() {} });
        const got = [];
        const ended = new Promise((resolve) => r.on('end', resolve));
        r.on('data', (item) => got.push(item));
        r.push(1); r.push(2); r.push(null);
        await ended;
        if (got.join(',') !== '1,2') throw new Error('readable flow: ' + got.join(','));

        // Duplex round trip.
        const d = new (class extends Duplex {
            _read() {}
            _write(chunk, _enc, cb) { this.push(chunk.toUpperCase()); cb(); }
        })({ objectMode: true });
        const dGot = [];
        d.on('data', (item) => dGot.push(item));
        d.write('x'); d.write('y');
        await new Promise((resolve) => setTimeout(resolve, 10));
        if (dGot.join('') !== 'XY') throw new Error('duplex: ' + dGot.join(''));

        // zlib round trip over CompressionStream.
        const gzip = promisify(zlib.gzip);
        const gunzip = promisify(zlib.gunzip);
        const original = Buffer.from('hello '.repeat(100));
        const packed = await gzip(original);
        if (packed.length >= original.length) throw new Error('gzip did not compress');
        const unpacked = await gunzip(packed);
        if (!unpacked.equals(original)) throw new Error('gzip round trip');
    "#
    .to_string();

    run_js(&engine, code).await.expect("node compat shims should work");
}

// ── The official interop cases with stock @grpc/grpc-js ─────────────────

const GRPC_JS_SPECIFIER: &str = "npm:@grpc/grpc-js@1.12.6?target=node";

fn interop_client_js(addr: &str) -> String {
    let prelude = format!(
        r#"
// Node-global bridge: packages built for Node reference these as globals.
globalThis.Buffer = (await import('node:buffer')).Buffer;
globalThis.process = (await import('node:process')).default;
const grpcModule = await import('{GRPC_JS_SPECIFIER}');
const grpc = grpcModule.default && grpcModule.default.makeGenericClientConstructor
    ? grpcModule.default : grpcModule;
const SERVER = '{addr}';
"#
    );

    let body = r#"
// ── protobuf wire helpers (grpc.testing messages, hand-encoded) ─────────
function varintBytes(n) {
    const out = [];
    let v = n >>> 0;
    while (v > 0x7f) { out.push((v & 0x7f) | 0x80); v >>>= 7; }
    out.push(v);
    return Buffer.from(out);
}
function lenDelim(field, bytes) {
    return Buffer.concat([varintBytes((field << 3) | 2), varintBytes(bytes.length), Buffer.from(bytes)]);
}
function varintField(field, value) {
    return Buffer.concat([varintBytes(field << 3), varintBytes(value)]);
}
function payloadMessage(size) {
    return lenDelim(2, Buffer.alloc(size));
}
function echoStatusMessage(code, message) {
    return Buffer.concat([varintField(1, code), lenDelim(2, Buffer.from(message, 'utf8'))]);
}

function readFields(buf) {
    const fields = [];
    let pos = 0;
    function varint() {
        let result = 0, shift = 0;
        for (;;) {
            const byte = buf[pos++];
            result += (byte & 0x7f) * Math.pow(2, shift);
            if ((byte & 0x80) === 0) return result;
            shift += 7;
        }
    }
    while (pos < buf.length) {
        const key = varint();
        const field = key >>> 3;
        const wire = key & 7;
        if (wire === 0) fields.push([field, varint()]);
        else if (wire === 2) {
            const len = varint();
            fields.push([field, buf.subarray(pos, pos + len)]);
            pos += len;
        } else if (wire === 5) pos += 4;
        else if (wire === 1) pos += 8;
        else break;
    }
    return fields;
}
function payloadBodyLen(buf) {
    for (const [field, value] of readFields(buf)) {
        if (field === 2 && typeof value !== 'number') return value.length;
    }
    return 0;
}

const encEmpty = () => Buffer.alloc(0);
const decEmpty = () => ({});
function encSimpleRequest({ responseSize = 0, payloadSize = 0, echoStatus = null } = {}) {
    const parts = [];
    if (responseSize) parts.push(varintField(2, responseSize));
    if (payloadSize) parts.push(lenDelim(3, payloadMessage(payloadSize)));
    if (echoStatus) parts.push(lenDelim(7, echoStatusMessage(echoStatus.code, echoStatus.message)));
    return Buffer.concat(parts);
}
function decSimpleResponse(buf) {
    for (const [field, value] of readFields(buf)) {
        if (field === 1 && typeof value !== 'number') return { payloadLen: payloadBodyLen(value) };
    }
    return { payloadLen: 0 };
}
function encStreamingOutputRequest({ sizes = [], payloadSize = 0, echoStatus = null } = {}) {
    const parts = [];
    for (const size of sizes) parts.push(lenDelim(2, varintField(1, size)));
    if (payloadSize) parts.push(lenDelim(3, payloadMessage(payloadSize)));
    if (echoStatus) parts.push(lenDelim(7, echoStatusMessage(echoStatus.code, echoStatus.message)));
    return Buffer.concat(parts);
}
const decStreamingOutputResponse = decSimpleResponse;
function encStreamingInputRequest({ payloadSize = 0 } = {}) {
    return payloadSize ? lenDelim(1, payloadMessage(payloadSize)) : Buffer.alloc(0);
}
function decStreamingInputResponse(buf) {
    for (const [field, value] of readFields(buf)) {
        if (field === 1 && typeof value === 'number') return { aggregated: value };
    }
    return { aggregated: 0 };
}

// ── stock grpc-js client via makeGenericClientConstructor ───────────────
const service = {
    EmptyCall: {
        path: '/grpc.testing.TestService/EmptyCall',
        requestStream: false, responseStream: false,
        requestSerialize: encEmpty, responseDeserialize: decEmpty,
    },
    UnaryCall: {
        path: '/grpc.testing.TestService/UnaryCall',
        requestStream: false, responseStream: false,
        requestSerialize: encSimpleRequest, responseDeserialize: decSimpleResponse,
    },
    StreamingInputCall: {
        path: '/grpc.testing.TestService/StreamingInputCall',
        requestStream: true, responseStream: false,
        requestSerialize: encStreamingInputRequest, responseDeserialize: decStreamingInputResponse,
    },
    StreamingOutputCall: {
        path: '/grpc.testing.TestService/StreamingOutputCall',
        requestStream: false, responseStream: true,
        requestSerialize: encStreamingOutputRequest, responseDeserialize: decStreamingOutputResponse,
    },
    FullDuplexCall: {
        path: '/grpc.testing.TestService/FullDuplexCall',
        requestStream: true, responseStream: true,
        requestSerialize: encStreamingOutputRequest, responseDeserialize: decStreamingOutputResponse,
    },
    UnimplementedCall: {
        path: '/grpc.testing.TestService/UnimplementedCall',
        requestStream: false, responseStream: false,
        requestSerialize: encEmpty, responseDeserialize: decEmpty,
    },
};
const TestClient = grpc.makeGenericClientConstructor(service, 'TestService');
const client = new TestClient(SERVER, grpc.credentials.createInsecure());

function unaryFull(method, request, metadata, options) {
    return new Promise((resolve) => {
        let err = null, value = null, initial = null;
        const call = client[method](
            request, metadata || new grpc.Metadata(), options || {},
            (e, v) => { err = e; value = v; });
        call.on('metadata', (m) => { initial = m; });
        call.on('status', (status) => resolve({ err, value, initial, status }));
    });
}

function assert(cond, label) {
    if (!cond) throw new Error(label);
}

// ── the official cases ──────────────────────────────────────────────────
const results = [];
// Each case is bounded: a case that never settles fails itself rather than
// wedging the whole suite (and tells us which one).
const CASE_TIMEOUT_MS = 10000;
async function runCase(name, fn) {
    let timer;
    try {
        await Promise.race([
            fn(),
            new Promise((_resolve, reject) => {
                timer = setTimeout(
                    () => reject(new Error('case timed out after ' + CASE_TIMEOUT_MS + 'ms')),
                    CASE_TIMEOUT_MS);
            }),
        ]);
        results.push(name + ': PASS');
    } catch (e) {
        results.push(name + ': FAIL ' + (e && e.message ? e.message : String(e)));
    } finally {
        clearTimeout(timer);
    }
}

await runCase('empty_unary', async () => {
    const { err, status } = await unaryFull('EmptyCall', {});
    assert(!err, 'unexpected error: ' + (err && err.message));
    assert(status.code === 0, 'status ' + status.code);
});

await runCase('large_unary', async () => {
    const { err, value, status } = await unaryFull('UnaryCall', {
        responseSize: 314159, payloadSize: 271828,
    });
    assert(!err, 'unexpected error: ' + (err && err.message));
    assert(status.code === 0, 'status ' + status.code);
    assert(value.payloadLen === 314159, 'payload len ' + value.payloadLen);
});

await runCase('custom_metadata', async () => {
    const ECHO_INITIAL = 'test_initial_metadata_value';
    const ECHO_TRAILING = Buffer.from([0xab, 0xab, 0xab]);
    const metadata = new grpc.Metadata();
    metadata.set('x-grpc-test-echo-initial', ECHO_INITIAL);
    metadata.set('x-grpc-test-echo-trailing-bin', ECHO_TRAILING);
    const { err, initial, status } = await unaryFull(
        'UnaryCall', { responseSize: 314159, payloadSize: 271828 }, metadata);
    assert(!err, 'unexpected error: ' + (err && err.message));
    const initialEcho = initial && initial.get('x-grpc-test-echo-initial');
    assert(initialEcho && initialEcho[0] === ECHO_INITIAL,
        'initial metadata echo: ' + JSON.stringify(initialEcho));
    const trailingEcho = status.metadata.get('x-grpc-test-echo-trailing-bin');
    assert(trailingEcho && trailingEcho[0] && ECHO_TRAILING.equals(trailingEcho[0]),
        'trailing metadata echo: ' + JSON.stringify(trailingEcho));
});

await runCase('status_code_and_message', async () => {
    const MESSAGE = 'test status message';
    const { err } = await unaryFull('UnaryCall', {
        echoStatus: { code: 2, message: MESSAGE },
    });
    assert(err, 'expected an error');
    assert(err.code === 2, 'code ' + err.code);
    assert(err.details === MESSAGE, 'details "' + err.details + '"');

    // Full-duplex variant of the same case.
    const call = client.FullDuplexCall();
    const duplexStatus = await new Promise((resolve) => {
        let e = null;
        call.on('error', (error) => { e = error; });
        call.on('status', (s) => resolve({ error: e, status: s }));
        call.write({ echoStatus: { code: 2, message: MESSAGE } });
        call.end();
    });
    assert(duplexStatus.status.code === 2, 'duplex code ' + duplexStatus.status.code);
    assert(duplexStatus.status.details === MESSAGE,
        'duplex details "' + duplexStatus.status.details + '"');
});

await runCase('unimplemented_method', async () => {
    const { err, status } = await unaryFull('UnimplementedCall', {});
    assert(err, 'expected an error');
    assert(status.code === 12, 'status ' + status.code);
});

await runCase('server_streaming', async () => {
    const SIZES = [31415, 9, 2653, 58979];
    const call = client.StreamingOutputCall({ sizes: SIZES });
    const received = [];
    const status = await new Promise((resolve, reject) => {
        call.on('data', (message) => received.push(message.payloadLen));
        call.on('error', reject);
        call.on('status', resolve);
    });
    assert(status.code === 0, 'status ' + status.code);
    assert(received.join(',') === SIZES.join(','), 'sizes ' + received.join(','));
});

await runCase('client_streaming', async () => {
    const result = await new Promise((resolve, reject) => {
        const call = client.StreamingInputCall((err, value) => {
            if (err) reject(err); else resolve(value);
        });
        for (const size of [27182, 8, 1828, 45904]) {
            call.write({ payloadSize: size });
        }
        call.end();
    });
    assert(result.aggregated === 74922, 'aggregated ' + result.aggregated);
});

await runCase('ping_pong', async () => {
    const REQUESTS = [
        { respSize: 31415, payload: 27182 },
        { respSize: 9, payload: 8 },
        { respSize: 2653, payload: 1828 },
        { respSize: 58979, payload: 45904 },
    ];
    const call = client.FullDuplexCall();
    const received = [];
    let resolveNext = null;
    call.on('data', (message) => {
        received.push(message.payloadLen);
        if (resolveNext) { const r = resolveNext; resolveNext = null; r(); }
    });
    const statusPromise = new Promise((resolve, reject) => {
        call.on('error', reject);
        call.on('status', resolve);
    });
    for (const round of REQUESTS) {
        const arrived = new Promise((resolve) => { resolveNext = resolve; });
        call.write({ sizes: [round.respSize], payloadSize: round.payload });
        await arrived; // response must come back before the next request
    }
    call.end();
    const status = await statusPromise;
    assert(status.code === 0, 'status ' + status.code);
    assert(received.join(',') === '31415,9,2653,58979', 'sizes ' + received.join(','));
});

await runCase('empty_stream', async () => {
    const call = client.FullDuplexCall();
    const received = [];
    const status = await new Promise((resolve, reject) => {
        call.on('data', (message) => received.push(message));
        call.on('error', reject);
        call.on('status', resolve);
        call.end();
    });
    assert(status.code === 0, 'status ' + status.code);
    assert(received.length === 0, 'received ' + received.length);
});

await runCase('cancel_after_begin', async () => {
    const status = await new Promise((resolve) => {
        const call = client.StreamingInputCall(() => {});
        call.on('error', () => {});
        call.on('status', resolve);
        setTimeout(() => call.cancel(), 20);
    });
    assert(status.code === 1, 'status ' + status.code);
});

await runCase('timeout_on_sleeping_server', async () => {
    const call = client.FullDuplexCall({ deadline: Date.now() + 1 });
    const status = await new Promise((resolve) => {
        call.on('error', () => {});
        call.on('status', resolve);
        try { call.write({ sizes: [31415], payloadSize: 27182 }); } catch (_e) {}
    });
    assert(status.code === 4, 'status ' + status.code);
});

client.close();
const failures = results.filter((line) => !line.endsWith(': PASS'));
if (failures.length > 0) {
    throw new Error('INTEROP FAILURES\n' + results.join('\n'));
}
// Surfaced by the Rust side so the gate's coverage is visible in CI output.
globalThis.__interopResults = results.join('\n');
console.log(globalThis.__interopResults);
"#;

    format!("{prelude}\n{body}")
}

/// Network-dependent (esm.sh): run with `cargo test --test grpc_interop -- --ignored`.
#[tokio::test]
#[ignore]
async fn grpc_interop_official_cases_with_stock_grpc_js() {
    ensure_v8();
    let addr = start_test_service().await;
    let engine = build_engine(true);

    let code = interop_client_js(&addr);
    match run_js(&engine, code).await {
        Ok(_) => println!(
            "official gRPC interop cases passed with stock @grpc/grpc-js \
             (empty_unary, large_unary, custom_metadata, status_code_and_message, \
             unimplemented_method, server_streaming, client_streaming, ping_pong, \
             empty_stream, cancel_after_begin, timeout_on_sleeping_server)"
        ),
        Err(error) => panic!("official interop cases should pass with stock @grpc/grpc-js: {error}"),
    }
}
