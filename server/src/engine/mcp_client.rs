//! MCP client manager for programmatic tool calling from JavaScript.
//!
//! Connects to external MCP servers at startup (via stdio, SSE, or HTTP
//! transports) and exposes their tools to JS code through a `globalThis.mcp`
//! object. Follows the same deno_core op pattern as `fetch.rs`.
//!
//! JS API:
//! ```js
//! mcp.servers                              // string[] — connected server names
//! mcp.listTools("server")                  // [{server, name, description, inputSchema}, ...]
//! mcp.listTools()                          // all tools from all servers
//! await mcp.callTool("server", "tool", {}) // {content: [...], isError: false} — throws McpToolError on error
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};

use rmcp::model::{CallToolRequestParams, CallToolResult, Content, Tool};
use rmcp::service::Peer;
use rmcp::RoleClient;

// ── Configuration ────────────────────────────────────────────────────────

/// Authentication configuration for HTTP-based MCP server connections.
///
/// Only available via `--mcp-config` JSON file (too complex for CLI flags).
/// Ignored for stdio transports.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServerAuth {
    /// Static bearer token — sent as `Authorization: Bearer <token>` on every request.
    Bearer { token: String },
    /// OAuth 2.0 Client Credentials grant — acquires and auto-refreshes tokens
    /// using the existing `OAuthClientCredentialsTokenSource` infrastructure.
    /// Requires knowing the token endpoint URL upfront.
    ClientCredentials {
        token_url: String,
        client_id: String,
        client_secret: String,
        #[serde(default)]
        scope: Option<String>,
    },
    /// Full MCP OAuth discovery per the 2025-11-25 spec (RFC 9728 + RFC 8414).
    ///
    /// Flow: makes an unauthenticated request to the MCP server URL → receives 401
    /// with `WWW-Authenticate` header containing `resource_metadata` URL (or falls
    /// back to well-known URI) → fetches Protected Resource Metadata → discovers
    /// Authorization Server → fetches AS metadata → performs client_credentials
    /// grant → uses the resulting token.
    ///
    /// This is the spec-compliant way to connect to OAuth-protected MCP servers
    /// without needing to know the token endpoint in advance.
    OauthDiscovery {
        client_id: String,
        client_secret: String,
        #[serde(default)]
        scope: Option<Vec<String>>,
        /// Optional resource indicator (RFC 8707). If omitted, uses the server URL.
        #[serde(default)]
        resource: Option<String>,
    },
}

/// Transport configuration for a single MCP server.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum McpServerTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Sse {
        url: String,
    },
    Http {
        url: String,
    },
}

/// Configuration for a single named MCP server.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(flatten)]
    pub transport: McpServerTransport,
    /// Optional authentication for HTTP-based transports (SSE, HTTP).
    /// Ignored for stdio transports. Only available via `--mcp-config` JSON
    /// file (too complex for CLI flags).
    #[serde(default)]
    pub auth: Option<McpServerAuth>,
}

// ── Tool metadata for JS ─────────────────────────────────────────────────

/// Serializable tool info returned to JavaScript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub server: String,
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

impl ToolInfo {
    fn from_tool(server_name: &str, tool: &Tool) -> Self {
        Self {
            server: server_name.to_string(),
            name: tool.name.to_string(),
            description: tool.description.as_ref().map(|d| d.to_string()),
            input_schema: serde_json::Value::Object(tool.input_schema.as_ref().clone()),
        }
    }
}

// ── Connected server ─────────────────────────────────────────────────────

/// The result of a single `connect_one` handshake: a live peer, its tool list,
/// and the task that holds the underlying RunningService alive.
struct ConnectedMcpServer {
    peer: Peer<RoleClient>,
    tools: Vec<Tool>,
    /// Holds the RunningService alive. Aborting this drops the connection.
    _keep_alive: tokio::task::AbortHandle,
}

/// The live connection for one downstream server. Swapped wholesale by
/// `reconnect` when the server goes unhealthy (e.g. the downstream restarted),
/// so an established connection can self-heal without restarting MCPJS.
struct LiveConn {
    peer: Peer<RoleClient>,
    /// Holds the RunningService alive; aborted when the connection is replaced.
    keep_alive: tokio::task::AbortHandle,
}

/// A named downstream server: the config needed to reconnect, plus the current
/// live connection behind a lock so a background liveness task (or a failed
/// `call_tool`) can replace it in place.
struct ServerConn {
    config: McpServerConfig,
    live: RwLock<LiveConn>,
}

/// How often the background liveness task probes each downstream connection.
const LIVENESS_INTERVAL: Duration = Duration::from_secs(20);

/// A cheap round-trip that fails if the transport is disconnected. Used both as
/// the periodic liveness probe and to tell a dead connection apart from a
/// genuine tool error inside `call_tool`.
async fn is_healthy(peer: &Peer<RoleClient>) -> bool {
    peer.list_all_tools().await.is_ok()
}

/// Re-run the handshake for a server and swap in the fresh peer, aborting the
/// stale RunningService. Tools are intentionally left as first-connect values.
async fn reconnect(server: &ServerConn) -> Result<(), String> {
    let fresh = connect_one(&server.config).await?;
    let mut live = server.live.write().await;
    live.keep_alive.abort();
    live.peer = fresh.peer;
    live.keep_alive = fresh._keep_alive;
    Ok(())
}

/// Spawn a detached task that periodically health-checks one server and
/// reconnects it when the probe fails.
fn spawn_liveness(server: Arc<ServerConn>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(LIVENESS_INTERVAL).await;
            let peer = { server.live.read().await.peer.clone() };
            if is_healthy(&peer).await {
                continue;
            }
            tracing::warn!(
                "MCP server '{}' failed liveness check; reconnecting...",
                server.config.name
            );
            match reconnect(&server).await {
                Ok(()) => tracing::info!("MCP server '{}' reconnected", server.config.name),
                Err(e) => tracing::warn!(
                    "MCP server '{}' reconnect attempt failed: {} (will retry)",
                    server.config.name,
                    e
                ),
            }
        }
    });
}

