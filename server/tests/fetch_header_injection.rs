/// Integration tests for fetch header injection.
///
/// Uses a local allow-all Rego policy together with mock token and echo
/// servers, configures an Engine with fetch header rules, runs JS `fetch()`
/// calls, and asserts against the real outbound requests.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Once};

use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, post},
    Json, Router,
};
use serde_json::{Value, json};
use server::engine::execution::ExecutionRegistry;
use server::engine::fetch::{FetchConfig, HeaderRule, OAuthClientCredentialsConfig};
use server::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

#[derive(Clone)]
struct TestTokenServer {
    base_url: String,
    state: TestTokenServerState,
}

impl TestTokenServer {
    fn token_url(&self) -> String {
        format!("{}/token", self.base_url)
    }

    async fn requests(&self) -> Vec<TestTokenRequest> {
        self.state.requests.lock().await.clone()
    }
}

#[derive(Clone)]
struct TestTokenServerState {
    responses: Arc<tokio::sync::Mutex<VecDeque<TestTokenResponse>>>,
    requests: Arc<tokio::sync::Mutex<Vec<TestTokenRequest>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TestTokenResponse {
    status: StatusCode,
    body: Value,
}

impl TestTokenResponse {
    fn success(body: Value) -> Self {
        Self {
            status: StatusCode::OK,
            body,
        }
    }

