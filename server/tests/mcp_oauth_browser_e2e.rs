//! Headless end-to-end coverage for downstream browser OAuth.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::extract::{Form, Query, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{
    Json, Router,
    routing::{get, post},
};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use server::engine::mcp_client::{
    McpClientManager, McpServerAuth, McpServerConfig, McpServerTransport,
};
use tokio_util::sync::CancellationToken;

static ENVIRONMENT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EmptyInput {}

#[derive(Clone)]
struct ProtectedMcp {
    tool_router: ToolRouter<Self>,
}

impl ProtectedMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl ProtectedMcp {
    #[tool(description = "Confirms the protected MCP server is available")]
    fn browser_oauth_tool(&self, _: Parameters<EmptyInput>) -> String {
        "connected".to_string()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ProtectedMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[derive(Clone)]
struct MockState {
    base_url: String,
    authorization_requests: Arc<AtomicUsize>,
    registration_requests: Arc<AtomicUsize>,
    token_grants: Arc<Mutex<Vec<HashMap<String, String>>>>,
    mcp_tokens: Arc<Mutex<Vec<String>>>,
    accepted_tokens: Arc<Mutex<HashSet<String>>>,
}

struct MockOAuthMcpServer {
    base_url: String,
    state: MockState,
    cancellation: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl MockOAuthMcpServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let state = MockState {
            base_url: base_url.clone(),
            authorization_requests: Arc::new(AtomicUsize::new(0)),
            registration_requests: Arc::new(AtomicUsize::new(0)),
            token_grants: Arc::new(Mutex::new(Vec::new())),
            mcp_tokens: Arc::new(Mutex::new(Vec::new())),
            accepted_tokens: Arc::new(Mutex::new(HashSet::new())),
        };
        let cancellation = CancellationToken::new();
        let mcp_service: StreamableHttpService<ProtectedMcp, LocalSessionManager> =
            StreamableHttpService::new(
                || Ok(ProtectedMcp::new()),
                LocalSessionManager::default().into(),
                StreamableHttpServerConfig::default()
                    .with_sse_keep_alive(None)
                    .with_cancellation_token(cancellation.child_token()),
            );
        let mcp_routes = Router::new().nest_service("/mcp", mcp_service).route_layer(
            middleware::from_fn_with_state(state.clone(), require_access_token),
        );
        let app = Router::new()
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(protected_resource_metadata),
            )
            .route(
                "/.well-known/oauth-authorization-server",
                get(authorization_metadata),
            )
            .route("/register", post(register_client))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .merge(mcp_routes)
            .with_state(state.clone());
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown.cancelled_owned().await })
                .await;
        });

        Ok(Self {
            base_url,
            state,
            cancellation,
            task,
        })
    }

    fn mcp_url(&self) -> String {
        format!("{}/mcp", self.base_url)
    }
}

impl Drop for MockOAuthMcpServer {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

async fn protected_resource_metadata(State(state): State<MockState>) -> Json<Value> {
    Json(json!({"authorization_servers": [state.base_url]}))
}

async fn authorization_metadata(State(state): State<MockState>) -> Json<Value> {
    Json(json!({
        "issuer": state.base_url,
        "authorization_endpoint": format!("{}/authorize", state.base_url),
        "token_endpoint": format!("{}/token", state.base_url),
        "registration_endpoint": format!("{}/register", state.base_url),
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"]
    }))
}

async fn register_client(
    State(state): State<MockState>,
    Json(request): Json<Value>,
) -> Json<Value> {
    state.registration_requests.fetch_add(1, Ordering::SeqCst);
    Json(json!({
        "client_id": "headless-test-client",
        "client_secret": null,
        "client_name": request["client_name"],
        "redirect_uris": request["redirect_uris"]
    }))
}

async fn authorize(
    State(state): State<MockState>,
    Query(_query): Query<HashMap<String, String>>,
) -> StatusCode {
    state.authorization_requests.fetch_add(1, Ordering::SeqCst);
    StatusCode::OK
}

async fn token(
    State(state): State<MockState>,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    let grant_type = form.get("grant_type").cloned().unwrap_or_default();
    state.token_grants.lock().unwrap().push(form);
    let access_token = match grant_type.as_str() {
        "authorization_code" => "browser-access",
        "refresh_token" => "refreshed-access",
        other => panic!("unexpected OAuth grant type: {other}"),
    };
    state
        .accepted_tokens
        .lock()
        .unwrap()
        .insert(access_token.to_string());
    Json(json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "refresh_token": if grant_type == "authorization_code" { "browser-refresh" } else { "rotated-refresh" },
        "expires_in": 3600
    }))
}