// ── McpClientManager ─────────────────────────────────────────────────────

/// Configuration for the auto-generated MCP tool stubs that MCPJS exposes
/// to its own clients on behalf of upstream servers. The default prefix
/// `runjs__` makes it obvious to a calling agent that these tools execute
/// indirectly through the JavaScript runtime (`run_js` + `mcp.callTool(...)`),
/// rather than through MCPJS's normal tool dispatcher.
#[derive(Debug, Clone)]
pub struct StubConfig {
    pub prefix: String,
    pub enabled: bool,
}

pub const DEFAULT_STUB_PREFIX: &str = "runjs__";

impl Default for StubConfig {
    fn default() -> Self {
        Self {
            prefix: DEFAULT_STUB_PREFIX.to_string(),
            enabled: true,
        }
    }
}

/// Manages connections to multiple MCP servers. Thread-safe and cloneable
/// for sharing across V8 executions (stored in deno_core OpState).
///
/// `tools_by_server` is the source of truth for tool listings (and the basis
/// for the auto-generated MCP tool stubs that MCPJS exposes to its own
/// clients). It is populated alongside `servers` during `connect()`, and can
/// be populated independently for tests via `from_tools_for_test()`.
#[derive(Clone)]
pub struct McpClientManager {
    tools_by_server: Arc<HashMap<String, Vec<Tool>>>,
    servers: Arc<HashMap<String, Arc<ServerConn>>>,
    stub_config: StubConfig,
    /// The runtime that owns the server connections (captured at `connect()`,
    /// i.e. the multi-thread server runtime). `call_tool` bridges onto it: the
    /// peers' transport I/O lives here, so awaiting a call from the isolate's
    /// per-execution current-thread runtime would otherwise stall. Mirrors
    /// `S3HeapStorage`'s `runtime` handle. `None` for test-only constructors.
    runtime: Option<tokio::runtime::Handle>,
}

impl McpClientManager {
    /// Connect to all configured MCP servers. Fails fast if any connection fails.
    pub async fn connect(configs: Vec<McpServerConfig>) -> Result<Self, String> {
        let mut servers = HashMap::new();
        let mut tools_by_server: HashMap<String, Vec<Tool>> = HashMap::new();

        for config in configs {
            tracing::info!("Connecting to MCP server '{}'...", config.name);
            let connected = connect_one(&config).await?;

            tracing::info!(
                "MCP server '{}': {} tool(s) available",
                config.name,
                connected.tools.len()
            );
            for tool in &connected.tools {
                tracing::info!("  - {}.{}", config.name, tool.name);
            }

            if servers.contains_key(&config.name) {
                return Err(format!("Duplicate MCP server name: '{}'", config.name));
            }
            let name = config.name.clone();
            tools_by_server.insert(name.clone(), connected.tools.clone());
            let server = Arc::new(ServerConn {
                config,
                live: RwLock::new(LiveConn {
                    peer: connected.peer,
                    keep_alive: connected._keep_alive,
                }),
            });
            // Self-heal: probe this connection periodically and reconnect if the
            // downstream server restarts (the long-lived handshake otherwise
            // stays dead until MCPJS is restarted).
            spawn_liveness(server.clone());
            servers.insert(name, server);
        }

        Ok(Self {
            tools_by_server: Arc::new(tools_by_server),
            servers: Arc::new(servers),
            stub_config: StubConfig::default(),
            runtime: Some(tokio::runtime::Handle::current()),
        })
    }

    /// Override the stub-tool exposure config. Builder-style; intended to be
    /// chained right after `connect()`.
    pub fn with_stub_config(mut self, config: StubConfig) -> Self {
        self.stub_config = config;
        self
    }

    pub fn stub_config(&self) -> &StubConfig {
        &self.stub_config
    }

    /// Test-only constructor: build a catalog-only manager (no live peers).
    /// `call_tool` will fail because no peers exist, but `list_tools`,
    /// `stub_tools`, and `stub_call_response` work as if the servers were
    /// connected. Reserved for unit tests.
    #[cfg(test)]
    pub fn from_tools_for_test(tools_by_server: HashMap<String, Vec<Tool>>) -> Self {
        Self {
            tools_by_server: Arc::new(tools_by_server),
            servers: Arc::new(HashMap::new()),
            stub_config: StubConfig::default(),
            runtime: None,
        }
    }

    /// List connected server names.
    pub fn server_names(&self) -> Vec<String> {
        self.tools_by_server.keys().cloned().collect()
    }

    /// List tools, optionally filtered by server name.
    pub fn list_tools(&self, server_name: Option<&str>) -> Result<Vec<ToolInfo>, String> {
        match server_name {
            Some(name) => {
                let tools = self.tools_by_server.get(name).ok_or_else(|| {
                    format!(
                        "MCP server '{}' not found. Available: {:?}",
                        name,
                        self.server_names()
                    )
                })?;
                Ok(tools.iter().map(|t| ToolInfo::from_tool(name, t)).collect())
            }
            None => {
                let mut all = Vec::new();
                for (name, tools) in self.tools_by_server.as_ref() {
                    for tool in tools {
                        all.push(ToolInfo::from_tool(name, tool));
                    }
                }
                Ok(all)
            }
        }
    }

    /// Generate stub `Tool` definitions for every upstream tool. These are
    /// intended to be served by MCPJS's own MCP server so that an external
    /// agent can discover the tool via MCP tool-list/search but invoke it
    /// through the JavaScript runtime (`run_js` → `mcp.callTool(...)`).
    /// Returns an empty vec when stub exposure is disabled in the config.
    pub fn stub_tools(&self) -> Vec<Tool> {
        if !self.stub_config.enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (server, tools) in self.tools_by_server.as_ref() {
            for tool in tools {
                out.push(make_stub_tool(&self.stub_config.prefix, server, tool));
            }
        }
        out
    }

