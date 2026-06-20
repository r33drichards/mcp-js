/// Integration tests for MCP server authentication.
///
/// Tests all three auth modes for `--mcp-config` downstream MCP server
/// connections:
///   1. Bearer — static token
///   2. ClientCredentials — OAuth 2.0 client_credentials grant
///   3. OauthDiscovery — full MCP spec discovery (RFC 9728 + RFC 8414)
///
/// Each test spins up:
///   * A mock "downstream MCP server" that requires Authorization headers
///   * (For OAuth) A mock token server
///   * (For discovery) A mock resource metadata + AS metadata endpoint
///   * The main mcp-v8 process configured via `--mcp-config` to connect
///
/// We verify that mcp-v8 successfully connects (tools/list includes the
/// downstream server's tools), meaning auth was applied correctly.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write as _;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

mod common;

// ── Helpers ─────────────────────────────────────────────────────────────

/// A minimal MCP server that requires a Bearer token and exposes one tool.
/// If the token is missing/wrong, returns 401. Otherwise speaks stdio MCP.
///
/// Since the downstream MCP transport is stdio or HTTP, for these tests we
/// use the real mcp-v8 binary as the downstream (it doesn't require auth on
/// stdio) and instead verify auth at the HTTP transport layer.
///
/// For HTTP-transport tests, we spin up a mock HTTP "MCP server" that:
///   - Validates the Authorization header
///   - Responds to POST /mcp with a minimal tools/list response

#[derive(Clone)]
struct MockProtectedMcpServer {
    expected_token: String,
    requests: Arc<Mutex<Vec<String>>>,
}

async fn mock_mcp_handler(
    State(state): State<MockProtectedMcpServer>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    // Record the auth header
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    state.requests.lock().await.push(auth.clone());

    let expected = format!("Bearer {}", state.expected_token);
    if auth != expected {
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", "Bearer realm=\"mcp\"")],
            "Unauthorized",
        )
            .into_response();
    }

    // Parse the JSON-RPC request and respond appropriately
    let request: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response();
        }
    };

    let method = request["method"].as_str().unwrap_or("");
    let id = &request["id"];

    let response = match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "mock-protected", "version": "1.0" }
            }
        }),
        "notifications/initialized" => return (StatusCode::OK, "").into_response(),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [{
                    "name": "protected_tool",
                    "description": "A tool behind auth",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "input": { "type": "string" } }
                    }
                }]
            }
        }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "Method not found" }
        }),
    };

    (StatusCode::OK, Json(response)).into_response()
}

async fn start_protected_mcp_server(expected_token: &str) -> (String, Arc<Mutex<Vec<String>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = MockProtectedMcpServer {
        expected_token: expected_token.to_string(),
        requests: requests.clone(),
    };

    let app = Router::new()
        .route("/mcp", post(mock_mcp_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{}/mcp", addr), requests)
}

// ── Token server (for client_credentials tests) ─────────────────────────

#[derive(Clone)]
struct MockTokenServer {
    expected_client_id: String,
    expected_client_secret: String,
    token_to_issue: String,
    requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

async fn token_handler(
    State(state): State<MockTokenServer>,
    axum::extract::Form(form): axum::extract::Form<HashMap<String, String>>,
) -> impl IntoResponse {
    state.requests.lock().await.push(form.clone());

    let grant_type = form.get("grant_type").map(|s| s.as_str()).unwrap_or("");
    let client_id = form.get("client_id").map(|s| s.as_str()).unwrap_or("");
    let client_secret = form.get("client_secret").map(|s| s.as_str()).unwrap_or("");

    if grant_type != "client_credentials" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unsupported_grant_type"})),
        )
            .into_response();
    }

    if client_id != state.expected_client_id || client_secret != state.expected_client_secret {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "access_token": state.token_to_issue,
            "token_type": "Bearer",
            "expires_in": 3600
        })),
    )
        .into_response()
}