async fn require_access_token(
    State(state): State<MockState>,
    request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    let accepted = state.accepted_tokens.lock().unwrap().contains(&token);
    if !accepted {
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", HeaderValue::from_static("Bearer"))],
        )
            .into_response();
    }
    state.mcp_tokens.lock().unwrap().push(token);
    next.run(request).await
}

struct EnvironmentGuard {
    previous_path: Option<OsString>,
    previous_capture: Option<OsString>,
}

impl EnvironmentGuard {
    fn install(bin_dir: &Path, capture_path: &Path) -> Self {
        let previous_path = std::env::var_os("PATH");
        let previous_capture = std::env::var_os("MCP_OAUTH_CAPTURE");
        let mut paths = vec![bin_dir.to_path_buf()];
        paths.extend(std::env::split_paths(
            previous_path.as_deref().unwrap_or_default(),
        ));
        // The production opener remains unchanged; only this test process resolves its shim first.
        unsafe {
            std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
            std::env::set_var("MCP_OAUTH_CAPTURE", capture_path);
        }
        Self {
            previous_path,
            previous_capture,
        }
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
            match &self.previous_capture {
                Some(value) => std::env::set_var("MCP_OAUTH_CAPTURE", value),
                None => std::env::remove_var("MCP_OAUTH_CAPTURE"),
            }
        }
    }
}

#[cfg(unix)]
fn install_headless_browser(
    directory: &Path,
) -> Result<(PathBuf, EnvironmentGuard), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = directory.join("bin");
    std::fs::create_dir(&bin_dir)?;
    let capture_path = directory.join("authorization-url");
    let opener = bin_dir.join("xdg-open");
    std::fs::write(
        &opener,
        "#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$MCP_OAUTH_CAPTURE\"\n",
    )?;
    std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755))?;
    let guard = EnvironmentGuard::install(&bin_dir, &capture_path);
    Ok((capture_path, guard))
}

fn opener_invocations(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|captured| captured.lines().count())
        .unwrap_or(0)
}

async fn wait_for_authorization_url(path: &Path) -> Result<url::Url, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(url) = std::fs::read_to_string(path) {
                if let Ok(url) = url.trim().parse() {
                    return url;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| "timed out waiting for the headless browser URL".into())
}