    /// If `name` is a stub for a known upstream tool, build the instructional
    /// `CallToolResult` (telling the caller to invoke the tool via `run_js`).
    /// Returns `None` if stubs are disabled or if `name` does not match any
    /// known stub — callers should fall through to their normal tool
    /// dispatcher in that case.
    pub fn stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Option<CallToolResult> {
        if !self.stub_config.enabled {
            return None;
        }
        let (server, tool) = parse_stub_tool_name(&self.stub_config.prefix, name)?;
        let tools = self.tools_by_server.get(&server)?;
        if !tools.iter().any(|t| t.name.as_ref() == tool) {
            return None;
        }
        Some(CallToolResult::success(vec![Content::text(
            stub_call_instructions(&server, &tool, arguments),
        )]))
    }

    /// Call a tool on a specific server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<serde_json::Value, String> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| {
                format!(
                    "MCP server '{}' not found. Available: {:?}",
                    server_name,
                    self.server_names()
                )
            })?
            .clone();
        let server_name = server_name.to_string();
        let tool_name = tool_name.to_string();

        // The downstream peer's transport I/O lives on the runtime that owns the
        // connection (captured at `connect()`). run_js ops run on a per-execution
        // current-thread runtime, from which awaiting the peer would stall, so run
        // the whole call on the connection's runtime and await the JoinHandle
        // (safe to poll from any runtime) — mirrors S3HeapStorage::*_blocking.
        let call = async move {
            let make_req = || {
                let mut req = CallToolRequestParams::default();
                req.name = tool_name.clone().into();
                req.arguments = arguments.clone();
                req
            };

            let peer = { server.live.read().await.peer.clone() };
            let result = match peer.call_tool(make_req()).await {
                Ok(r) => r,
                Err(e) => {
                    // A call can fail because the tool errored OR because the
                    // downstream connection died (e.g. the server restarted).
                    // Probe to tell them apart: if the connection is still
                    // healthy the error is genuine; otherwise reconnect and retry
                    // once so a restarted downstream heals transparently.
                    if is_healthy(&peer).await {
                        return Err(format!("mcp.callTool({}.{}): {}", server_name, tool_name, e));
                    }
                    tracing::warn!(
                        "MCP server '{}' looks disconnected ({}); reconnecting and retrying",
                        server_name,
                        e
                    );
                    reconnect(&server).await.map_err(|re| {
                        format!(
                            "mcp.callTool({}.{}): reconnect failed: {}",
                            server_name, tool_name, re
                        )
                    })?;
                    let peer = { server.live.read().await.peer.clone() };
                    peer.call_tool(make_req()).await.map_err(|e| {
                        format!(
                            "mcp.callTool({}.{}): {} (after reconnect)",
                            server_name, tool_name, e
                        )
                    })?
                }
            };

            // Serialize content to JSON for JS consumption.
            let content_json: Vec<serde_json::Value> = result
                .content
                .iter()
                .map(|c| {
                    serde_json::to_value(c)
                        .unwrap_or(serde_json::json!({"error": "serialization failed"}))
                })
                .collect();

            Ok(serde_json::json!({
                "content": content_json,
                "isError": result.is_error.unwrap_or(false),
            }))
        };

        match &self.runtime {
            Some(rt) => rt
                .spawn(call)
                .await
                .map_err(|e| format!("mcp.callTool: task join error: {e}"))?,
            None => call.await,
        }
    }
}

// ── Tool stubs ──────────────────────────────────────────────────────────
//
// Stub names follow `<prefix><server>__<tool>`. The default prefix is
// `runjs__`, signalling that the tool is dispatched through the JS runtime
// rather than through MCPJS's normal tool dispatcher. The stub Tool's
// input schema is identical to the upstream tool's schema, so the agent
// can plan a `run_js` call with correct arguments.

const STUB_SEPARATOR: &str = "__";

/// Build the stub tool name for `server.tool` under the given `prefix`.
pub fn stub_tool_name(prefix: &str, server: &str, tool: &str) -> String {
    format!("{}{}{}{}", prefix, server, STUB_SEPARATOR, tool)
}

/// Inverse of `stub_tool_name`. Returns `(server, tool)` or `None` if `name`
/// does not start with `prefix` or does not contain a `__` separator after
/// the prefix. Splits on the **first** `__` after the prefix so server
/// names without `__` round-trip exactly; tool names containing `__` are
/// preserved. An empty `prefix` is treated as "no stub recognition" and
/// always returns `None` — pass a non-empty prefix or disable stubs via
/// `StubConfig::enabled = false`.
pub fn parse_stub_tool_name(prefix: &str, name: &str) -> Option<(String, String)> {
    if prefix.is_empty() {
        return None;
    }
    let rest = name.strip_prefix(prefix)?;
    let idx = rest.find(STUB_SEPARATOR)?;
    let server = &rest[..idx];
    let tool = &rest[idx + STUB_SEPARATOR.len()..];
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server.to_string(), tool.to_string()))
}

/// Build a stub `Tool` mirroring an upstream tool's schema. The description
/// is rewritten to make it clear the tool is invoked via `run_js`.
pub fn make_stub_tool(prefix: &str, server: &str, tool: &Tool) -> Tool {
    let stub_name = stub_tool_name(prefix, server, &tool.name);
    let original_desc = tool.description.as_deref().unwrap_or("");
    let header = format!(
        "[stub for upstream MCP tool {server}.{tool} — invoke via run_js then \
         `await mcp.callTool({server:?}, {tool:?}, args)`. Calling this tool \
         directly only returns instructions; it does not execute.]",
        server = server,
        tool = tool.name,
    );
    let new_desc = if original_desc.is_empty() {
        header
    } else {
        format!("{}\n\n{}", header, original_desc)
    };
    // Drop annotations from stubs: upstream servers (e.g. GitHub MCP,
    // Linear) may return `null` for optional boolean hint fields
    // (readOnlyHint, destructiveHint, etc.). The rmcp ToolAnnotations
    // struct serializes Option::None as JSON `null` (its fields lack
    // skip_serializing_if), which violates the MCP spec and causes
    // Claude Code SDK's Zod validator to reject the entire tools/list
    // response.
    //
    // Since stubs are discovery mechanisms (they return instructions, not
    // results), upstream annotations about behavior are misleading anyway.
    // Setting annotations to None omits the field entirely from the JSON.
    Tool::new(stub_name, new_desc, tool.input_schema.clone())
}