async fn start_token_server(
    client_id: &str,
    client_secret: &str,
    token: &str,
) -> (String, Arc<Mutex<Vec<HashMap<String, String>>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = MockTokenServer {
        expected_client_id: client_id.to_string(),
        expected_client_secret: client_secret.to_string(),
        token_to_issue: token.to_string(),
        requests: requests.clone(),
    };

    let app = Router::new()
        .route("/token", post(token_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{}/token", addr), requests)
}

// ── OAuth Discovery mock (RFC 9728 + RFC 8414) ─────────────────────────

/// Sets up endpoints for:
///   - Protected Resource Metadata at /.well-known/oauth-protected-resource
///   - Authorization Server Metadata at /.well-known/oauth-authorization-server
///   - Token endpoint at /oauth/token
async fn start_discovery_server(
    client_id: &str,
    client_secret: &str,
    token: &str,
    mcp_server_url: &str,
) -> String {
    let token_state = MockTokenServer {
        expected_client_id: client_id.to_string(),
        expected_client_secret: client_secret.to_string(),
        token_to_issue: token.to_string(),
        requests: Arc::new(Mutex::new(Vec::new())),
    };

    let mcp_url = mcp_server_url.to_string();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://{}", addr);
    let issuer_clone = issuer.clone();

    let app = Router::new()
        // RFC 9728: Protected Resource Metadata
        .route(
            "/.well-known/oauth-protected-resource",
            get(move || async move {
                Json(json!({
                    "resource": mcp_url,
                    "authorization_servers": [issuer_clone],
                    "scopes_supported": ["mcp:read", "mcp:write"]
                }))
            }),
        )
        // RFC 8414: Authorization Server Metadata
        .route(
            "/.well-known/oauth-authorization-server",
            {
                let issuer2 = issuer.clone();
                get(move || async move {
                    Json(json!({
                        "issuer": issuer2,
                        "token_endpoint": format!("{}/oauth/token", issuer2),
                        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
                        "grant_types_supported": ["client_credentials", "authorization_code"],
                        "response_types_supported": ["code"],
                        "code_challenge_methods_supported": ["S256"],
                        "scopes_supported": ["mcp:read", "mcp:write"]
                    }))
                })
            },
        )
        // Token endpoint
        .route("/oauth/token", post(token_handler))
        .with_state(token_state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    issuer
}

// ── mcp-v8 process wrapper ──────────────────────────────────────────────

struct McpV8Process {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl McpV8Process {
    async fn start_with_config(config_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let server_bin = env!("CARGO_BIN_EXE_server");

        let heap_dir = common::create_temp_heap_dir();
        std::fs::create_dir_all(&heap_dir).ok();

        let mut child = Command::new(server_bin)
            .args([
                "--heap-store",
                "dir",
                "--heap-dir",
                &heap_dir,
                "--mcp-config",
                config_path,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));

        // Give server time to start and connect to downstream MCP servers.
        tokio::time::sleep(Duration::from_millis(3000)).await;

        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    async fn send(&mut self, msg: Value) -> Result<Value, Box<dyn std::error::Error>> {
        let s = format!("{}\n", serde_json::to_string(&msg)?);
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.flush().await?;
        let mut line = String::new();
        timeout(Duration::from_secs(10), self.stdout.read_line(&mut line)).await??;
        Ok(serde_json::from_str(&line)?)
    }

    async fn send_notification(&mut self, msg: Value) -> Result<(), Box<dyn std::error::Error>> {
        let s = format!("{}\n", serde_json::to_string(&msg)?);
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let resp = self
            .send(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "auth-test", "version": "1.0.0"}
                }
            }))
            .await?;
        assert!(resp.get("result").is_some(), "initialize failed: {:?}", resp);

        self.send_notification(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .await?;
        Ok(())
    }

    async fn list_tools(&mut self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let resp = self
            .send(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }))
            .await?;

        let tools = resp["result"]["tools"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(tools)
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

fn write_mcp_config(configs: &[Value]) -> String {
    let path = std::env::temp_dir().join(format!(
        "mcp-auth-test-config-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut file = std::fs::File::create(&path).unwrap();
    write!(file, "{}", serde_json::to_string_pretty(configs).unwrap()).unwrap();
    path.to_string_lossy().to_string()
}

// ── Test: Bearer auth ───────────────────────────────────────────────────

/// Scenario 1: downstream MCP server protected by a static bearer token.
/// mcp-v8 is configured with `auth.type = "bearer"` and connects successfully.
#[tokio::test]
async fn mcp_server_auth_bearer_connects_with_valid_token(
) -> Result<(), Box<dyn std::error::Error>> {
    let token = "test-secret-token-12345";
    let (mcp_url, requests) = start_protected_mcp_server(token).await;

    let config_path = write_mcp_config(&[json!({
        "name": "protected",
        "transport": "http",
        "url": mcp_url,
        "auth": {
            "type": "bearer",
            "token": token
        }
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;

    // The downstream "protected_tool" should appear as a stub
    assert!(
        tools.iter().any(|t| t.contains("protected_tool")),
        "Expected protected_tool stub in tool list, got: {:?}",
        tools
    );

    // Verify the mock received requests with the correct Authorization header
    let reqs = requests.lock().await;
    assert!(
        !reqs.is_empty(),
        "Expected at least one request to the protected server"
    );
    assert!(
        reqs.iter()
            .all(|r| r == &format!("Bearer {}", token)),
        "All requests should have correct bearer token, got: {:?}",
        *reqs
    );

    server.stop().await;
    Ok(())
}

/// Scenario 1b: bearer auth with WRONG token should fail to connect.
#[tokio::test]
async fn mcp_server_auth_bearer_fails_with_wrong_token() -> Result<(), Box<dyn std::error::Error>>
{
    let (mcp_url, _requests) = start_protected_mcp_server("correct-token").await;

    let config_path = write_mcp_config(&[json!({
        "name": "protected",
        "transport": "http",
        "url": mcp_url,
        "auth": {
            "type": "bearer",
            "token": "wrong-token"
        }
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;

    // The downstream tool should NOT appear (connection failed)
    assert!(
        !tools.iter().any(|t| t.contains("protected_tool")),
        "Should NOT have protected_tool with wrong token, got: {:?}",
        tools
    );

    server.stop().await;
    Ok(())
}

// ── Test: Client Credentials auth ───────────────────────────────────────

/// Scenario 2: downstream MCP server protected by OAuth, mcp-v8 uses
/// client_credentials grant to acquire a token from the token endpoint.
#[tokio::test]
async fn mcp_server_auth_client_credentials_acquires_token(
) -> Result<(), Box<dyn std::error::Error>> {
    let client_id = "test-client";
    let client_secret = "test-secret";
    let issued_token = "oauth-access-token-xyz";

    // Start the token server
    let (token_url, token_requests) =
        start_token_server(client_id, client_secret, issued_token).await;

    // Start the protected MCP server that expects the issued token
    let (mcp_url, mcp_requests) = start_protected_mcp_server(issued_token).await;

    let config_path = write_mcp_config(&[json!({
        "name": "oauth_protected",
        "transport": "http",
        "url": mcp_url,
        "auth": {
            "type": "client_credentials",
            "token_url": token_url,
            "client_id": client_id,
            "client_secret": client_secret,
            "scope": "mcp:read"
        }
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;

    // Verify the token was acquired and the connection succeeded
    assert!(
        tools.iter().any(|t| t.contains("protected_tool")),
        "Expected protected_tool stub after OAuth auth, got: {:?}",
        tools
    );

    // Verify the token server received a client_credentials request
    let treqs = token_requests.lock().await;
    assert!(
        !treqs.is_empty(),
        "Token server should have received a request"
    );
    assert_eq!(
        treqs[0].get("grant_type").map(|s| s.as_str()),
        Some("client_credentials")
    );
    assert_eq!(
        treqs[0].get("client_id").map(|s| s.as_str()),
        Some(client_id)
    );

    // Verify the MCP server received the correct token
    let mreqs = mcp_requests.lock().await;
    assert!(
        mreqs
            .iter()
            .any(|r| r == &format!("Bearer {}", issued_token)),
        "MCP server should have received the OAuth token, got: {:?}",
        *mreqs
    );

    server.stop().await;
    Ok(())
}

/// Scenario 2b: client_credentials with wrong credentials should fail.
#[tokio::test]
async fn mcp_server_auth_client_credentials_fails_with_wrong_secret(
) -> Result<(), Box<dyn std::error::Error>> {
    let (token_url, _) = start_token_server("correct-id", "correct-secret", "token").await;
    let (mcp_url, _) = start_protected_mcp_server("token").await;

    let config_path = write_mcp_config(&[json!({
        "name": "oauth_protected",
        "transport": "http",
        "url": mcp_url,
        "auth": {
            "type": "client_credentials",
            "token_url": token_url,
            "client_id": "correct-id",
            "client_secret": "WRONG-secret"
        }
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;

    // Connection should have failed — no protected tool visible
    assert!(
        !tools.iter().any(|t| t.contains("protected_tool")),
        "Should NOT connect with wrong credentials, got: {:?}",
        tools
    );

    server.stop().await;
    Ok(())
}

// ── Test: OAuth Discovery auth ──────────────────────────────────────────

/// Scenario 3: full MCP OAuth discovery flow.
/// mcp-v8 discovers the authorization server from the MCP server's
/// Protected Resource Metadata and performs client_credentials exchange.
#[tokio::test]
async fn mcp_server_auth_oauth_discovery_full_flow() -> Result<(), Box<dyn std::error::Error>> {
    let client_id = "discovery-client";
    let client_secret = "discovery-secret";
    let issued_token = "discovered-token-abc";

    // Start the protected MCP server
    let (mcp_url, mcp_requests) = start_protected_mcp_server(issued_token).await;

    // Start the discovery/AS server (serves resource metadata, AS metadata, and token endpoint)
    let _discovery_url =
        start_discovery_server(client_id, client_secret, issued_token, &mcp_url).await;

    let config_path = write_mcp_config(&[json!({
        "name": "discovered",
        "transport": "http",
        "url": mcp_url,
        "auth": {
            "type": "oauth_discovery",
            "client_id": client_id,
            "client_secret": client_secret,
            "scope": ["mcp:read"]
        }
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;

    // If the discovery flow worked, the protected tool should be visible
    assert!(
        tools.iter().any(|t| t.contains("protected_tool")),
        "Expected protected_tool after OAuth discovery, got: {:?}",
        tools
    );

    // Verify the MCP server received the discovered token
    let mreqs = mcp_requests.lock().await;
    assert!(
        mreqs
            .iter()
            .any(|r| r == &format!("Bearer {}", issued_token)),
        "MCP server should have received the discovered OAuth token, got: {:?}",
        *mreqs
    );

    server.stop().await;
    Ok(())
}

// ── Test: No auth (baseline) ────────────────────────────────────────────

/// Baseline: connecting to an unprotected MCP server via --mcp-config
/// without auth should work fine (no auth header sent).
#[tokio::test]
async fn mcp_server_no_auth_connects_to_unprotected_server(
) -> Result<(), Box<dyn std::error::Error>> {
    let server_bin = env!("CARGO_BIN_EXE_server");
    let upstream_heap = common::create_temp_heap_dir() + "-upstream-noauth";
    std::fs::create_dir_all(&upstream_heap).ok();

    // Use a real mcp-v8 as the downstream server (stdio, no auth needed)
    let config_path = write_mcp_config(&[json!({
        "name": "noauth",
        "transport": "stdio",
        "command": server_bin,
        "args": ["--heap-store", "dir", "--heap-dir", upstream_heap]
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;

    // Should have native tools + upstream stubs
    assert!(
        tools.iter().any(|t| t.contains("noauth__run_js")),
        "Expected noauth__run_js stub, got: {:?}",
        tools
    );

    server.stop().await;
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}

// ── Test: SSE transport with auth ───────────────────────────────────────

/// Same as bearer but with `"transport": "sse"` to verify both HTTP
/// transport variants handle auth the same way.
#[tokio::test]
async fn mcp_server_auth_bearer_works_with_sse_transport(
) -> Result<(), Box<dyn std::error::Error>> {
    let token = "sse-bearer-token";
    let (mcp_url, requests) = start_protected_mcp_server(token).await;

    let config_path = write_mcp_config(&[json!({
        "name": "sse_protected",
        "transport": "sse",
        "url": mcp_url,
        "auth": {
            "type": "bearer",
            "token": token
        }
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;

    // Should connect successfully with SSE transport + bearer auth
    assert!(
        tools.iter().any(|t| t.contains("protected_tool")),
        "Expected protected_tool via SSE + bearer, got: {:?}",
        tools
    );

    let reqs = requests.lock().await;
    assert!(
        reqs.iter()
            .all(|r| r == &format!("Bearer {}", token)),
        "SSE requests should have correct bearer token"
    );

    server.stop().await;
    Ok(())
}