    fn failure(status: StatusCode, body: Value) -> Self {
        Self { status, body }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TestTokenRequest {
    grant_type: String,
}

async fn start_token_server(responses: Vec<TestTokenResponse>) -> TestTokenServer {
    async fn token_handler(
        State(state): State<TestTokenServerState>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Response {
        state.requests.lock().await.push(TestTokenRequest {
            grant_type: form.get("grant_type").cloned().unwrap_or_default(),
        });

        let response = state.responses.lock().await.pop_front().unwrap_or_else(|| {
            TestTokenResponse::failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error":"no_more_responses"}),
            )
        });

        (response.status, Json(response.body)).into_response()
    }

    let state = TestTokenServerState {
        responses: Arc::new(tokio::sync::Mutex::new(VecDeque::from(responses))),
        requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };

    let app = Router::new()
        .route("/token", post(token_handler))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestTokenServer {
        base_url: format!("http://{}", address),
        state,
    }
}

#[derive(Clone)]
struct EchoServer {
    url: String,
    state: EchoServerState,
}

impl EchoServer {
    async fn requests(&self) -> Vec<EchoRequestRecord> {
        self.state.requests.lock().await.clone()
    }
}

#[derive(Clone)]
struct EchoServerState {
    requests: Arc<tokio::sync::Mutex<Vec<EchoRequestRecord>>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct EchoRequestRecord {
    method: String,
    authorization: Option<String>,
}

async fn start_echo_server() -> EchoServer {
    async fn echo_handler(
        State(state): State<EchoServerState>,
        headers: HeaderMap,
        method: axum::http::Method,
    ) -> impl IntoResponse {
        state.requests.lock().await.push(EchoRequestRecord {
            method: method.as_str().to_string(),
            authorization: headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned),
        });

        (StatusCode::OK, Json(json!({"ok": true})))
    }

    let state = EchoServerState {
        requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };

    let app = Router::new()
        .route("/resource", any(echo_handler))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    EchoServer {
        url: format!("http://{}/resource", address),
        state,
    }
}

fn allow_all_chain() -> Arc<PolicyChain> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("allow_all.rego");
    std::fs::write(&path, "package mcp.fetch\ndefault allow = true\n").unwrap();
    std::mem::forget(dir);

    let evaluator =
        LocalPolicyEvaluator::from_file(&path, "data.mcp.fetch.allow".to_string()).unwrap();
    Arc::new(PolicyChain::new(
        vec![PolicyEvaluatorKind::Local(evaluator)],
        EvalMode::All,
    ))
}

fn build_engine(header_rules: Vec<HeaderRule>) -> Engine {
    let fetch_config = FetchConfig::new_with_chain(allow_all_chain()).with_header_rules(header_rules);
    let tmp = std::env::temp_dir().join(format!(
        "mcp-fetch-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry = ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");

    Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_fetch_config(fetch_config)
        .with_execution_registry(Arc::new(registry))
}

fn oauth_rule_for_host(host: &str, methods: &[&str], token_url: String) -> HeaderRule {
    HeaderRule::oauth_client_credentials(
        host.to_string(),
        methods.iter().map(|method| method.to_string()).collect(),
        OAuthClientCredentialsConfig {
            header_name: "Authorization".to_string(),
            token_url,
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            scope: Some("read:all".to_string()),
            refresh_buffer_secs: 0,
        },
    )
    .expect("rule should be valid")
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

async fn run_fetch(
    engine: &Engine,
    url: &str,
    method: &str,
    authorization: Option<&str>,
) -> Result<(), String> {
    let headers_expr = match authorization {
        Some(value) => format!(
            r#"{{ Authorization: {} }}"#,
            serde_json::to_string(value).expect("authorization should serialize")
        ),
        None => "{}".to_string(),
    };
    let code = format!(
        r#"
        (async () => {{
            const resp = await fetch("{url}", {{
                method: "{method}",
                headers: {headers_expr},
            }});
            if (!resp.ok) {{
                throw new Error("unexpected status " + resp.status);
            }}
            return JSON.stringify(await resp.json());
        }})()
        "#,
        url = url,
        method = method,
        headers_expr = headers_expr,
    );

    run_js(engine, code).await.map(|_| ())
}

#[tokio::test]
async fn matching_request_receives_injected_bearer_token_from_mock_token_server() {
    ensure_v8();

    let token_server = start_token_server(vec![TestTokenResponse::success(json!({
        "access_token": "dynamic-token",
        "token_type": "Bearer",
        "expires_in": 3600
    }))])
    .await;
    let echo_server = start_echo_server().await;
    let engine = build_engine(vec![oauth_rule_for_host(
        "127.0.0.1",
        &[],
        token_server.token_url(),
    )]);

    run_fetch(&engine, &echo_server.url, "GET", None)
        .await
        .expect("fetch should succeed");

    assert_eq!(
        echo_server.requests().await,
        vec![EchoRequestRecord {
            method: "GET".to_string(),
            authorization: Some("Bearer dynamic-token".to_string()),
        }]
    );
    assert_eq!(
        token_server.requests().await,
        vec![TestTokenRequest {
            grant_type: "client_credentials".to_string(),
        }]
    );
}

#[tokio::test]
async fn repeated_requests_reuse_cached_token() {
    ensure_v8();

    let token_server = start_token_server(vec![TestTokenResponse::success(json!({
        "access_token": "cached-token",
        "token_type": "Bearer",
        "expires_in": 3600
    }))])
    .await;
    let echo_server = start_echo_server().await;
    let engine = build_engine(vec![oauth_rule_for_host(
        "127.0.0.1",
        &[],
        token_server.token_url(),
    )]);

    run_fetch(&engine, &echo_server.url, "GET", None)
        .await
        .expect("first fetch should succeed");
    run_fetch(&engine, &echo_server.url, "GET", None)
        .await
        .expect("second fetch should reuse cached token");

    assert_eq!(
        echo_server.requests().await,
        vec![
            EchoRequestRecord {
                method: "GET".to_string(),
                authorization: Some("Bearer cached-token".to_string()),
            },
            EchoRequestRecord {
                method: "GET".to_string(),
                authorization: Some("Bearer cached-token".to_string()),
            },
        ]
    );
    assert_eq!(
        token_server.requests().await,
        vec![TestTokenRequest {
            grant_type: "client_credentials".to_string(),
        }]
    );
}

#[tokio::test]
async fn post_expiry_request_reacquires_token() {
    ensure_v8();

    let token_server = start_token_server(vec![
        TestTokenResponse::success(json!({
            "access_token": "expired-token",
            "token_type": "Bearer",
            "expires_in": 0
        })),
        TestTokenResponse::success(json!({
            "access_token": "refreshed-token",
            "token_type": "Bearer",
            "expires_in": 3600
        })),
    ])
    .await;
    let echo_server = start_echo_server().await;
    let engine = build_engine(vec![oauth_rule_for_host(
        "127.0.0.1",
        &[],
        token_server.token_url(),
    )]);

    run_fetch(&engine, &echo_server.url, "GET", None)
        .await
        .expect("first fetch should succeed");
    run_fetch(&engine, &echo_server.url, "GET", None)
        .await
        .expect("second fetch should reacquire after expiry");

    assert_eq!(
        echo_server.requests().await,
        vec![
            EchoRequestRecord {
                method: "GET".to_string(),
                authorization: Some("Bearer expired-token".to_string()),
            },
            EchoRequestRecord {
                method: "GET".to_string(),
                authorization: Some("Bearer refreshed-token".to_string()),
            },
        ]
    );
    assert_eq!(
        token_server.requests().await,
        vec![
            TestTokenRequest {
                grant_type: "client_credentials".to_string(),
            },
            TestTokenRequest {
                grant_type: "client_credentials".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn user_provided_authorization_overrides_dynamic_injection() {
    ensure_v8();

    let token_server = start_token_server(Vec::new()).await;
    let echo_server = start_echo_server().await;
    let engine = build_engine(vec![oauth_rule_for_host(
        "127.0.0.1",
        &[],
        token_server.token_url(),
    )]);

    run_fetch(
        &engine,
        &echo_server.url,
        "GET",
        Some("Bearer user-provided"),
    )
    .await
    .expect("user-provided authorization should bypass token lookup");

    assert_eq!(
        echo_server.requests().await,
        vec![EchoRequestRecord {
            method: "GET".to_string(),
            authorization: Some("Bearer user-provided".to_string()),
        }]
    );
    assert!(token_server.requests().await.is_empty());
}

#[tokio::test]
async fn non_matching_host_performs_no_token_lookup() {
    ensure_v8();

    let token_server = start_token_server(Vec::new()).await;
    let echo_server = start_echo_server().await;
    let engine = build_engine(vec![oauth_rule_for_host(
        "other.example.com",
        &[],
        token_server.token_url(),
    )]);

    run_fetch(&engine, &echo_server.url, "GET", None)
        .await
        .expect("fetch should succeed without dynamic auth");

    assert_eq!(
        echo_server.requests().await,
        vec![EchoRequestRecord {
            method: "GET".to_string(),
            authorization: None,
        }]
    );
    assert!(token_server.requests().await.is_empty());
}

#[tokio::test]
async fn non_matching_method_performs_no_token_lookup() {
    ensure_v8();

    let token_server = start_token_server(Vec::new()).await;
    let echo_server = start_echo_server().await;
    let engine = build_engine(vec![oauth_rule_for_host(
        "127.0.0.1",
        &["POST"],
        token_server.token_url(),
    )]);

    run_fetch(&engine, &echo_server.url, "GET", None)
        .await
        .expect("fetch should succeed without dynamic auth");

    assert_eq!(
        echo_server.requests().await,
        vec![EchoRequestRecord {
            method: "GET".to_string(),
            authorization: None,
        }]
    );
    assert!(token_server.requests().await.is_empty());
}