/// Render the instructional text returned when an external client calls a
/// stub tool. The caller is expected to re-invoke the tool from JavaScript.
pub fn stub_call_instructions(
    server: &str,
    tool: &str,
    arguments: Option<&serde_json::Map<String, serde_json::Value>>,
) -> String {
    let args_value = arguments
        .map(|m| serde_json::Value::Object(m.clone()))
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let pretty = serde_json::to_string_pretty(&args_value).unwrap_or_else(|_| "{}".into());
    format!(
        "This tool is a stub. Execute it from JavaScript via the `run_js` tool, e.g.:\n\
         \n\
         const result = await mcp.callTool({server:?}, {tool:?}, {pretty});\n\
         console.log(JSON.stringify(result));\n",
        server = server,
        tool = tool,
        pretty = pretty,
    )
}

// ── Connection logic ─────────────────────────────────────────────────────

/// Resolve the auth configuration into an `Authorization` header value (e.g.
/// `"Bearer <token>"`) that can be passed to the Streamable HTTP transport.
async fn resolve_auth_header(
    server_name: &str,
    server_url: Option<&str>,
    auth: &Option<McpServerAuth>,
) -> Result<Option<String>, String> {
    match auth {
        None => Ok(None),
        Some(McpServerAuth::Bearer { token }) => Ok(Some(format!("Bearer {}", token))),
        Some(McpServerAuth::ClientCredentials {
            token_url,
            client_id,
            client_secret,
            scope,
        }) => {
            use super::fetch_auth::{OAuthClientCredentialsTokenSource, OAuthTokenSourceConfig};

            let source = OAuthClientCredentialsTokenSource::new(
                reqwest::Client::new(),
                OAuthTokenSourceConfig {
                    header: "Authorization".to_string(),
                    token_url: token_url.clone(),
                    client_id: client_id.clone(),
                    client_secret: client_secret.clone(),
                    scope: scope.clone(),
                    refresh_buffer_secs: 30,
                },
            );
            let header_value = source.authorization_header_value().await.map_err(|e| {
                format!(
                    "OAuth token acquisition for '{}' failed: {}",
                    server_name, e
                )
            })?;
            Ok(Some(header_value))
        }
        Some(McpServerAuth::OauthDiscovery {
            client_id,
            client_secret,
            scope,
            resource,
        }) => {
            let url = server_url.ok_or_else(|| {
                format!(
                    "MCP server '{}': oauth_discovery auth requires an HTTP-based transport URL",
                    server_name
                )
            })?;

            // Implements the MCP authorization spec (2025-11-25) using reqwest:
            //   1. Probe the MCP server URL for Protected Resource Metadata (RFC 9728)
            //   2. Discover Authorization Server Metadata (RFC 8414)
            //   3. Exchange client_credentials at the token endpoint

            let http = reqwest::Client::new();

            // ── Step 1: Discover Protected Resource Metadata ──────────────
            // Try /.well-known/oauth-protected-resource relative to the server URL.
            let parsed_url = url::Url::parse(url).map_err(|e| {
                format!("MCP server '{}': invalid URL '{}': {}", server_name, url, e)
            })?;
            let base = format!(
                "{}://{}{}",
                parsed_url.scheme(),
                parsed_url.host_str().unwrap_or("localhost"),
                parsed_url
                    .port()
                    .map(|p| format!(":{}", p))
                    .unwrap_or_default()
            );

            // Try path-specific well-known first, then root
            let path = parsed_url.path().trim_end_matches('/');
            let resource_metadata_urls = if path.is_empty() || path == "/" {
                vec![format!("{}/.well-known/oauth-protected-resource", base)]
            } else {
                vec![
                    format!(
                        "{}/.well-known/oauth-protected-resource{}",
                        base, path
                    ),
                    format!("{}/.well-known/oauth-protected-resource", base),
                ]
            };

            let mut resource_metadata: Option<serde_json::Value> = None;
            for rm_url in &resource_metadata_urls {
                match http.get(rm_url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            resource_metadata = Some(json);
                            break;
                        }
                    }
                    _ => continue,
                }
            }

            let rm = resource_metadata.ok_or_else(|| {
                format!(
                    "MCP server '{}': could not discover Protected Resource Metadata at {:?}",
                    server_name, resource_metadata_urls
                )
            })?;

            // Extract authorization server URL
            let auth_servers = rm["authorization_servers"]
                .as_array()
                .ok_or_else(|| {
                    format!(
                        "MCP server '{}': resource metadata missing 'authorization_servers'",
                        server_name
                    )
                })?;
            let as_url = auth_servers
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!(
                        "MCP server '{}': authorization_servers array is empty",
                        server_name
                    )
                })?;

            // ── Step 2: Discover Authorization Server Metadata ────────────
            let as_parsed = url::Url::parse(as_url).map_err(|e| {
                format!(
                    "MCP server '{}': invalid AS URL '{}': {}",
                    server_name, as_url, e
                )
            })?;
            let as_base = format!(
                "{}://{}{}",
                as_parsed.scheme(),
                as_parsed.host_str().unwrap_or("localhost"),
                as_parsed
                    .port()
                    .map(|p| format!(":{}", p))
                    .unwrap_or_default()
            );
            let as_path = as_parsed.path().trim_end_matches('/');

            // Try RFC 8414 endpoints in priority order
            let as_metadata_urls = if as_path.is_empty() || as_path == "/" {
                vec![
                    format!("{}/.well-known/oauth-authorization-server", as_base),
                    format!("{}/.well-known/openid-configuration", as_base),
                ]
            } else {
                vec![
                    format!(
                        "{}/.well-known/oauth-authorization-server{}",
                        as_base, as_path
                    ),
                    format!(
                        "{}/.well-known/openid-configuration{}",
                        as_base, as_path
                    ),
                    format!("{}{}/.well-known/openid-configuration", as_base, as_path),
                ]
            };

            let mut as_metadata: Option<serde_json::Value> = None;
            for am_url in &as_metadata_urls {
                match http.get(am_url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            as_metadata = Some(json);
                            break;
                        }
                    }
                    _ => continue,
                }
            }

            let am = as_metadata.ok_or_else(|| {
                format!(
                    "MCP server '{}': could not discover AS metadata at {:?}",
                    server_name, as_metadata_urls
                )
            })?;

            let token_endpoint = am["token_endpoint"]
                .as_str()
                .ok_or_else(|| {
                    format!(
                        "MCP server '{}': AS metadata missing 'token_endpoint'",
                        server_name
                    )
                })?;

            // ── Step 3: Exchange client_credentials ───────────────────────
            let scope_str = scope
                .as_ref()
                .map(|s| s.join(" "))
                .unwrap_or_default();
            let resource_value = resource.clone().unwrap_or_else(|| url.to_string());

            let mut form = HashMap::new();
            form.insert("grant_type", "client_credentials".to_string());
            form.insert("client_id", client_id.clone());
            form.insert("client_secret", client_secret.clone());
            if !scope_str.is_empty() {
                form.insert("scope", scope_str);
            }
            form.insert("resource", resource_value);

            let token_resp = http
                .post(token_endpoint)
                .form(&form)
                .send()
                .await
                .map_err(|e| {
                    format!(
                        "MCP server '{}': token request to '{}' failed: {}",
                        server_name, token_endpoint, e
                    )
                })?;

            if !token_resp.status().is_success() {
                let status = token_resp.status();
                let body = token_resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "<no body>".to_string());
                return Err(format!(
                    "MCP server '{}': token endpoint returned {}: {}",
                    server_name, status, body
                ));
            }

            let token_json: serde_json::Value =
                token_resp.json().await.map_err(|e| {
                    format!(
                        "MCP server '{}': failed to parse token response: {}",
                        server_name, e
                    )
                })?;

            let access_token = token_json["access_token"]
                .as_str()
                .ok_or_else(|| {
                    format!(
                        "MCP server '{}': token response missing 'access_token'",
                        server_name
                    )
                })?;

            Ok(Some(format!("Bearer {}", access_token)))
        }
    }
}

