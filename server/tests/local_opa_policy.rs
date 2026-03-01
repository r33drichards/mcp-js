/// Integration tests for local OPA Rego policy evaluation via --policies-json.
///
/// Uses local `.rego` files to gate `fetch()` calls without a remote OPA server.
/// Verifies that policies correctly allow/deny based on input and that the
/// evaluation mode (all/any) works end-to-end.

use std::collections::HashMap;
use std::sync::{Arc, Once};

use axum::{
    Router,
    extract::Request,
    response::Json,
    routing::any,
};
use serde_json::Value;
use server::engine::{initialize_v8, Engine};
use server::engine::fetch::FetchConfig;
use server::engine::execution::ExecutionRegistry;
use server::engine::opa::{
    EvalMode, OperationPolicies, PolicySource, build_policy_chain,
};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

// ── Mock echo server ─────────────────────────────────────────────────────

/// Start a mock target server that echoes request info as JSON.
async fn start_echo_mock() -> String {
    let app = Router::new().route(
        "/",
        any(|req: Request| async move {
            let headers: HashMap<String, String> = req
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        v.to_str().unwrap_or("").to_string(),
                    )
                })
                .collect();
            Json(serde_json::json!({
                "headers": headers,
                "method": req.method().as_str(),
                "uri": req.uri().to_string(),
            }))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://127.0.0.1:{}", port)
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn write_rego(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn build_engine_with_chain(chain: Arc<server::engine::opa::PolicyChain>) -> Engine {
    let fetch_config = FetchConfig::new_with_chain(chain);

    let tmp = std::env::temp_dir().join(format!(
        "mcp-local-opa-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("registry");
    Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_fetch_config(fetch_config)
        .with_execution_registry(Arc::new(registry))
}

async fn run_fetch(engine: &Engine, url: &str, method: &str) -> Result<String, String> {
    let code = format!(
        r#"
        (async () => {{
            const resp = await fetch("{url}", {{ method: "{method}" }});
            return JSON.stringify({{ status: resp.status, ok: resp.ok }});
        }})()
        "#,
        url = url,
        method = method,
    );

    let exec_id = engine
        .run_js(code, None, None, None, None, None)
        .await
        .expect("submit should succeed");

    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return Ok(info.result.expect("should have result")),
                "failed" => return Err(info.error.unwrap_or_default()),
                "timed_out" => return Err("timed out".to_string()),
                _ => continue,
            }
        }
    }
    Err("timeout waiting for execution".to_string())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_local_rego_allows_get() {
    ensure_v8();
    let echo_url = start_echo_mock().await;

    let dir = tempfile::tempdir().unwrap();
    write_rego(dir.path(), "fetch.rego", r#"
package mcp.fetch
default allow = false

allow if {
    input.method == "GET"
}
"#);

    let op = OperationPolicies {
        mode: EvalMode::All,
        policies: vec![PolicySource {
            url: format!("file://{}", dir.path().join("fetch.rego").display()),
            policy_path: None,
            rule: None,
        }],
    };
    let chain = Arc::new(
        build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow").unwrap(),
    );
    let engine = build_engine_with_chain(chain);

    let result = run_fetch(&engine, &echo_url, "GET").await;
    assert!(result.is_ok(), "GET should be allowed. Got: {:?}", result);
}

#[tokio::test]
async fn test_local_rego_denies_post() {
    ensure_v8();
    let echo_url = start_echo_mock().await;

    let dir = tempfile::tempdir().unwrap();
    write_rego(dir.path(), "fetch.rego", r#"
package mcp.fetch
default allow = false

allow if {
    input.method == "GET"
}
"#);

    let op = OperationPolicies {
        mode: EvalMode::All,
        policies: vec![PolicySource {
            url: format!("file://{}", dir.path().join("fetch.rego").display()),
            policy_path: None,
            rule: None,
        }],
    };
    let chain = Arc::new(
        build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow").unwrap(),
    );
    let engine = build_engine_with_chain(chain);

    let result = run_fetch(&engine, &echo_url, "POST").await;
    assert!(result.is_err(), "POST should be denied. Got: {:?}", result);
    assert!(
        result.unwrap_err().contains("denied by policy"),
        "Error should mention policy denial"
    );
}

#[tokio::test]
async fn test_local_rego_uses_existing_policy_file() {
    // Uses the real policies/fetch.rego that ships with the project.
    ensure_v8();
    let echo_url = start_echo_mock().await;

    // The project's fetch.rego allows GET to certain domains, but
    // 127.0.0.1 is NOT in the allowlist — so this should be denied.
    let policy_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../policies/fetch.rego");

    if !policy_path.exists() {
        eprintln!("Skipping: policies/fetch.rego not found at {:?}", policy_path);
        return;
    }

    let op = OperationPolicies {
        mode: EvalMode::All,
        policies: vec![PolicySource {
            url: format!("file://{}", policy_path.canonicalize().unwrap().display()),
            policy_path: None,
            rule: None,
        }],
    };
    let chain = Arc::new(
        build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow").unwrap(),
    );
    let engine = build_engine_with_chain(chain);

    // 127.0.0.1 is not in the allowlist → denied
    let result = run_fetch(&engine, &echo_url, "GET").await;
    assert!(result.is_err(), "127.0.0.1 should be denied by the real policy. Got: {:?}", result);
}

#[tokio::test]
async fn test_policy_chain_all_mode_both_must_allow() {
    ensure_v8();
    let echo_url = start_echo_mock().await;

    let dir = tempfile::tempdir().unwrap();
    // Policy 1: allows GET
    write_rego(dir.path(), "get_only.rego", r#"
package mcp.fetch
default allow = false
allow if { input.method == "GET" }
"#);
    // Policy 2: denies everything (default allow = false, no rules)
    let dir2 = tempfile::tempdir().unwrap();
    write_rego(dir2.path(), "deny_all.rego", r#"
package mcp.fetch
default allow = false
"#);

    let op = OperationPolicies {
        mode: EvalMode::All, // BOTH must allow
        policies: vec![
            PolicySource {
                url: format!("file://{}", dir.path().join("get_only.rego").display()),
                policy_path: None,
                rule: None,
            },
            PolicySource {
                url: format!("file://{}", dir2.path().join("deny_all.rego").display()),
                policy_path: None,
                rule: None,
            },
        ],
    };
    let chain = Arc::new(
        build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow").unwrap(),
    );
    let engine = build_engine_with_chain(chain);

    // First allows GET, second denies everything → denied
    let result = run_fetch(&engine, &echo_url, "GET").await;
    assert!(result.is_err(), "ALL mode: should deny when second policy denies. Got: {:?}", result);
}

#[tokio::test]
async fn test_policy_chain_any_mode_one_suffices() {
    ensure_v8();
    let echo_url = start_echo_mock().await;

    let dir = tempfile::tempdir().unwrap();
    // Policy 1: denies everything
    write_rego(dir.path(), "deny_all.rego", r#"
package mcp.fetch
default allow = false
"#);
    // Policy 2: allows GET
    let dir2 = tempfile::tempdir().unwrap();
    write_rego(dir2.path(), "allow_get.rego", r#"
package mcp.fetch
default allow = false
allow if { input.method == "GET" }
"#);

    let op = OperationPolicies {
        mode: EvalMode::Any, // ANY can allow
        policies: vec![
            PolicySource {
                url: format!("file://{}", dir.path().join("deny_all.rego").display()),
                policy_path: None,
                rule: None,
            },
            PolicySource {
                url: format!("file://{}", dir2.path().join("allow_get.rego").display()),
                policy_path: None,
                rule: None,
            },
        ],
    };
    let chain = Arc::new(
        build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow").unwrap(),
    );
    let engine = build_engine_with_chain(chain);

    // First denies, second allows GET → allowed (Any mode)
    let result = run_fetch(&engine, &echo_url, "GET").await;
    assert!(result.is_ok(), "ANY mode: should allow when second policy allows. Got: {:?}", result);

    // Both deny POST → denied
    let result = run_fetch(&engine, &echo_url, "POST").await;
    assert!(result.is_err(), "ANY mode: should deny when all policies deny. Got: {:?}", result);
}

#[tokio::test]
async fn test_directory_of_rego_files() {
    ensure_v8();
    let echo_url = start_echo_mock().await;

    let dir = tempfile::tempdir().unwrap();
    // Two files in same package — rules combine (OR within same package)
    write_rego(dir.path(), "a_get.rego", r#"
package mcp.fetch
default allow = false
allow if { input.method == "GET" }
"#);
    write_rego(dir.path(), "b_head.rego", r#"
package mcp.fetch
allow if { input.method == "HEAD" }
"#);

    let op = OperationPolicies {
        mode: EvalMode::All,
        policies: vec![PolicySource {
            url: format!("file://{}", dir.path().display()),
            policy_path: None,
            rule: None,
        }],
    };
    let chain = Arc::new(
        build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow").unwrap(),
    );
    let engine = build_engine_with_chain(chain);

    // GET is allowed by a_get.rego
    let result = run_fetch(&engine, &echo_url, "GET").await;
    assert!(result.is_ok(), "GET should be allowed by directory policy. Got: {:?}", result);

    // POST is not allowed by either file
    let result = run_fetch(&engine, &echo_url, "POST").await;
    assert!(result.is_err(), "POST should be denied by directory policy. Got: {:?}", result);
}
