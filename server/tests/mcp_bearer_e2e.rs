//! Exercises file-reference auth through the real client manager and rmcp HTTP transport.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use server::engine::mcp_client::{
    McpClientManager, McpServerAuth, McpServerConfig, McpServerTransport,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct FixtureMcp {
    tool_router: ToolRouter<Self>,
}
impl FixtureMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}
#[tool_router]
impl FixtureMcp {
    #[tool(description = "A local fixture tool")]
    fn ping(&self) -> String {
        "connected".into()
    }
}
#[tool_handler(router = self.tool_router)]
impl ServerHandler for FixtureMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}
#[derive(Clone)]
struct AuthState {
    expected: Arc<Mutex<String>>,
    accepted: Arc<AtomicUsize>,
    rejected: Arc<AtomicUsize>,
}
async fn require_token(State(state): State<AuthState>, request: Request, next: Next) -> Response {
    let expected = format!("Bearer {}", state.expected.lock().unwrap());
    if request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some(expected.as_str())
    {
        state.rejected.fetch_add(1, Ordering::SeqCst);
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.accepted.fetch_add(1, Ordering::SeqCst);
    next.run(request).await
}
async fn require_no_token(request: Request, next: Next) -> Response {
    if request.headers().contains_key("authorization") {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}
fn service(cancel: CancellationToken) -> StreamableHttpService<FixtureMcp, LocalSessionManager> {
    StreamableHttpService::new(
        || Ok(FixtureMcp::new()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(cancel),
    )
}

#[tokio::test]
async fn file_auth_connects_isolated_and_reloads_on_reconnect()
-> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("token");
        std::fs::write(&path, "fixture-token-one\n")?;
        let state = AuthState {
            expected: Arc::new(Mutex::new("fixture-token-one".into())),
            accepted: Arc::new(AtomicUsize::new(0)),
            rejected: Arc::new(AtomicUsize::new(0)),
        };
        let cancel = CancellationToken::new();
        let protected = Router::new()
            .nest_service("/protected", service(cancel.child_token()))
            .route_layer(middleware::from_fn_with_state(state.clone(), require_token));
        let public = Router::new()
            .nest_service("/public", service(cancel.child_token()))
            .route_layer(middleware::from_fn(require_no_token));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, protected.merge(public))
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        let manager = McpClientManager::connect(vec![
            McpServerConfig {
                name: "weechat".into(),
                transport: McpServerTransport::Http {
                    url: format!("{base}/protected"),
                },
                auth: Some(McpServerAuth::Bearer {
                    token_file: path.display().to_string(),
                }),
            },
            McpServerConfig {
                name: "public".into(),
                transport: McpServerTransport::Http {
                    url: format!("{base}/public"),
                },
                auth: None,
            },
        ])
        .await?;
        assert_eq!(manager.server_names().len(), 2);
        assert!(
            manager
                .call_tool("weechat", "ping", None)
                .await?
                .to_string()
                .contains("connected")
        );
        assert!(
            manager
                .call_tool("public", "ping", None)
                .await?
                .to_string()
                .contains("connected")
        );
        let before = state.accepted.load(Ordering::SeqCst);
        std::fs::write(&path, "fixture-token-two\r\n")?;
        *state.expected.lock().unwrap() = "fixture-token-two".into();
        // The old connection fails, probes, reconnects, rereads the file, and retries once.
        assert!(
            manager
                .call_tool("weechat", "ping", None)
                .await?
                .to_string()
                .contains("connected")
        );
        assert!(state.rejected.load(Ordering::SeqCst) > 0);
        assert!(state.accepted.load(Ordering::SeqCst) > before);
        manager.shutdown().await;
        cancel.cancel();
        task.await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn missing_secret_fails_before_network_request() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/mcp", listener.local_addr()?);
    let result = McpClientManager::connect(vec![McpServerConfig {
        name: "missing".into(),
        transport: McpServerTransport::Http { url },
        auth: Some(McpServerAuth::Bearer {
            token_file: temp.path().join("absent").display().to_string(),
        }),
    }])
    .await;
    assert!(matches!(result, Err(error) if error.contains("token_file could not be read")));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    Ok(())
}