async fn connect_one(config: &McpServerConfig) -> Result<ConnectedMcpServer, String> {
    use rmcp::ServiceExt;

    match &config.transport {
        McpServerTransport::Stdio { command, args, env } => {
            if config.auth.is_some() {
                tracing::warn!(
                    "MCP server '{}': auth configuration is ignored for stdio transport",
                    config.name
                );
            }

            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args);
            for (k, v) in env {
                cmd.env(k, v);
            }
            let transport = rmcp::transport::TokioChildProcess::new(cmd)
                .map_err(|e| format!("Failed to spawn '{}': {}", command, e))?;

            let service: rmcp::service::RunningService<RoleClient, ()> =
                ().serve(transport)
                    .await
                    .map_err(|e| format!("MCP client handshake with '{}' failed: {}", config.name, e))?;

            let peer = service.peer().clone();
            let tools = peer
                .list_all_tools()
                .await
                .map_err(|e| format!("Failed to list tools from '{}': {}", config.name, e))?;

            let keep_alive = tokio::spawn(async move {
                let _ = service.waiting().await;
            });

            Ok(ConnectedMcpServer {
                peer,
                tools,
                _keep_alive: keep_alive.abort_handle(),
            })
        }
        McpServerTransport::Sse { url } | McpServerTransport::Http { url } => {
            // Resolve auth header (Bearer, OAuth client_credentials, or MCP OAuth discovery).
            let auth_header = resolve_auth_header(&config.name, Some(url.as_str()), &config.auth).await?;

            // The standalone SSE client transport was removed in rmcp 1.x; the
            // Streamable HTTP client transport is its replacement and speaks to
            // the same `/mcp`-style endpoints modern MCP servers expose.
            let transport = match auth_header {
                Some(header) => {
                    use rmcp::transport::streamable_http_client::{
                        StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
                    };
                    let cfg = StreamableHttpClientTransportConfig {
                        uri: url.clone().into(),
                        auth_header: Some(header),
                        ..Default::default()
                    };
                    StreamableHttpClientWorker::new(reqwest::Client::new(), cfg)
                }
                None => rmcp::transport::StreamableHttpClientTransport::from_uri(url.clone()),
            };

            let service: rmcp::service::RunningService<RoleClient, ()> =
                ().serve(transport)
                    .await
                    .map_err(|e| format!("MCP client handshake with '{}' failed: {}", config.name, e))?;

            let peer = service.peer().clone();
            let tools = peer
                .list_all_tools()
                .await
                .map_err(|e| format!("Failed to list tools from '{}': {}", config.name, e))?;

            let keep_alive = tokio::spawn(async move {
                let _ = service.waiting().await;
            });

            Ok(ConnectedMcpServer {
                peer,
                tools,
                _keep_alive: keep_alive.abort_handle(),
            })
        }
    }
}

// ── OpState config ───────────────────────────────────────────────────────

