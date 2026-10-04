// A/B benchmark: rust-url-backed URL (product default) vs the bundled
// whatwg-url JS implementation. Run explicitly:
//   cargo test --test url_impl_bench -- --ignored --nocapture
use std::sync::{Arc, Once};

use server::engine::execution::ExecutionRegistry;
use server::engine::{initialize_v8, Engine};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn build_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-url-bench-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(256 * 1024 * 1024, 120, 4)
        .with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: String) -> serde_json::Value {
    let mut args = serde_json::Map::new();
    args.insert("code".into(), serde_json::Value::String(code));
    args.insert("execution_timeout_secs".into(), serde_json::json!(120));
    server::mcp_dispatch::run_js_blocking(engine, None, &serde_json::Value::Object(args))
        .await
        .json
}

async fn eval(engine: &Engine, code: String) -> String {
    let wrapped = format!("Promise.resolve({code}).then(function(v) {{ console.log(v); }});");
    let resp = run_js(engine, wrapped).await;
    assert!(resp["error"].is_null(), "execution failed: {resp:?}");
    resp["output"]
        .as_str()
        .expect("dispatcher should return an output field")
        .to_string()
}

const WHATWG_URL_BUNDLE_JS: &str = include_str!("wpt/runner/whatwg-url-bundle.js");
const ADAPTER_INLINE: &str = r#"
globalThis.URL = globalThis.__whatwgURL.URL;
globalThis.URLSearchParams = globalThis.__whatwgURL.URLSearchParams;
"#;

const BENCH: &str = r#"
(function () {
    var corpus = [
        ["https://example.com/", undefined],
        ["https://user:pass@sub.example.co.uk:8443/a/b/c?x=1&y=2#frag", undefined],
        ["http://192.168.0.1:8080/path", undefined],
        ["file:///C:/Windows/System32/", undefined],
        ["https://[2001:db8::1]/ipv6", undefined],
        ["../relative/path?q", "https://base.example/dir/file"],
        ["//protocol-relative.example/x", "https://base.example/"],
        ["data:text/plain;base64,SGVsbG8=", undefined],
        ["https://xn--nxasmq6b.example/idn", undefined],
        ["https://example.com/%E4%BD%A0%E5%A5%BD?%20q=%3F", undefined],
        ["ws://echo.example/socket", undefined],
        ["https://example.com/a/../b/./c//d", undefined],
        ["mailto:user@example.com", undefined],
        ["https://日本語.example/パス?クエリ=値", undefined],
        ["not a url", "https://fallback.example/"],
    ];
    var ITERS = 20000;
    // warmup
    for (var w = 0; w < 500; w++) {
        for (var i = 0; i < corpus.length; i++) {
            try { new URL(corpus[i][0], corpus[i][1]); } catch (_) {}
        }
    }
    var checksum = 0;
    var t0 = performance.now();
    for (var iter = 0; iter < ITERS; iter++) {
        for (var j = 0; j < corpus.length; j++) {
            try {
                var u = new URL(corpus[j][0], corpus[j][1]);
                checksum += u.href.length + u.pathname.length;
                u.searchParams.get("x");
            } catch (_) { checksum += 1; }
        }
    }
    var t1 = performance.now();
    var sp = 0;
    var t2 = performance.now();
    for (var k = 0; k < 50000; k++) {
        var p = new URLSearchParams("a=1&b=2&c=3&a=4");
        p.append("d", "5");
        sp += p.toString().length + (p.get("a") || "").length;
    }
    var t3 = performance.now();
    return JSON.stringify({
        bundle_eval_ms: typeof __bundleMs === 'number' ? __bundleMs : null,
        parse_ms: t1 - t0,
        parses: ITERS * corpus.length,
        usp_ms: t3 - t2,
        usp_ops: 50000,
        checksum: checksum + sp,
    });
})()
"#;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark, run explicitly"]
async fn url_impl_ab_benchmark() {
    ensure_v8();

    // Stateless engines run each eval in a fresh isolate, so each variant
    // is one self-contained script.
    let a = build_engine();
    let rust_result = eval(&a, BENCH.to_string()).await;

    // Bundle + adapter + bench in one script; the bundle eval is timed
    // in-script — that is the per-isolate startup tax option 2 would add.
    let whatwg_script = format!(
        "(function () {{\n\
         var __t = performance.now();\n\
         {WHATWG_URL_BUNDLE_JS}\n\
         globalThis.__bundleMs = performance.now() - __t;\n\
         {ADAPTER_INLINE}\n\
         return ({BENCH});\n\
         }})()"
    );
    let b = build_engine();
    let whatwg_result = eval(&b, whatwg_script.clone()).await;
    // Second fresh isolate for run-to-run variance on the bundle eval.
    let c = build_engine();
    let whatwg_result_2 = eval(&c, whatwg_script).await;

    println!("== URL implementation A/B benchmark ==");
    println!("bundle size: {} KiB", WHATWG_URL_BUNDLE_JS.len() / 1024);
    println!("rust-url ops: {rust_result}");
    println!("whatwg-url #1: {whatwg_result}");
    println!("whatwg-url #2: {whatwg_result_2}");
}
