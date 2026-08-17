//! Proof that gRPC services — Modal's control plane specifically — are
//! reachable from sandboxed JS through plain `fetch`.
//!
//! Native gRPC is HTTP/2 POST with 5-byte-prefixed frames plus a
//! `te: trailers` header; none of that needs sockets or node:tls. The probe
//! parses Modal's `api.proto` at runtime with protobufjs, builds a
//! nice-grpc-shaped client over `fetch`, and injects it into the real
//! `modal` npm SDK as `cpClient` — the SDK's own transport (nice-grpc over
//! node TLS channels) never loads.
//!
//! Network test, run explicitly:
//!   cargo test --test modal_grpc -- --ignored --nocapture
//!
//! Without credentials the expected outcome is grpc-status 16
//! ("Token not found") on every call — proving encode → HTTP/2 → decode
//! round-trips into Modal's service. Set MODAL_TOKEN_ID/MODAL_TOKEN_SECRET
//! to exercise authenticated calls.
use std::sync::{Arc, Once};
use server::engine::execution::ExecutionRegistry;
use server::engine::fetch::FetchConfig;
use server::engine::module_loader::ModuleLoaderConfig;
use server::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};
use server::engine::{initialize_v8, Engine};

static INIT: Once = Once::new();
fn ensure_v8() { INIT.call_once(initialize_v8); }

fn allow_all_chain() -> Arc<PolicyChain> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("allow_all.rego");
    std::fs::write(&path, "package mcp.fetch\ndefault allow = true\n").unwrap();
    let evaluator =
        LocalPolicyEvaluator::from_file(&path, "data.mcp.fetch.allow".to_string()).unwrap();
    Arc::new(PolicyChain::new(vec![PolicyEvaluatorKind::Local(evaluator)], EvalMode::All))
}

fn build_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!("mcp-modal-scratch-{}", std::process::id()));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("registry");
    Engine::new_stateless(64 * 1024 * 1024, 60, 4)
        .with_fetch_config(FetchConfig::new_with_chain(allow_all_chain()))
        .with_module_loader_config(ModuleLoaderConfig {
            allow_external: true,
            policy_chain: None,
        })
        .with_execution_registry(Arc::new(registry))
}

async fn eval(engine: &Engine, code: String) -> String {
    let wrapped = format!("Promise.resolve({code}).then(function(v) {{ console.log(v); }});");
    let mut args = serde_json::Map::new();
    args.insert("code".into(), serde_json::Value::String(wrapped));
    args.insert("execution_timeout_secs".into(), serde_json::json!(60));
    let resp = server::mcp_dispatch::run_js_blocking(engine, None, &serde_json::Value::Object(args)).await;
    format!("{resp:?}")
}