/// Configuration stored in deno_core's OpState for the MCP ops.
#[derive(Clone)]
pub struct McpConfig {
    pub client_manager: McpClientManager,
    /// Optional OPA policy chain for gating `mcp.callTool()` calls.
    pub policy_chain: Option<std::sync::Arc<super::opa::PolicyChain>>,
}

// ── Deno ops ─────────────────────────────────────────────────────────────

/// OPA policy input for MCP tool calls.
#[derive(Serialize)]
struct McpToolPolicyInput {
    operation: &'static str,
    server: String,
    tool: String,
    arguments: serde_json::Value,
}

/// Async op: call an MCP tool. Spawned on a separate tokio task to avoid
/// RefCell re-entrancy issues (same pattern as op_fetch).
#[op2(async)]
#[string]
async fn op_mcp_call_tool(
    state: Rc<RefCell<OpState>>,
    #[string] server_name: String,
    #[string] tool_name: String,
    #[string] arguments_json: String,
) -> Result<String, JsErrorBox> {
    let (manager, policy_chain) = {
        let state = state.borrow();
        let config = state
            .try_borrow::<McpConfig>()
            .ok_or_else(|| JsErrorBox::generic("mcp: internal error — no MCP config available"))?;
        (config.client_manager.clone(), config.policy_chain.clone())
    };

    let arguments: Option<serde_json::Map<String, serde_json::Value>> =
        if arguments_json.is_empty() {
            None
        } else {
            Some(
                serde_json::from_str(&arguments_json).map_err(|e| {
                    JsErrorBox::generic(format!("mcp.callTool: invalid arguments JSON: {}", e))
                })?,
            )
        };

    // Spawn on separate tokio task (same pattern as fetch) to avoid
    // RefCell re-entrancy panic in deno_core's FuturesUnorderedDriver.
    tokio::spawn(async move {
        // Evaluate OPA policy if configured.
        if let Some(ref chain) = policy_chain {
            let policy_input = McpToolPolicyInput {
                operation: "mcp_call_tool",
                server: server_name.clone(),
                tool: tool_name.clone(),
                arguments: arguments
                    .as_ref()
                    .map(|a| serde_json::Value::Object(a.clone()))
                    .unwrap_or(serde_json::Value::Null),
            };
            let input_value = serde_json::to_value(&policy_input)
                .map_err(|e| JsErrorBox::generic(format!("mcp.callTool: failed to serialize policy input: {}", e)))?;
            let allowed = chain.evaluate(&input_value).await
                .map_err(|e| JsErrorBox::generic(format!("mcp.callTool: policy evaluation error: {}", e)))?;
            if !allowed {
                return Err(JsErrorBox::generic(format!(
                    "mcp.callTool denied by policy: {}.{} is not allowed",
                    server_name, tool_name
                )));
            }
        }

        let result = manager
            .call_tool(&server_name, &tool_name, arguments)
            .await
            .map_err(|e| JsErrorBox::generic(e))?;
        serde_json::to_string(&result)
            .map_err(|e| JsErrorBox::generic(format!("mcp.callTool: serialization error: {}", e)))
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("mcp task join error: {}", e)))?
}

/// Sync op: list available tools from cached data (no network call).
#[op2]
#[string]
fn op_mcp_list_tools(
    state: &mut OpState,
    #[string] server_name: String,
) -> Result<String, JsErrorBox> {
    let config = state
        .try_borrow::<McpConfig>()
        .ok_or_else(|| JsErrorBox::generic("mcp: internal error — no MCP config available"))?;

    let server_filter = if server_name.is_empty() {
        None
    } else {
        Some(server_name.as_str())
    };
    let tools = config
        .client_manager
        .list_tools(server_filter)
        .map_err(|e| JsErrorBox::generic(e))?;

    serde_json::to_string(&tools)
        .map_err(|e| JsErrorBox::generic(format!("mcp.listTools: serialization error: {}", e)))
}

/// Sync op: list connected server names.
#[op2]
#[string]
fn op_mcp_list_servers(state: &mut OpState) -> Result<String, JsErrorBox> {
    let config = state
        .try_borrow::<McpConfig>()
        .ok_or_else(|| JsErrorBox::generic("mcp: internal error — no MCP config available"))?;

    let servers = config.client_manager.server_names();
    serde_json::to_string(&servers)
        .map_err(|e| JsErrorBox::generic(format!("mcp.servers: serialization error: {}", e)))
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    mcp_client_ext,
    ops = [op_mcp_call_tool, op_mcp_list_tools, op_mcp_list_servers],
);

/// Create the MCP client extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    mcp_client_ext::init()
}

// ── Inject mcp JS wrapper into the global scope ─────────────────────────

/// Inject the `globalThis.mcp` JS wrapper. Must be called after the
/// runtime is created (with the mcp_client extension) but before user code runs.
pub fn inject_mcp(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<mcp-setup>", MCP_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install mcp wrapper: {}", e))?;
    Ok(())
}

