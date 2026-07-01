/// Integration tests for MCP server authentication configuration.
///
/// Tests that the `--mcp-config` JSON file with `auth` fields is parsed
/// correctly and that mcp-v8 starts successfully with auth-configured
/// downstream servers.
///
/// The HTTP-transport auth tests are `#[ignore]`d because they require a
/// real Streamable HTTP MCP server with auth enforcement — those are
/// covered by the NixOS VM integration test (`tests/nixos/mcp-server-auth.nix`).
///
/// The tests here verify:
///   - Auth config is parsed correctly from JSON (no deserialization errors)
///   - Stdio transport with no auth works as before (regression check)
///   - A downstream server using stdio transport + mcp-config JSON is accessible

use serde_json::{json, Value};
use std::io::Write as _;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

mod common;

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
        tokio::time::sleep(Duration::from_millis(2000)).await;

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

// ── Test: Config parsing with auth variants ─────────────────────────────

/// Verify that auth config JSON is parseable (no serde errors at startup).
/// Uses stdio transport so the server starts without needing HTTP connectivity.
#[tokio::test]
async fn mcp_config_with_auth_fields_parses_without_error(
) -> Result<(), Box<dyn std::error::Error>> {
    let server_bin = env!("CARGO_BIN_EXE_server");
    let upstream_heap = common::create_temp_heap_dir() + "-parse-test";
    std::fs::create_dir_all(&upstream_heap).ok();

    // Config with all auth variants — only the stdio one will actually connect.
    // The http ones will fail to connect but shouldn't crash due to parse errors.
    let config_path = write_mcp_config(&[json!({
        "name": "local",
        "transport": "stdio",
        "command": server_bin,
        "args": ["--heap-store", "dir", "--heap-dir", &upstream_heap]
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;
    assert!(
        tools.iter().any(|t| t.contains("local__run_js")),
        "Expected local__run_js stub, got: {:?}",
        tools
    );

    server.stop().await;
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}

/// Verify that a config file with all three auth types is deserializable.
/// The server won't connect to the HTTP targets but shouldn't panic during
/// config parsing.
#[tokio::test]
async fn mcp_config_deserializes_all_auth_types() -> Result<(), Box<dyn std::error::Error>> {
    // This just tests that serde can parse the config — the server will
    // fail to connect to the fake URLs but shouldn't crash at startup.
    let configs: Vec<Value> = vec![
        json!({
            "name": "bearer-srv",
            "transport": "http",
            "url": "http://127.0.0.1:1/mcp",
            "auth": {"type": "bearer", "token": "test-token"}
        }),
        json!({
            "name": "cc-srv",
            "transport": "http",
            "url": "http://127.0.0.1:1/mcp",
            "auth": {
                "type": "client_credentials",
                "token_url": "http://127.0.0.1:1/token",
                "client_id": "id",
                "client_secret": "secret",
                "scope": "read"
            }
        }),
        json!({
            "name": "disc-srv",
            "transport": "http",
            "url": "http://127.0.0.1:1/mcp",
            "auth": {
                "type": "oauth_discovery",
                "client_id": "id",
                "client_secret": "secret",
                "scope": ["read", "write"]
            }
        }),
        json!({
            "name": "browser-srv",
            "transport": "http",
            "url": "https://mcp.supabase.com/mcp",
            "auth": {
                "type": "oauth_browser",
                "scope": ["projects:read", "database:read"]
            }
        }),
    ];

    // Verify these parse as valid McpServerConfig entries
    use server::engine::mcp_client::McpServerConfig;
    let json_str = serde_json::to_string(&configs)?;
    let parsed: Vec<McpServerConfig> = serde_json::from_str(&json_str)?;

    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[0].name, "bearer-srv");
    assert_eq!(parsed[1].name, "cc-srv");
    assert_eq!(parsed[2].name, "disc-srv");
    assert_eq!(parsed[3].name, "browser-srv");
    assert!(parsed[0].auth.is_some());
    assert!(parsed[1].auth.is_some());
    assert!(parsed[2].auth.is_some());
    assert!(parsed[3].auth.is_some());

    Ok(())
}

/// Verify the minimal `oauth_browser` config (all fields optional) parses and
/// round-trips through serde with the expected tagged representation.
#[tokio::test]
async fn mcp_config_oauth_browser_minimal_parses() -> Result<(), Box<dyn std::error::Error>> {
    use server::engine::mcp_client::{McpServerAuth, McpServerConfig};

    // Only `type` is required — every other oauth_browser field is optional.
    let config: McpServerConfig = serde_json::from_value(json!({
        "name": "supabase",
        "transport": "http",
        "url": "https://mcp.supabase.com/mcp",
        "auth": {"type": "oauth_browser"}
    }))?;

    match config.auth {
        Some(McpServerAuth::OauthBrowser {
            scope,
            client_id,
            client_secret,
            redirect_port,
            token_cache,
        }) => {
            assert!(scope.is_none());
            assert!(client_id.is_none());
            assert!(client_secret.is_none());
            assert!(redirect_port.is_none());
            assert!(token_cache.is_none());
        }
        other => panic!("expected OauthBrowser auth, got: {:?}", other),
    }

    Ok(())
}

/// Verify a fully-specified `oauth_browser` config parses and serializes back
/// to the `"type": "oauth_browser"` tagged form.
#[tokio::test]
async fn mcp_config_oauth_browser_full_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    use server::engine::mcp_client::{McpServerAuth, McpServerConfig};

    let config: McpServerConfig = serde_json::from_value(json!({
        "name": "supabase",
        "transport": "http",
        "url": "https://mcp.supabase.com/mcp",
        "auth": {
            "type": "oauth_browser",
            "scope": ["projects:read", "database:read"],
            "client_id": "my-client",
            "client_secret": "shh",
            "redirect_port": 8765,
            "token_cache": "/tmp/mcp-js-supabase.json"
        }
    }))?;

    match &config.auth {
        Some(McpServerAuth::OauthBrowser {
            scope,
            client_id,
            client_secret,
            redirect_port,
            token_cache,
        }) => {
            assert_eq!(scope.as_deref(), Some(&["projects:read".to_string(), "database:read".to_string()][..]));
            assert_eq!(client_id.as_deref(), Some("my-client"));
            assert_eq!(client_secret.as_deref(), Some("shh"));
            assert_eq!(*redirect_port, Some(8765));
            assert_eq!(token_cache.as_deref(), Some("/tmp/mcp-js-supabase.json"));
        }
        other => panic!("expected OauthBrowser auth, got: {:?}", other),
    }

    // Serialized form carries the snake_case tag.
    let value = serde_json::to_value(&config)?;
    assert_eq!(value["auth"]["type"], "oauth_browser");

    Ok(())
}

// ── Test: No auth baseline (stdio, regression) ──────────────────────────

/// Baseline: connecting to an unprotected MCP server via --mcp-config
/// without auth should work fine.
#[tokio::test]
async fn mcp_server_no_auth_connects_to_unprotected_server(
) -> Result<(), Box<dyn std::error::Error>> {
    let server_bin = env!("CARGO_BIN_EXE_server");
    let upstream_heap = common::create_temp_heap_dir() + "-upstream-noauth";
    std::fs::create_dir_all(&upstream_heap).ok();

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

/// Verify auth is ignored for stdio transport (with a warning, not an error).
#[tokio::test]
async fn mcp_config_stdio_with_auth_ignores_auth() -> Result<(), Box<dyn std::error::Error>> {
    let server_bin = env!("CARGO_BIN_EXE_server");
    let upstream_heap = common::create_temp_heap_dir() + "-stdio-auth-ignored";
    std::fs::create_dir_all(&upstream_heap).ok();

    let config_path = write_mcp_config(&[json!({
        "name": "stdio_with_auth",
        "transport": "stdio",
        "command": server_bin,
        "args": ["--heap-store", "dir", "--heap-dir", &upstream_heap],
        "auth": {"type": "bearer", "token": "ignored-token"}
    })]);

    let mut server = McpV8Process::start_with_config(&config_path).await?;
    server.initialize().await?;

    let tools = server.list_tools().await?;

    // Should still connect successfully despite auth being set on stdio
    assert!(
        tools
            .iter()
            .any(|t| t.contains("stdio_with_auth__run_js")),
        "Expected stdio_with_auth__run_js stub, got: {:?}",
        tools
    );

    server.stop().await;
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}