const PROBE: &str = r##"
(async () => {
    const protobuf = (await import('npm:protobufjs@7.4.0')).default;
    const protoText = await (await fetch('https://cdn.jsdelivr.net/gh/modal-labs/modal-client@main/modal_proto/api.proto')).text();
    const root = new protobuf.Root();
    // parse() does not auto-load the google well-known types; merge the
    // copies protobufjs ships.
    for (const f of ['google/protobuf/any.proto','google/protobuf/empty.proto',
                     'google/protobuf/struct.proto','google/protobuf/timestamp.proto',
                     'google/protobuf/wrappers.proto']) {
        const json = protobuf.common.get(f);
        if (json && json.nested) root.addJSON(json.nested);
    }
    protobuf.parse(protoText, root, { keepCase: false });
    const svc = root.lookupService('modal.client.ModalClient');

    const SERVER = 'https://api.modal.com';
    const AUTH = {
        'x-modal-token-id': '__TOKEN_ID__',
        'x-modal-token-secret': '__TOKEN_SECRET__',
        'x-modal-client-type': '8',
        'x-modal-client-version': '1.0.0',
        'x-modal-libmodal-version': '0.9.0',
    };

    async function unary(methodName, plain) {
        const m = svc.methods[methodName];
        m.resolve();
        const msg = m.resolvedRequestType.fromObject(plain || {});
        const bytes = m.resolvedRequestType.encode(msg).finish();
        const frame = new Uint8Array(5 + bytes.length);
        frame[0] = 0;
        new DataView(frame.buffer).setUint32(1, bytes.length);
        frame.set(bytes, 5);
        const r = await fetch(`${SERVER}/modal.client.ModalClient/${methodName}`, {
            method: 'POST',
            headers: { 'content-type': 'application/grpc', 'te': 'trailers', ...AUTH },
            body: frame,
        });
        const status = r.headers.get('grpc-status');
        if (status !== null && status !== '0') {
            return { grpcStatus: Number(status), grpcMessage: r.headers.get('grpc-message') };
        }
        const buf = new Uint8Array(await r.arrayBuffer());
        const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
        const items = [];
        let off = 0;
        while (off + 5 <= buf.length) {
            const flags = buf[off];
            const len = dv.getUint32(off + 1);
            if (flags & 0x80) break; // trailers frame (grpc-web); native uses real trailers
            const msg = m.resolvedResponseType.decode(buf.subarray(off + 5, off + 5 + len));
            items.push(m.resolvedResponseType.toObject(msg, { longs: Number, defaults: false }));
            off += 5 + len;
        }
        return { ok: true, response: items[0], stream: items };
    }

    const hello = await unary('ClientHello', {});
    const appList = await unary('AppList', { environmentName: 'main' });

    // cpClient: same interface nice-grpc would generate — camelCase methods
    // taking plain objects. Errors mimic nice-grpc's ClientError shape.
    function grpcError(methodName, st, msg) {
        const e = new Error(`/modal.client.ModalClient/${methodName} ${st}: ${msg}`);
        e.code = st;
        e.details = msg;
        e.name = 'ClientError';
        return e;
    }
    const cpClient = new Proxy({}, {
        get(_t, prop) {
            if (typeof prop !== 'string') return undefined;
            const methodName = prop[0].toUpperCase() + prop.slice(1);
            if (!svc.methods[methodName]) return undefined;
            const m = svc.methods[methodName];
            if (m.responseStream) {
                return async function* (req) {
                    const out = await unary(methodName, req);
                    if (!out.ok) throw grpcError(methodName, out.grpcStatus, out.grpcMessage);
                    for (const item of out.stream) yield item;
                };
            }
            return async (req) => {
                const out = await unary(methodName, req);
                if (!out.ok) throw grpcError(methodName, out.grpcStatus, out.grpcMessage);
                return out.response;
            };
        },
    });

    // Drive the real SDK through the injected client.
    const { ModalClient } = await import('npm:modal@0.9.0?bundle');
    const client = new ModalClient({ tokenId: '__TOKEN_ID__', tokenSecret: '__TOKEN_SECRET__', cpClient });
    let sdkResult;
    try {
        const app = await client.apps.fromName('my-app', { createIfMissing: false });
        sdkResult = { ok: true, appId: app.appId };
    } catch (e) {
        sdkResult = { error: String(e && e.message || e).slice(0, 120) };
    }
    return JSON.stringify({ methods: Object.keys(svc.methods).length, hello, appList, sdkResult });
})()
"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "network test"]
async fn modal_sdk_over_fetch() {
    ensure_v8();
    let engine = build_engine();
    let token_id = std::env::var("MODAL_TOKEN_ID").unwrap_or_else(|_| "ak-FAKE".into());
    let token_secret = std::env::var("MODAL_TOKEN_SECRET").unwrap_or_else(|_| "as-FAKE".into());
    let probe = PROBE
        .replace("__TOKEN_ID__", &token_id)
        .replace("__TOKEN_SECRET__", &token_secret);
    let out = eval(&engine, probe).await;
    println!("RESULT: {out}");
    // The service must be reached: either authenticated success or the
    // auth-layer rejection. Anything else (TLS, transport, framing,
    // proto errors) fails the assertion.
    assert!(
        out.contains("Token not found") || out.contains("\"ok\":true"),
        "gRPC plumbing did not reach Modal's service: {out}"
    );
}
