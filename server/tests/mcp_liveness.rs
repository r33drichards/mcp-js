//! Loopback-only demand liveness and cold-wake regression tests.
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use server::engine::mcp_client::{McpClientManager, McpServerConfig};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
};
use std::time::Duration;

#[derive(Default)]
struct Counters {
    requests: AtomicUsize,
    initializes: AtomicUsize,
    calls: AtomicUsize,
    stale: AtomicBool,
    wake_failures: AtomicUsize,
    fail_call: AtomicBool,
    expire_call: AtomicBool,
    stall_probe: AtomicBool,
}

async fn rpc(State(state): State<Arc<Counters>>, Json(body): Json<Value>) -> Response {
    state.requests.fetch_add(1, SeqCst);
    let method = body["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => {
            state.initializes.fetch_add(1, SeqCst);
            if state
                .wake_failures
                .fetch_update(SeqCst, SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return StatusCode::BAD_GATEWAY.into_response();
            }
            state.stale.store(false, SeqCst);
            json!({"protocolVersion":"2025-03-26", "capabilities":{"tools":{}}, "serverInfo":{"name":"mock", "version":"1"}})
        }
        "tools/list" => {
            if state.stall_probe.load(SeqCst) {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            if state.stale.load(SeqCst) {
                return StatusCode::BAD_GATEWAY.into_response();
            }
            json!({"tools":[{"name":"effect", "inputSchema":{"type":"object"}}]})
        }
        "tools/call" => {
            state.calls.fetch_add(1, SeqCst);
            if state.expire_call.load(SeqCst) {
                return StatusCode::NOT_FOUND.into_response();
            }
            if state.fail_call.load(SeqCst) {
                return StatusCode::BAD_GATEWAY.into_response();
            }
            json!({"content":[{"type":"text","text":"done"}], "isError":false})
        }
        _ => return StatusCode::ACCEPTED.into_response(),
    };
    Json(json!({"jsonrpc":"2.0", "id":body["id"], "result":result})).into_response()
}