fn pkce_s256(verifier: &str) -> String {
    use sha2::{Digest, Sha256};

    const BASE64_URL: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let digest = Sha256::digest(verifier.as_bytes());
    let mut encoded = String::new();
    for chunk in digest.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(BASE64_URL[((value >> 18) & 0x3f) as usize] as char);
        encoded.push(BASE64_URL[((value >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(BASE64_URL[((value >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            encoded.push(BASE64_URL[(value & 0x3f) as usize] as char);
        }
    }
    encoded
}

fn browser_oauth_config(server: &MockOAuthMcpServer, cache_path: &Path) -> McpServerConfig {
    McpServerConfig {
        name: "protected".to_string(),
        transport: McpServerTransport::Http {
            url: server.mcp_url(),
        },
        auth: Some(McpServerAuth::OauthBrowser {
            scope: Some(vec!["tools.read".to_string()]),
            client_id: None,
            client_secret: None,
            redirect_port: Some(0),
            token_cache: Some(cache_path.display().to_string()),
        }),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn browser_oauth_authorizes_reuses_cache_and_refreshes_headlessly()
-> Result<(), Box<dyn std::error::Error>> {
    let _environment_lock = ENVIRONMENT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();
    let directory = tempfile::tempdir()?;
    let (capture_path, _environment_guard) = install_headless_browser(directory.path())?;
    let server = MockOAuthMcpServer::start().await?;
    let cache_path = directory.path().join("oauth-cache.json");
    let config = browser_oauth_config(&server, &cache_path);

    let mut first_connection = tokio::spawn(McpClientManager::connect(vec![config.clone()]));
    let authorization_url = tokio::select! {
        result = &mut first_connection => match result {
            Ok(Err(error)) => return Err(error.into()),
            Ok(Ok(_)) => return Err("OAuth connection succeeded without opening a browser".into()),
            Err(error) => return Err(error.into()),
        },
        url = wait_for_authorization_url(&capture_path) => url?,
    };
    let query: HashMap<_, _> = authorization_url.query_pairs().into_owned().collect();
    assert_eq!(authorization_url.path(), "/authorize");
    assert_eq!(
        query.get("code_challenge_method"),
        Some(&"S256".to_string())
    );
    assert!(
        query
            .get("code_challenge")
            .is_some_and(|value| !value.is_empty())
    );
    let state = query.get("state").expect("authorization state").to_string();
    let redirect_uri = query.get("redirect_uri").expect("redirect URI").to_string();
    assert_eq!(opener_invocations(&capture_path), 1);

    reqwest::get(authorization_url.clone())
        .await?
        .error_for_status()?;
    let rejected_callback = url::Url::parse_with_params(
        &redirect_uri,
        [("code", "wrong-state-code"), ("state", "wrong-state")],
    )?;
    reqwest::get(rejected_callback).await?.error_for_status()?;
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut first_connection)
            .await
            .is_err(),
        "a callback with the wrong state must not complete OAuth"
    );

    let callback = url::Url::parse_with_params(
        &redirect_uri,
        [("code", "headless-code"), ("state", &state)],
    )?;
    reqwest::get(callback).await?.error_for_status()?;
    let first = first_connection.await??;
    let tools = first.list_tools(Some("protected"))?;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "browser_oauth_tool");
    drop(first);

    let grants = server.state.token_grants.lock().unwrap().clone();
    assert_eq!(grants.len(), 1);
    assert_eq!(
        grants[0].get("grant_type"),
        Some(&"authorization_code".to_string())
    );
    assert_eq!(grants[0].get("code"), Some(&"headless-code".to_string()));
    let verifier = grants[0].get("code_verifier").expect("PKCE verifier");
    assert_eq!(pkce_s256(verifier), *query.get("code_challenge").unwrap());
    assert_eq!(
        server.state.authorization_requests.load(Ordering::SeqCst),
        1
    );
    assert_eq!(server.state.registration_requests.load(Ordering::SeqCst), 1);

    let cached = tokio::time::timeout(
        Duration::from_secs(2),
        McpClientManager::connect(vec![config.clone()]),
    )
    .await
    .map_err(|_| "cache reuse unexpectedly waited for browser authorization")??;
    assert_eq!(cached.list_tools(Some("protected"))?.len(), 1);
    drop(cached);
    assert_eq!(opener_invocations(&capture_path), 1);
    assert_eq!(
        server.state.authorization_requests.load(Ordering::SeqCst),
        1
    );
    assert_eq!(server.state.registration_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server.state.token_grants.lock().unwrap().len(), 1);

    let mut cache: Value = serde_json::from_slice(&std::fs::read(&cache_path)?)?;
    cache["credentials"]["token_received_at"] = json!(0);
    std::fs::write(&cache_path, serde_json::to_vec(&cache)?)?;

    let refreshed = tokio::time::timeout(
        Duration::from_secs(2),
        McpClientManager::connect(vec![config]),
    )
    .await
    .map_err(|_| "token refresh unexpectedly waited for browser authorization")??;
    assert_eq!(refreshed.list_tools(Some("protected"))?.len(), 1);
    assert_eq!(opener_invocations(&capture_path), 1);
    let grants = server.state.token_grants.lock().unwrap().clone();
    assert_eq!(grants.len(), 2);
    assert_eq!(
        grants[1].get("grant_type"),
        Some(&"refresh_token".to_string())
    );
    assert_eq!(
        grants[1].get("refresh_token"),
        Some(&"browser-refresh".to_string())
    );
    assert_eq!(
        server.state.authorization_requests.load(Ordering::SeqCst),
        1
    );
    assert_eq!(server.state.registration_requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        server.state.mcp_tokens.lock().unwrap().last(),
        Some(&"refreshed-access".to_string())
    );

    Ok(())
}