/// JavaScript wrapper that provides the `globalThis.mcp` API.
const MCP_JS_WRAPPER: &str = r#"
(function() {
    /**
     * Error thrown when an MCP tool returns an error result.
     * The original result (with content and isError) is available on the `result` property.
     */
    class McpToolError extends Error {
        constructor(serverName, toolName, result) {
            var text = (result.content && result.content.length > 0 && result.content[0].text)
                ? result.content[0].text
                : 'Tool returned an error';
            super('mcp.callTool ' + serverName + '.' + toolName + ' failed: ' + text);
            this.name = 'McpToolError';
            this.result = result;
            this.serverName = serverName;
            this.toolName = toolName;
        }
    }
    globalThis.McpToolError = McpToolError;

    globalThis.mcp = {
        /**
         * Call a tool on a connected MCP server.
         * Throws McpToolError if the tool returns an error result (isError: true).
         * @param {string} serverName - Name of the MCP server
         * @param {string} toolName - Name of the tool to call
         * @param {object} [args] - Arguments to pass to the tool
         * @returns {Promise<{content: Array, isError: boolean}>}
         * @throws {McpToolError} When the tool returns isError: true
         */
        callTool: async function(serverName, toolName, args) {
            if (typeof serverName !== 'string') throw new TypeError('mcp.callTool: serverName must be a string');
            if (typeof toolName !== 'string') throw new TypeError('mcp.callTool: toolName must be a string');
            var argsJson = args ? JSON.stringify(args) : '';
            var raw = await Deno.core.ops.op_mcp_call_tool(serverName, toolName, argsJson);
            var result = JSON.parse(raw);
            if (result.isError) {
                throw new McpToolError(serverName, toolName, result);
            }
            return result;
        },

        /**
         * List available tools, optionally filtered by server name.
         * Each tool has: server, name, description, inputSchema.
         * @param {string} [serverName] - If provided, list only tools for this server
         * @returns {Array<{server: string, name: string, description: string|null, inputSchema: object}>}
         */
        listTools: function(serverName) {
            var raw = Deno.core.ops.op_mcp_list_tools(serverName || '');
            return JSON.parse(raw);
        },

        /**
         * Get the list of connected MCP server names.
         * @returns {string[]}
         */
        get servers() {
            var raw = Deno.core.ops.op_mcp_list_servers();
            return JSON.parse(raw);
        },
    };
})();
"#;

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Tool;
    use serde_json::json;
    use std::sync::Arc as StdArc;

    fn schema(props: serde_json::Value) -> StdArc<rmcp::model::JsonObject> {
        let obj = json!({"type": "object", "properties": props})
            .as_object()
            .cloned()
            .unwrap();
        StdArc::new(obj)
    }

    fn tool(name: &'static str, desc: &'static str) -> Tool {
        Tool::new(name, desc, schema(json!({"x": {"type": "number"}})))
    }

    #[test]
    fn default_stub_prefix_is_runjs() {
        // The default prefix advertises that the tool runs via the JS
        // runtime rather than dispatching through MCPJS directly.
        assert_eq!(StubConfig::default().prefix, "runjs__");
        assert_eq!(DEFAULT_STUB_PREFIX, "runjs__");
        assert!(StubConfig::default().enabled);
    }

    #[test]
    fn stub_name_round_trips() {
        let n = stub_tool_name("runjs__", "github", "create_issue");
        assert_eq!(n, "runjs__github__create_issue");
        assert_eq!(
            parse_stub_tool_name("runjs__", &n),
            Some(("github".to_string(), "create_issue".to_string()))
        );
    }

    #[test]
    fn stub_name_round_trips_with_custom_prefix() {
        let n = stub_tool_name("rj_", "srv", "do_thing");
        assert_eq!(n, "rj_srv__do_thing");
        assert_eq!(
            parse_stub_tool_name("rj_", &n),
            Some(("srv".to_string(), "do_thing".to_string()))
        );
        // Default prefix should not match a name minted with a custom prefix.
        assert_eq!(parse_stub_tool_name("runjs__", &n), None);
    }

    #[test]
    fn parse_stub_preserves_underscores_in_tool_name() {
        // Tool names with `__` should round-trip via the rest of the string.
        let n = stub_tool_name("runjs__", "srv", "do__a_thing");
        assert_eq!(n, "runjs__srv__do__a_thing");
        assert_eq!(
            parse_stub_tool_name("runjs__", &n),
            Some(("srv".to_string(), "do__a_thing".to_string()))
        );
    }

    #[test]
    fn parse_stub_rejects_non_stub_names() {
        // Built-in MCPJS tools should not be misclassified as stubs.
        assert_eq!(parse_stub_tool_name("runjs__", "run_js"), None);
        assert_eq!(parse_stub_tool_name("runjs__", "get_execution"), None);
        // Missing separator after server name.
        assert_eq!(parse_stub_tool_name("runjs__", "runjs__github"), None);
        // Empty server or tool segment.
        assert_eq!(parse_stub_tool_name("runjs__", "runjs____tool"), None);
        assert_eq!(parse_stub_tool_name("runjs__", "runjs__server__"), None);
        // Wrong prefix.
        assert_eq!(parse_stub_tool_name("runjs__", "mcp__server__tool"), None);
        // Empty prefix is treated as "no stub recognition".
        assert_eq!(parse_stub_tool_name("", "server__tool"), None);
    }

    #[test]
    fn make_stub_tool_preserves_schema_and_rewrites_description() {
        let upstream = tool("create_issue", "Create a GitHub issue.");
        let stub = make_stub_tool("runjs__", "github", &upstream);
        assert_eq!(stub.name, "runjs__github__create_issue");
        // Schema is the *same Arc* — stubs share the upstream schema.
        assert!(StdArc::ptr_eq(&stub.input_schema, &upstream.input_schema));
        // Description hints at run_js usage and includes original docs.
        let desc = stub.description.expect("description");
        assert!(desc.contains("run_js"), "description should mention run_js: {}", desc);
        assert!(desc.contains("mcp.callTool"), "description should mention mcp.callTool: {}", desc);
        assert!(desc.contains("Create a GitHub issue."));
    }

    #[test]
    fn make_stub_handles_missing_description() {
        let upstream = Tool::new_with_raw("ping", None, schema(json!({})));
        let stub = make_stub_tool("runjs__", "infra", &upstream);
        let desc = stub.description.unwrap();
        assert!(desc.contains("run_js"));
        // No trailing duplicated newlines from empty original docs.
        assert!(!desc.contains("\n\n\n"));
    }

    #[test]
    fn stub_call_instructions_includes_args() {
        let mut args = serde_json::Map::new();
        args.insert("title".into(), json!("hello"));
        let text = stub_call_instructions("github", "create_issue", Some(&args));
        assert!(text.contains("mcp.callTool"));
        assert!(text.contains("\"github\""));
        assert!(text.contains("\"create_issue\""));
        assert!(text.contains("\"title\""));
        assert!(text.contains("\"hello\""));
    }

    #[test]
    fn stub_call_instructions_handles_no_args() {
        let text = stub_call_instructions("srv", "ping", None);
        assert!(text.contains("mcp.callTool"));
        // Should render an empty object placeholder, not "null".
        assert!(text.contains("{}") || text.contains("{\n}"));
    }

    #[test]
    fn manager_stub_tools_lists_every_upstream_tool() {
        let mut by_server = HashMap::new();
        by_server.insert(
            "github".to_string(),
            vec![tool("create_issue", "doc"), tool("close_issue", "doc")],
        );
        by_server.insert("infra".to_string(), vec![tool("ping", "doc")]);
        let mgr = McpClientManager::from_tools_for_test(by_server);

        let mut names: Vec<String> = mgr
            .stub_tools()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "runjs__github__close_issue".to_string(),
                "runjs__github__create_issue".to_string(),
                "runjs__infra__ping".to_string(),
            ]
        );
    }

    #[test]
    fn manager_stub_tools_honours_custom_prefix() {
        let mut by_server = HashMap::new();
        by_server.insert("github".to_string(), vec![tool("create_issue", "doc")]);
        let mgr = McpClientManager::from_tools_for_test(by_server)
            .with_stub_config(StubConfig {
                prefix: "rj_".to_string(),
                enabled: true,
            });

        let names: Vec<String> = mgr.stub_tools().into_iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, vec!["rj_github__create_issue".to_string()]);

        // And the dispatcher recognises the custom-prefixed name.
        let resp = mgr.stub_call_response("rj_github__create_issue", None);
        assert!(resp.is_some());
        // The default-prefix name is no longer recognised.
        assert!(mgr.stub_call_response("runjs__github__create_issue", None).is_none());
    }

    #[test]
    fn manager_stub_tools_empty_when_disabled() {
        let mut by_server = HashMap::new();
        by_server.insert("github".to_string(), vec![tool("create_issue", "doc")]);
        let mgr = McpClientManager::from_tools_for_test(by_server)
            .with_stub_config(StubConfig {
                prefix: "runjs__".to_string(),
                enabled: false,
            });

        // No stub tools advertised at all.
        assert!(mgr.stub_tools().is_empty());
        // And calls to stub-shaped names fall through (return None, so the
        // caller can dispatch as a normal tool / report not-found).
        assert!(mgr.stub_call_response("runjs__github__create_issue", None).is_none());
    }

    #[test]
    fn manager_stub_call_response_matches_known_stub() {
        let mut by_server = HashMap::new();
        by_server.insert("github".to_string(), vec![tool("create_issue", "doc")]);
        let mgr = McpClientManager::from_tools_for_test(by_server);

        let mut args = serde_json::Map::new();
        args.insert("title".into(), json!("hi"));
        let resp = mgr
            .stub_call_response("runjs__github__create_issue", Some(&args))
            .expect("stub should match");
        // Expect a single text content block with usage instructions.
        assert_eq!(resp.is_error, Some(false));
        assert_eq!(resp.content.len(), 1);
        let json = serde_json::to_value(&resp.content[0]).unwrap();
        let text = json.get("text").and_then(|v| v.as_str()).unwrap_or_default();
        assert!(text.contains("mcp.callTool"));
        assert!(text.contains("github"));
        assert!(text.contains("create_issue"));
    }

    #[test]
    fn manager_stub_call_response_returns_none_for_unknowns() {
        let mut by_server = HashMap::new();
        by_server.insert("github".to_string(), vec![tool("create_issue", "doc")]);
        let mgr = McpClientManager::from_tools_for_test(by_server);

        // Built-in tool names: not stubs.
        assert!(mgr.stub_call_response("run_js", None).is_none());
        // Stub-shaped name but unknown server.
        assert!(mgr.stub_call_response("runjs__other__tool", None).is_none());
        // Stub-shaped name with known server but unknown tool.
        assert!(mgr.stub_call_response("runjs__github__delete_issue", None).is_none());
        // Default-prefix dispatcher should reject the old `mcp__` prefix.
        assert!(mgr.stub_call_response("mcp__github__create_issue", None).is_none());
    }

    #[test]
    fn make_stub_drops_annotations_with_nulls() {
        use rmcp::model::ToolAnnotations;

        // Simulate GitHub MCP server: hints with None values that would
        // serialize as JSON null and break Claude Code SDK's Zod validator.
        let mut annotations = ToolAnnotations::default();
        annotations.title = Some("Create a GitHub issue".into());
        let mut upstream = Tool::new(
            "create_issue",
            "Create issue",
            schema(json!({"title": {"type": "string"}})),
        );
        upstream.annotations = Some(annotations);
        let stub = make_stub_tool("runjs__", "github", &upstream);

        // Stubs should never carry upstream annotations — they are discovery
        // mechanisms, not executable tools, so behavioral hints are misleading.
        let json = serde_json::to_value(&stub).unwrap();
        assert!(json.get("annotations").is_none(),
            "stub annotations should be absent to avoid null serialization issues");
    }

    #[test]
    fn make_stub_drops_annotations_even_when_valid() {
        use rmcp::model::ToolAnnotations;

        // Even fully valid annotations are dropped — stubs don't execute.
        let mut annotations = ToolAnnotations::default();
        annotations.title = Some("Get file".into());
        annotations.read_only_hint = Some(true);
        annotations.destructive_hint = Some(false);
        annotations.idempotent_hint = Some(true);
        annotations.open_world_hint = Some(false);
        let mut upstream = Tool::new(
            "get_file",
            "Get file contents",
            schema(json!({"path": {"type": "string"}})),
        );
        upstream.annotations = Some(annotations);
        let stub = make_stub_tool("runjs__", "github", &upstream);

        let json = serde_json::to_value(&stub).unwrap();
        assert!(json.get("annotations").is_none(),
            "stub annotations should be absent");
    }
}