struct Mock {
    state: Arc<Counters>,
    task: tokio::task::JoinHandle<()>,
    config: McpServerConfig,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Mock {
    async fn start(liveness: Value) -> Self {
        let state = Arc::new(Counters::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/mcp", post(rpc))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let config = serde_json::from_value(json!({"name":"mock", "transport":"http", "url":format!("http://{addr}/mcp"), "liveness":liveness})).unwrap();
        Self {
            state,
            task,
            config,
        }
    }
    async fn manager(&self) -> McpClientManager {
        McpClientManager::connect(vec![self.config.clone()])
            .await
            .unwrap()
    }
}

#[test]
fn config_defaults_and_validation() {
    let base = json!({"name":"mock","transport":"http","url":"http://127.0.0.1/mcp"});
    let config: McpServerConfig = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(config).unwrap()["liveness"],
        json!({"mode":"periodic","interval_secs":20})
    );
    for invalid in [
        json!({"mode":"periodic","interval_secs":0}),
        json!({"mode":"periodic","interval_secs":-1}),
        json!({"mode":"on_demnad"}),
        json!({"mode":"on_demand","interval_secs":20}),
    ] {
        let mut value = base.clone();
        value["liveness"] = invalid;
        assert!(serde_json::from_value::<McpServerConfig>(value).is_err());
    }
}

#[tokio::test]
async fn demand_is_quiet_past_default_interval_and_recovers_on_call() {
    let mock = Mock::start(json!({"mode":"on_demand"})).await;
    let manager = mock.manager().await;
    let before = mock.state.requests.load(SeqCst);
    tokio::time::sleep(Duration::from_secs(21)).await;
    assert_eq!(
        before,
        mock.state.requests.load(SeqCst),
        "idle generated traffic"
    );
    mock.state.stale.store(true, SeqCst);
    mock.state.wake_failures.store(1, SeqCst);
    manager.call_tool("mock", "effect", None).await.unwrap();
    assert_eq!(mock.state.initializes.load(SeqCst), 3);
    assert_eq!(mock.state.calls.load(SeqCst), 1);
    manager.shutdown().await;
}

#[tokio::test]
async fn periodic_interval_is_configurable_and_shutdown_stops_it() {
    let mock = Mock::start(json!({"mode":"periodic","interval_secs":1})).await;
    let manager = mock.manager().await;
    let before = mock.state.requests.load(SeqCst);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(mock.state.requests.load(SeqCst) > before);
    manager.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = mock.state.requests.load(SeqCst);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(before, mock.state.requests.load(SeqCst));
}

#[tokio::test]
async fn demand_never_replays_an_ambiguous_tool_failure() {
    let mock = Mock::start(json!({"mode":"on_demand"})).await;
    let manager = mock.manager().await;
    mock.state.fail_call.store(true, SeqCst);
    let error = manager.call_tool("mock", "effect", None).await.unwrap_err();
    assert!(error.contains("not replayed"), "{error}");
    assert_eq!(mock.state.calls.load(SeqCst), 1);
    assert_eq!(mock.state.initializes.load(SeqCst), 1);
    manager.shutdown().await;
}

#[tokio::test]
async fn concurrent_demand_reconnects_only_once() {
    let mock = Mock::start(json!({"mode":"on_demand"})).await;
    let manager = mock.manager().await;
    mock.state.stale.store(true, SeqCst);
    let (a, b) = tokio::join!(
        manager.call_tool("mock", "effect", None),
        manager.call_tool("mock", "effect", None)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(mock.state.initializes.load(SeqCst), 2);
    assert_eq!(mock.state.calls.load(SeqCst), 2);
    manager.shutdown().await;
}

#[tokio::test]
async fn demand_unavailable_does_not_dispatch() {
    let mock = Mock::start(json!({"mode":"on_demand"})).await;
    let manager = mock.manager().await;
    mock.state.stale.store(true, SeqCst);
    mock.state.wake_failures.store(99, SeqCst);
    let error = manager.call_tool("mock", "effect", None).await.unwrap_err();
    assert!(error.contains("tool not dispatched"), "{error}");
    assert_eq!(mock.state.calls.load(SeqCst), 0);
    assert_eq!(mock.state.initializes.load(SeqCst), 4);
    manager.shutdown().await;
}

#[tokio::test]
async fn shutdown_cancels_stalled_readiness() {
    let mock = Mock::start(json!({"mode":"on_demand"})).await;
    let manager = mock.manager().await;
    mock.state.stall_probe.store(true, SeqCst);
    let calling = manager.clone();
    let call = tokio::spawn(async move { calling.call_tool("mock", "effect", None).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    manager.shutdown().await;
    let error = tokio::time::timeout(Duration::from_secs(1), call)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.contains("shut down"));
    assert_eq!(mock.state.calls.load(SeqCst), 0);
    assert_eq!(mock.state.initializes.load(SeqCst), 1);
}

#[tokio::test]
async fn demand_startup_retries_cold_wake() {
    let mock = Mock::start(json!({"mode":"on_demand"})).await;
    mock.state.wake_failures.store(1, SeqCst);
    let manager = mock.manager().await;
    assert_eq!(mock.state.initializes.load(SeqCst), 2);
    assert_eq!(mock.state.calls.load(SeqCst), 0);
    manager.call_tool("mock", "effect", None).await.unwrap();
    manager.shutdown().await;
}

#[tokio::test]
async fn demand_disables_transport_replay_on_session_expiration() {
    let mock = Mock::start(json!({"mode":"on_demand"})).await;
    let manager = mock.manager().await;
    mock.state.expire_call.store(true, SeqCst);
    let error = manager.call_tool("mock", "effect", None).await.unwrap_err();
    assert!(error.contains("not replayed"), "{error}");
    assert_eq!(mock.state.calls.load(SeqCst), 1);
    assert_eq!(mock.state.initializes.load(SeqCst), 1);
    manager.shutdown().await;
}
