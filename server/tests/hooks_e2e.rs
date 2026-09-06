//! End-to-end tests for the pre/post hook system: JS in the sandbox calls
//! `fetch()` / `fs.*`, and the configured hook chain (pre hooks → policy →
//! post hooks) denies or rewrites the operation.

use std::sync::{Arc, Once};

use axum::{
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use server::engine::execution::ExecutionRegistry;
use server::engine::fetch::FetchConfig;
use server::engine::fs::FsConfig;
use server::engine::hooks::{build_hook_chain, HookCaps, HookChain, HookSource, StackEntry};
use server::engine::opa::{OperationPolicies, PolicySource};
use server::engine::{initialize_v8, Engine};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

// ── Test server ─────────────────────────────────────────────────────────────

async fn start_server() -> String {
    async fn echo_handler(headers: HeaderMap) -> impl IntoResponse {
        let hooked = headers
            .get("x-hooked")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("absent")
            .to_string();
        (StatusCode::OK, format!("echo hooked={}", hooked))
    }

    async fn secret_handler() -> impl IntoResponse {
        (
            StatusCode::OK,
            [("x-contains-secret", "yes")],
            "token=hunter2",
        )
    }

    let app = Router::new()
        .route("/echo", get(echo_handler))
        .route("/secret", get(secret_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}", address)
}

// ── Engine harness ──────────────────────────────────────────────────────────

fn write_rego(dir: &std::path::Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    format!("file://{}", path.display())
}

fn build_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-hooks-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(64 * 1024 * 1024, 30, 4).with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: String) -> serde_json::Value {
    let mut args = serde_json::Map::new();
    args.insert("code".into(), serde_json::Value::String(code));
    args.insert("execution_timeout_secs".into(), serde_json::json!(30));
    server::mcp_dispatch::run_js_blocking(engine, None, &serde_json::Value::Object(args))
        .await
        .json
}

/// Run an async JS expression, resolving errors into an `ERROR: <msg>` string
/// so both success and denial paths come back through console output.
async fn eval(engine: &Engine, code: String) -> String {
    let wrapped = format!(
        "Promise.resolve().then(function() {{ return ({code}); }})\
         .then(function(v) {{ console.log(v); }})\
         .catch(function(e) {{ console.log('ERROR: ' + (e && e.message ? e.message : e)); }});"
    );
    let resp = run_js(engine, wrapped).await;
    assert!(resp["error"].is_null(), "execution failed: {resp:?}");
    resp["output"]
        .as_str()
        .expect("dispatcher should return an output field")
        .trim()
        .to_string()
}

// ── fetch: pre hook mutation + policy over the effective input ──────────────

/// The pre hook rewrites `/blocked` to `/echo` and injects an `x-hooked`
/// header; the policy allows only `/echo`. The request succeeds *because*
/// the policy ran after the mutation — and the server observes the injected
/// header, proving the rewritten input is what executed.
#[tokio::test]
async fn fetch_pre_hook_rewrites_request_before_policy() {
    ensure_v8();
    let base = start_server().await;
    let dir = tempfile::tempdir().unwrap();

    let hook_url = write_rego(
        dir.path(),
        "hook.rego",
        r#"
package mcp.fetch

pre := {"input": patched} if {
    endswith(input.url, "/blocked")
    rewritten := replace(input.url, "/blocked", "/echo")
    patched := object.union(input, {
        "url": rewritten,
        "headers": object.union(input.headers, {"x-hooked": "yes"}),
    })
}
"#,
    );
    let policy_url = write_rego(
        dir.path(),
        "policy.rego",
        r#"
package mcp.fetch

default allow = false

allow if {
    input.url_parsed.path == "/echo"
}
"#,
    );

    let op = OperationPolicies {
        policies: vec![PolicySource {
            url: policy_url,
            policy_path: None,
            rule: None,
        }],
        pre: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None, // → data.mcp.fetch.pre
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "fetch",
        &op,
        "mcp/fetch",
        "data.mcp.fetch.allow",
        HookCaps {
            input_mutation: true,
            post: true,
        },
    )
    .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    // /blocked is rewritten to /echo (policy passes) with the header injected.
    let out = eval(
        &engine,
        format!(r#"fetch("{base}/blocked").then(r => r.text())"#),
    )
    .await;
    assert_eq!(out, "echo hooked=yes");

    // /secret is untouched by the hook, so the policy denies it.
    let out = eval(
        &engine,
        format!(r#"fetch("{base}/secret").then(r => r.text())"#),
    )
    .await;
    assert!(
        out.starts_with("ERROR:") && out.contains("denied by policy"),
        "got: {out}"
    );
}

// ── fetch: pre hook denial with a reason ────────────────────────────────────

#[tokio::test]
async fn fetch_pre_hook_denies_with_reason() {
    ensure_v8();
    let base = start_server().await;
    let dir = tempfile::tempdir().unwrap();

    let hook_url = write_rego(
        dir.path(),
        "hook.rego",
        r#"
package mcp.fetch

pre := {"allow": false, "reason": "mutating methods are frozen"} if {
    input.method != "GET"
}
"#,
    );

    let op = OperationPolicies {
        pre: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None,
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "fetch",
        &op,
        "mcp/fetch",
        "data.mcp.fetch.allow",
        HookCaps {
            input_mutation: true,
            post: true,
        },
    )
    .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    let out = eval(
        &engine,
        format!(r#"fetch("{base}/echo", {{method: "POST"}}).then(r => r.text())"#),
    )
    .await;
    assert!(
        out.contains("denied by pre hook (mutating methods are frozen)"),
        "got: {out}"
    );

    // GET abstains (rule undefined) and there is no policy → allowed.
    let out = eval(
        &engine,
        format!(r#"fetch("{base}/echo").then(r => r.text())"#),
    )
    .await;
    assert_eq!(out, "echo hooked=absent");
}

// ── fetch: post hooks mutate and deny the response ──────────────────────────

#[tokio::test]
async fn fetch_post_hook_mutates_and_denies_response() {
    ensure_v8();
    let base = start_server().await;
    let dir = tempfile::tempdir().unwrap();

    let post_url = write_rego(
        dir.path(),
        "post.rego",
        r#"
package mcp.fetch

# Deny responses flagged as carrying secrets.
post := {"allow": false, "reason": "response contains a secret"} if {
    input.output.headers["x-contains-secret"] == "yes"
}

# Otherwise stamp the response so JS can observe post-hook mutation.
post := {"output": patched} if {
    not input.output.headers["x-contains-secret"]
    patched := object.union(input.output, {
        "headers": object.union(input.output.headers, {"x-post-hooked": "yes"}),
    })
}
"#,
    );

    let op = OperationPolicies {
        post: vec![HookSource {
            url: post_url,
            policy_path: None,
            rule: None, // → data.mcp.fetch.post
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "fetch",
        &op,
        "mcp/fetch",
        "data.mcp.fetch.allow",
        HookCaps {
            input_mutation: true,
            post: true,
        },
    )
    .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    let out = eval(
        &engine,
        format!(r#"fetch("{base}/echo").then(r => r.headers.get("x-post-hooked"))"#),
    )
    .await;
    assert_eq!(out, "yes");

    let out = eval(
        &engine,
        format!(r#"fetch("{base}/secret").then(r => r.text())"#),
    )
    .await;
    assert!(
        out.contains("denied by post hook (response contains a secret)"),
        "got: {out}"
    );
}

// ── fs: pre hook rewrites a virtual path, policy gates the real one ─────────

#[tokio::test]
async fn fs_pre_hook_rewrites_path_before_policy() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    std::fs::write(data_dir.join("greeting.txt"), "hello from the real path").unwrap();
    let data_dir_str = data_dir.to_string_lossy().into_owned();

    let hook_url = write_rego(
        dir.path(),
        "hook.rego",
        &format!(
            r#"
package mcp.filesystem

pre := {{"input": object.union(input, {{"path": real}})}} if {{
    startswith(input.path, "/virtual/")
    real := concat("", ["{data_dir}/", substring(input.path, 9, -1)])
}}
"#,
            data_dir = data_dir_str
        ),
    );
    // The policy only allows the real data dir — /virtual/ paths pass solely
    // because the pre hook rewrote them first.
    let policy_url = write_rego(
        dir.path(),
        "policy.rego",
        &format!(
            r#"
package mcp.filesystem

default allow = false

allow if {{
    startswith(input.path, "{data_dir}/")
}}
"#,
            data_dir = data_dir_str
        ),
    );

    let op = OperationPolicies {
        policies: vec![PolicySource {
            url: policy_url,
            policy_path: None,
            rule: None,
        }],
        pre: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None, // → data.mcp.filesystem.pre
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "filesystem",
        &op,
        "mcp/filesystem",
        "data.mcp.filesystem.allow",
        HookCaps {
            input_mutation: true,
            post: false,
        },
    )
    .unwrap();
    let engine = build_engine().with_fs_config(FsConfig::new_with_hooks(Arc::new(chain)));

    // The virtual path resolves through the hook to the real file.
    let out = eval(
        &engine,
        r#"fs.readFile("/virtual/greeting.txt", "utf8")"#.to_string(),
    )
    .await;
    assert_eq!(out, "hello from the real path");

    // A path outside both the virtual prefix and the data dir is denied.
    let out = eval(&engine, r#"fs.readFile("/etc/hostname", "utf8")"#.to_string()).await;
    assert!(
        out.starts_with("ERROR:") && out.contains("denied by policy"),
        "got: {out}"
    );
}

// ── fs: a hook that drops `destination` fails the operation ─────────────────

#[tokio::test]
async fn fs_hook_dropping_destination_fails_closed() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    std::fs::write(data_dir.join("a.txt"), "x").unwrap();
    let data_dir_str = data_dir.to_string_lossy().into_owned();

    // A buggy hook that rebuilds the input without `destination`.
    let hook_url = write_rego(
        dir.path(),
        "hooks.js",
        r#"
function pre(input) {
    if (input.operation === "rename") {
        const { destination, ...rest } = input;
        return { input: rest };
    }
}
"#,
    );
    let op = OperationPolicies {
        pre: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None,
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "filesystem",
        &op,
        "mcp/filesystem",
        "data.mcp.filesystem.allow",
        HookCaps {
            input_mutation: true,
            post: false,
        },
    )
    .unwrap();
    let engine = build_engine().with_fs_config(FsConfig::new_with_hooks(Arc::new(chain)));

    let out = eval(
        &engine,
        format!(r#"fs.rename("{data_dir_str}/a.txt", "{data_dir_str}/b.txt")"#),
    )
    .await;
    assert!(
        out.starts_with("ERROR:") && out.contains("pre hook removed 'destination'"),
        "got: {out}"
    );
    // The operation did not run: the source file is untouched.
    assert!(data_dir.join("a.txt").exists());
    assert!(!data_dir.join("b.txt").exists());
}

// ── fetch: JavaScript hooks (file://*.js) ───────────────────────────────────

/// A JS pre hook rewrites `/blocked` to `/echo` and injects a header; a JS
/// post hook stamps the response. Same flow as the Rego tests, in JavaScript.
#[tokio::test]
async fn fetch_js_hooks_rewrite_request_and_response() {
    ensure_v8();
    let base = start_server().await;
    let dir = tempfile::tempdir().unwrap();

    let hook_url = write_rego(
        dir.path(),
        "hooks.js",
        r#"
function pre(input) {
    if (input.method !== "GET") {
        return { allow: false, reason: "read-only" };
    }
    if (input.url.endsWith("/blocked")) {
        return {
            input: {
                ...input,
                url: input.url.replace("/blocked", "/echo"),
                headers: { ...input.headers, "x-hooked": "js" },
            },
        };
    }
}

function post(input, output) {
    return { output: { ...output, headers: { ...output.headers, "x-post-hooked": "js" } } };
}
"#,
    );

    let op = OperationPolicies {
        pre: vec![HookSource {
            url: hook_url.clone(),
            policy_path: None,
            rule: None, // → function pre()
            timeout_ms: None,
            capabilities: None,
        }],
        post: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None, // → function post()
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "fetch",
        &op,
        "mcp/fetch",
        "data.mcp.fetch.allow",
        HookCaps {
            input_mutation: true,
            post: true,
        },
    )
    .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    // The JS pre hook rewrites the path and injects the header; the JS post
    // hook stamps the response.
    let out = eval(
        &engine,
        format!(
            r#"fetch("{base}/blocked").then(async r => (await r.text()) + " post=" + r.headers.get("x-post-hooked"))"#
        ),
    )
    .await;
    assert_eq!(out, "echo hooked=js post=js");

    // Non-GET is denied by the JS hook with its reason.
    let out = eval(
        &engine,
        format!(r#"fetch("{base}/echo", {{method: "POST"}}).then(r => r.text())"#),
    )
    .await;
    assert!(out.contains("denied by pre hook (read-only)"), "got: {out}");
}

// ── layered stacks (v2) ─────────────────────────────────────────────────────

fn js_source(url: String, capabilities: Option<Vec<String>>) -> HookSource {
    HookSource {
        url,
        policy_path: None,
        rule: None,
        timeout_ms: None,
        capabilities,
    }
}

fn stack_caps() -> HookCaps {
    HookCaps {
        input_mutation: true,
        post: true,
    }
}

/// A single JS layer pairs both sides of one call: it rewrites the request,
/// awaits `next`, and stamps the response — state held in one closure, which
/// the flat pre/post chain cannot express.
#[tokio::test]
async fn fetch_stack_layer_pairs_request_and_response() {
    ensure_v8();
    let base = start_server().await;
    let dir = tempfile::tempdir().unwrap();

    let layer_url = write_rego(
        dir.path(),
        "layer.js",
        r#"
async function handle(input, next) {
    const started = input.url;
    const out = await next({
        ...input,
        url: input.url.replace("/blocked", "/echo"),
        headers: { ...input.headers, "x-hooked": "layered" },
    });
    return { ...out, headers: { ...out.headers, "x-started-as": started.endsWith("/blocked") ? "blocked" : "other" } };
}
"#,
    );
    let op = OperationPolicies {
        stack: vec![
            StackEntry::Source(js_source(layer_url, None)),
            StackEntry::Builtin("@execute".to_string()),
        ],
        ..Default::default()
    };
    let chain = build_hook_chain("fetch", &op, "mcp/fetch", "data.mcp.fetch.allow", stack_caps())
        .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    let out = eval(
        &engine,
        format!(
            r#"fetch("{base}/blocked").then(async r => (await r.text()) + " started=" + r.headers.get("x-started-as"))"#
        ),
    )
    .await;
    assert_eq!(out, "echo hooked=layered started=blocked");
}

/// A layer that never calls `next` short-circuits the operation: the guest
/// gets a synthetic response and the server is never contacted.
#[tokio::test]
async fn fetch_stack_layer_short_circuits_without_executing() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();

    // "ZnJvbS1jYWNoZQ==" = base64("from-cache"); the layer isolate is bare
    // (no btoa), so the test precomputes it.
    let layer_url = write_rego(
        dir.path(),
        "layer.js",
        r#"
async function handle(input, next) {
    if (input.url_parsed.path === "/cached") {
        return {
            status: 200, statusText: "OK", url: input.url,
            headers: { "x-cache": "hit" },
            body: "ZnJvbS1jYWNoZQ==", bodyEncoding: "base64", redirected: false,
        };
    }
    return next(input);
}
"#,
    );
    let op = OperationPolicies {
        stack: vec![
            StackEntry::Source(js_source(layer_url, None)),
            StackEntry::Builtin("@execute".to_string()),
        ],
        ..Default::default()
    };
    let chain = build_hook_chain("fetch", &op, "mcp/fetch", "data.mcp.fetch.allow", stack_caps())
        .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    // Port 9 (discard) would hang or refuse — the layer must answer instead.
    let out = eval(
        &engine,
        r#"fetch("http://127.0.0.1:9/cached").then(async r => (await r.text()) + " cache=" + r.headers.get("x-cache"))"#.to_string(),
    )
    .await;
    assert_eq!(out, "from-cache cache=hit");
}

/// A layer may call `next` more than once: on a 500 it retries a fallback
/// path — retry/fallback logic the flat chain cannot express.
#[tokio::test]
async fn fetch_stack_layer_retries_on_error_status() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();

    async fn fail_handler() -> impl IntoResponse {
        (StatusCode::INTERNAL_SERVER_ERROR, "boom")
    }
    async fn ok_handler() -> impl IntoResponse {
        (StatusCode::OK, "recovered")
    }
    let app = Router::new()
        .route("/fail", get(fail_handler))
        .route("/fallback", get(ok_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let layer_url = write_rego(
        dir.path(),
        "layer.js",
        r#"
async function handle(input, next) {
    let out = await next(input);
    if (out.status >= 500) {
        out = await next({ ...input, url: input.url.replace("/fail", "/fallback") });
        out = { ...out, headers: { ...out.headers, "x-retried": "yes" } };
    }
    return out;
}
"#,
    );
    let op = OperationPolicies {
        stack: vec![
            StackEntry::Source(js_source(layer_url, None)),
            StackEntry::Builtin("@execute".to_string()),
        ],
        ..Default::default()
    };
    let chain = build_hook_chain("fetch", &op, "mcp/fetch", "data.mcp.fetch.allow", stack_caps())
        .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    let out = eval(
        &engine,
        format!(
            r#"fetch("{base}/fail").then(async r => (await r.text()) + " retried=" + r.headers.get("x-retried"))"#
        ),
    )
    .await;
    assert_eq!(out, "recovered retried=yes");
}

/// `@policy` in a stack gates the effective input: a layer's rewrite into
/// compliance passes, and every descent re-checks (a retry cannot slip an
/// unapproved input past it).
#[tokio::test]
async fn fetch_stack_policy_gates_every_descent() {
    ensure_v8();
    let base = start_server().await;
    let dir = tempfile::tempdir().unwrap();

    let layer_url = write_rego(
        dir.path(),
        "layer.js",
        r#"
async function handle(input, next) {
    return next({ ...input, url: input.url.replace("/blocked", "/echo") });
}
"#,
    );
    let policy_url = write_rego(
        dir.path(),
        "policy.rego",
        r#"
package mcp.fetch

default allow = false

allow if {
    input.url_parsed.path == "/echo"
}
"#,
    );
    let op = OperationPolicies {
        policies: vec![PolicySource {
            url: policy_url,
            policy_path: None,
            rule: None,
        }],
        stack: vec![
            StackEntry::Source(js_source(layer_url, None)),
            StackEntry::Builtin("@policy".to_string()),
            StackEntry::Builtin("@execute".to_string()),
        ],
        ..Default::default()
    };
    let chain = build_hook_chain("fetch", &op, "mcp/fetch", "data.mcp.fetch.allow", stack_caps())
        .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    let out = eval(
        &engine,
        format!(r#"fetch("{base}/blocked").then(r => r.text())"#),
    )
    .await;
    assert_eq!(out, "echo hooked=absent");

    let out = eval(
        &engine,
        format!(r#"fetch("{base}/secret").then(r => r.text())"#),
    )
    .await;
    assert!(
        out.starts_with("ERROR:") && out.contains("denied by policy"),
        "got: {out}"
    );
}

/// `@inject` ordering in a stack: a layer placed before it never sees the
/// injected credential, while the executor (and server) does.
#[tokio::test]
async fn fetch_stack_inject_after_layers() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();

    async fn token_handler(headers: HeaderMap) -> impl IntoResponse {
        let token = headers
            .get("x-injected-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("absent")
            .to_string();
        (StatusCode::OK, format!("token={}", token))
    }
    let app = Router::new().route("/token", get(token_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let layer_url = write_rego(
        dir.path(),
        "layer.js",
        r#"
async function handle(input, next) {
    const saw = input.headers["x-injected-token"] ? "yes" : "no";
    const out = await next(input);
    return { ...out, headers: { ...out.headers, "x-layer-saw-token": saw } };
}
"#,
    );
    let op = OperationPolicies {
        stack: vec![
            StackEntry::Source(js_source(layer_url, None)),
            StackEntry::Builtin("@inject".to_string()),
            StackEntry::Builtin("@execute".to_string()),
        ],
        ..Default::default()
    };
    let chain = build_hook_chain("fetch", &op, "mcp/fetch", "data.mcp.fetch.allow", stack_caps())
        .unwrap();
    let rule = server::engine::fetch::HeaderRule::static_header(
        "127.0.0.1".to_string(),
        vec![],
        "x-injected-token".to_string(),
        "hunter2".to_string(),
    )
    .unwrap();
    let engine = build_engine().with_fetch_config(
        FetchConfig::new_with_hooks(Arc::new(chain)).with_header_rules(vec![rule]),
    );

    let out = eval(
        &engine,
        format!(
            r#"fetch("{base}/token").then(async r => (await r.text()) + " layer-saw=" + r.headers.get("x-layer-saw-token"))"#
        ),
    )
    .await;
    assert_eq!(out, "token=hunter2 layer-saw=no");
}

// ── subprocess: full stack (executor innermost) ─────────────────────────────

/// A subprocess stack layer short-circuits a "mocked" command with a
/// synthetic result and transforms the exit code of real ones — the guest's
/// child_process.exec sees layer output either way.
#[tokio::test]
async fn subprocess_stack_layer_short_circuits_and_transforms() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();

    // A synthetic result uses the executor's document shape; with utf8
    // encoding, stdout is a plain string.
    let layer_url = write_rego(
        dir.path(),
        "layer.js",
        r#"
async function handle(input, next) {
    if (input.args.some((a) => a.includes("magic"))) {
        return { code: 0, stdout: "mocked", stderr: "", success: true, encoding: "utf8" };
    }
    const out = await next(input);
    return { ...out, code: out.code === 0 ? 0 : 42 };
}
"#,
    );
    let op = OperationPolicies {
        stack: vec![
            StackEntry::Source(js_source(layer_url, None)),
            StackEntry::Builtin("@execute".to_string()),
        ],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "subprocess",
        &op,
        "mcp/subprocess",
        "data.mcp.subprocess.allow",
        stack_caps(),
    )
    .unwrap();
    // Guest subprocess wiring rides on fetch + fs being configured.
    let engine = build_engine()
        .with_fetch_config(FetchConfig::new_with_hooks(Arc::new(HookChain::permissive(
            "fetch",
        ))))
        .with_fs_config(FsConfig::new_with_hooks(Arc::new(HookChain::permissive(
            "filesystem",
        ))))
        .with_subprocess_config(server::engine::subprocess::SubprocessConfig::new_with_hooks(
            Arc::new(chain),
        ));

    // Short-circuit: the "magic" command never runs; the layer answers.
    let out = eval(
        &engine,
        r#"child_process.exec("magic").then(r => r.stdout)"#.to_string(),
    )
    .await;
    assert_eq!(out, "mocked");

    // Real execution with a transformed exit code on failure.
    let out = eval(
        &engine,
        r#"child_process.exec("exit 3").then(r => "code=" + r.code)"#.to_string(),
    )
    .await;
    assert_eq!(out, "code=42");
}

// ── filesystem: gate-mode stack ─────────────────────────────────────────────

/// An fs stack runs in gate mode: the layer rewrites the path (full next
/// mechanics) and `@policy` gates the effective input; the operation then
/// executes with the captured input. Layers cannot fabricate outputs.
#[tokio::test]
async fn fs_stack_gate_mode_rewrites_and_gates() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let safe_dir = data_dir.join("safe");
    std::fs::create_dir_all(&safe_dir).unwrap();
    let data_dir_str = data_dir.to_string_lossy().into_owned();

    let layer_url = write_rego(
        dir.path(),
        "layer.js",
        &format!(
            r#"
async function handle(input, next) {{
    if (input.path && !input.path.includes("/safe/")) {{
        const parts = input.path.split("/");
        return next({{ ...input, path: {data:?} + "/safe/" + parts[parts.length - 1] }});
    }}
    return next(input);
}}
"#,
            data = data_dir_str,
        ),
    );
    let policy_url = write_rego(
        dir.path(),
        "policy.rego",
        &format!(
            r#"
package mcp.filesystem

default allow = false

allow if {{
    startswith(input.path, "{data_dir_str}/safe/")
}}
"#
        ),
    );
    let op = OperationPolicies {
        policies: vec![PolicySource {
            url: policy_url,
            policy_path: None,
            rule: None,
        }],
        stack: vec![
            StackEntry::Source(js_source(layer_url, None)),
            StackEntry::Builtin("@policy".to_string()),
            StackEntry::Builtin("@execute".to_string()),
        ],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "filesystem",
        &op,
        "mcp/filesystem",
        "data.mcp.filesystem.allow",
        HookCaps {
            input_mutation: true,
            post: false,
        },
    )
    .unwrap();
    let engine = build_engine().with_fs_config(FsConfig::new_with_hooks(Arc::new(chain)));

    // A write outside /safe/ is rewritten into it (and the policy passes
    // because it evaluates the rewritten path).
    let out = eval(
        &engine,
        format!(
            r#"fs.writeFile("{data_dir_str}/a.txt", "1").then(() => fs.readFile("{data_dir_str}/safe/a.txt", "utf8"))"#
        ),
    )
    .await;
    assert_eq!(out, "1");
    assert!(!data_dir.join("a.txt").exists(), "original path must not be written");
    assert!(safe_dir.join("a.txt").exists());
}

// ── fetch: injected credentials do not follow a hook's host rewrite ─────────

/// A header rule injects a credential for host `127.0.0.1`. A pre hook that
/// rewrites the URL to `localhost` (same listener, different host, so the
/// rule no longer matches) must not carry the credential along; a same-host
/// path rewrite keeps it. The echo server reports what it received.
#[tokio::test]
async fn fetch_injected_credential_stripped_on_host_rewrite() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();

    async fn token_handler(headers: HeaderMap) -> impl IntoResponse {
        let token = headers
            .get("x-injected-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("absent")
            .to_string();
        (StatusCode::OK, format!("token={}", token))
    }
    let app = Router::new().route("/token", get(token_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let hook_url = write_rego(
        dir.path(),
        "hooks.js",
        &format!(
            r#"
function pre(input) {{
    if (input.url_parsed.path === "/hop") {{
        // Cross-host rewrite: the injecting rule (127.0.0.1) stops matching.
        return {{ input: {{ ...input, url: "http://localhost:{port}/token" }} }};
    }}
    if (input.url_parsed.path === "/stay") {{
        // Same-host rewrite: the rule still matches, credential survives.
        return {{ input: {{ ...input, url: "http://127.0.0.1:{port}/token" }} }};
    }}
}}
"#
        ),
    );
    let op = OperationPolicies {
        pre: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None,
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "fetch",
        &op,
        "mcp/fetch",
        "data.mcp.fetch.allow",
        HookCaps {
            input_mutation: true,
            post: true,
        },
    )
    .unwrap();
    let rule = server::engine::fetch::HeaderRule::static_header(
        "127.0.0.1".to_string(),
        vec![],
        "x-injected-token".to_string(),
        "hunter2".to_string(),
    )
    .unwrap();
    let engine = build_engine().with_fetch_config(
        FetchConfig::new_with_hooks(Arc::new(chain)).with_header_rules(vec![rule]),
    );

    // Same-host rewrite: injected credential is kept.
    let out = eval(
        &engine,
        format!(r#"fetch("http://127.0.0.1:{port}/stay").then(r => r.text())"#),
    )
    .await;
    assert_eq!(out, "token=hunter2");

    // Cross-host rewrite: the credential must be stripped.
    let out = eval(
        &engine,
        format!(r#"fetch("http://127.0.0.1:{port}/hop").then(r => r.text())"#),
    )
    .await;
    assert_eq!(out, "token=absent", "credential must not follow the host rewrite");
}

// ── fs: a hook that rewrites `operation` fails closed ───────────────────────

/// The executor performs the operation it was invoked for regardless of the
/// JSON `operation` field, so a hook rewriting it could only make the policy
/// evaluate a different operation than what runs. That's rejected outright.
#[tokio::test]
async fn fs_hook_spoofing_operation_fails_closed() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    let data_dir_str = data_dir.to_string_lossy().into_owned();

    let hook_url = write_rego(
        dir.path(),
        "hooks.js",
        r#"
function pre(input) {
    if (input.operation === "writeFile") {
        return { input: { ...input, operation: "readFile" } };
    }
}
"#,
    );
    let op = OperationPolicies {
        pre: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None,
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "filesystem",
        &op,
        "mcp/filesystem",
        "data.mcp.filesystem.allow",
        HookCaps {
            input_mutation: true,
            post: false,
        },
    )
    .unwrap();
    let engine = build_engine().with_fs_config(FsConfig::new_with_hooks(Arc::new(chain)));

    let out = eval(
        &engine,
        format!(r#"fs.writeFile("{data_dir_str}/a.txt", "x")"#),
    )
    .await;
    assert!(
        out.starts_with("ERROR:") && out.contains("pre hook changed 'operation'"),
        "got: {out}"
    );
    assert!(!data_dir.join("a.txt").exists(), "the write must not run");
}

// ── fs: capability-bearing JS hook audits writes to a log file ──────────────

/// A JS pre hook granted the `fs` capability appends an audit line for every
/// guest write, then abstains — the guest's own operations run unchanged.
/// The hook's `fs.appendFile` is ungated (no recursion into the chain).
#[tokio::test]
async fn fs_js_hook_with_fs_capability_audits_guest_writes() {
    ensure_v8();
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    let data_dir_str = data_dir.to_string_lossy().into_owned();
    let log_path = dir.path().join("audit.log");

    let hook_url = write_rego(
        dir.path(),
        "audit.js",
        &format!(
            r#"
const LOG = {log:?};
async function pre(input) {{
    if (["writeFile", "appendFile", "rename", "remove"].includes(input.operation)) {{
        await fs.appendFile(LOG, input.operation + " " + input.path + "\n");
    }}
    // No return value: the hook observes and abstains.
}}
"#,
            log = log_path.to_string_lossy(),
        ),
    );

    let op = OperationPolicies {
        pre: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None,
            timeout_ms: None,
            capabilities: Some(vec!["fs".to_string()]),
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "filesystem",
        &op,
        "mcp/filesystem",
        "data.mcp.filesystem.allow",
        HookCaps {
            input_mutation: true,
            post: false,
        },
    )
    .unwrap();
    let engine = build_engine().with_fs_config(FsConfig::new_with_hooks(Arc::new(chain)));

    // The guest writes two files; both succeed and both are audited.
    let out = eval(
        &engine,
        format!(
            r#"fs.writeFile("{data_dir_str}/one.txt", "1")
                .then(() => fs.writeFile("{data_dir_str}/two.txt", "2"))
                .then(() => fs.readFile("{data_dir_str}/one.txt", "utf8"))"#
        ),
    )
    .await;
    assert_eq!(out, "1");
    assert!(data_dir.join("two.txt").exists());

    let log = std::fs::read_to_string(&log_path).unwrap();
    assert_eq!(
        log,
        format!(
            "writeFile {data_dir_str}/one.txt\nwriteFile {data_dir_str}/two.txt\n"
        ),
        "reads are not audited, writes are"
    );
}

// ── gate-only op refuses mutation ───────────────────────────────────────────

#[tokio::test]
async fn gate_only_chain_rejects_mutation_at_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let hook_url = write_rego(
        dir.path(),
        "hook.rego",
        r#"
package mcp.websocket

pre := {"input": object.union(input, {"url": "wss://elsewhere"})}
"#,
    );
    let op = OperationPolicies {
        pre: vec![HookSource {
            url: hook_url,
            policy_path: None,
            rule: None,
            timeout_ms: None,
            capabilities: None,
        }],
        ..Default::default()
    };
    let chain = build_hook_chain(
        "websocket",
        &op,
        "mcp/websocket",
        "data.mcp.websocket.allow",
        HookCaps {
            input_mutation: false,
            post: false,
        },
    )
    .unwrap();

    let err = chain
        .run_pre(serde_json::json!({"url": "wss://example.com"}))
        .await
        .expect_err("mutation on a gate-only op must fail closed");
    assert!(err.contains("does not support input mutation"), "got: {err}");
}

// ── HookChain::from_policy keeps plain policy configs working ───────────────

#[tokio::test]
async fn plain_policy_chain_still_gates_fetch() {
    ensure_v8();
    let base = start_server().await;
    let dir = tempfile::tempdir().unwrap();
    let policy_url = write_rego(
        dir.path(),
        "policy.rego",
        r#"
package mcp.fetch

default allow = false

allow if { input.method == "GET" }
"#,
    );
    let op = OperationPolicies {
        policies: vec![PolicySource {
            url: policy_url,
            policy_path: None,
            rule: None,
        }],
        ..Default::default()
    };
    let chain: HookChain = build_hook_chain(
        "fetch",
        &op,
        "mcp/fetch",
        "data.mcp.fetch.allow",
        HookCaps {
            input_mutation: true,
            post: true,
        },
    )
    .unwrap();
    let engine = build_engine().with_fetch_config(FetchConfig::new_with_hooks(Arc::new(chain)));

    let out = eval(
        &engine,
        format!(r#"fetch("{base}/echo").then(r => r.text())"#),
    )
    .await;
    assert_eq!(out, "echo hooked=absent");

    let out = eval(
        &engine,
        format!(r#"fetch("{base}/echo", {{method: "POST"}}).then(r => r.text())"#),
    )
    .await;
    assert!(out.contains("denied by policy"), "got: {out}");
}
