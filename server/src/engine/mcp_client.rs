//! MCP client manager for programmatic tool calling from JavaScript.
//!
//! Connects to external MCP servers at startup (via stdio, SSE, or HTTP
//! transports) and exposes their tools to JS code through a `globalThis.mcp`
//! object. Follows the same deno_core op pattern as `fetch.rs`.
//!
//! JS API:
//! ```js
//! mcp.servers                                   // string[] — connected server names
//! mcp.tools("server")                           // [{server, name, description, inputSchema}, ...]
//! mcp.tools()                                   // all tools from all servers
//! await mcp.github.list_issues({owner: "acme"}) // unwrapped result value — throws McpToolError on error
//! await mcp[server][tool](args)                 // dynamic form of the same call
//! ```
//!
//! Each `mcp.<server>` is a Proxy backed by the live tool catalog: property
//! access dispatches by tool name (original or identifier-sanitized spelling)
//! through `op_mcp_call_tool`, so a namespace captured in a heap snapshot
//! never goes stale — behavior lives in the ops, not in the JS object.
//! Results are unwrapped: `structuredContent` when present, otherwise
//! all-text content is joined and JSON-parsed (falling back to the string),
//! otherwise the raw content-block array is returned. Tool errors throw
//! `McpToolError` carrying the raw envelope on `.result`.

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
use rmcp::service::{NotificationContext, Peer};
use rmcp::{ClientHandler, RoleClient};

// ── Configuration ────────────────────────────────────────────────────────

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
}

/// Configuration for a single named MCP server.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(flatten)]
    pub transport: McpServerTransport,
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

/// Shared, live-updatable catalog of tools per downstream server. Written at
/// connect, on reconnect, and when a downstream emits
/// `notifications/tools/list_changed`; read by every listing, stub, and
/// JS-proxy dispatch path, so the sandbox always sees the current tool set.
type ToolCatalog = Arc<std::sync::RwLock<HashMap<String, Vec<Tool>>>>;

/// Client-side handler installed on every downstream connection. Its only job
/// is catalog freshness: when the downstream emits
/// `notifications/tools/list_changed`, re-list its tools and swap them into
/// the shared catalog. A failed re-list keeps the cached (stale but usable)
/// list and logs a warning rather than erroring.
#[derive(Clone)]
struct CatalogClientHandler {
    server_name: String,
    catalog: ToolCatalog,
}

impl ClientHandler for CatalogClientHandler {
    async fn on_tool_list_changed(&self, context: NotificationContext<RoleClient>) {
        match context.peer.list_all_tools().await {
            Ok(tools) => {
                tracing::info!(
                    "MCP server '{}' tool list changed: now {} tool(s)",
                    self.server_name,
                    tools.len()
                );
                if let Ok(mut catalog) = self.catalog.write() {
                    catalog.insert(self.server_name.clone(), tools);
                }
            }
            Err(e) => tracing::warn!(
                "MCP server '{}' sent tools/list_changed but re-listing failed: {} \
                 (keeping cached tool list)",
                self.server_name,
                e
            ),
        }
    }
}

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

/// A named downstream server: the config needed to reconnect, the shared tool
/// catalog to refresh on reconnect, plus the current live connection behind a
/// lock so a background liveness task (or a failed `call_tool`) can replace
/// it in place.
struct ServerConn {
    config: McpServerConfig,
    catalog: ToolCatalog,
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
/// stale RunningService. The handshake re-lists the downstream's tools, so
/// refresh the shared catalog with them — a restarted downstream may well
/// have a different tool set.
async fn reconnect(server: &ServerConn) -> Result<(), String> {
    let fresh = connect_one(&server.config, &server.catalog).await?;
    if let Ok(mut catalog) = server.catalog.write() {
        catalog.insert(server.config.name.clone(), fresh.tools.clone());
    }
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
/// indirectly through the JavaScript runtime (`run_js` + `await mcp.<server>.<tool>(args)`),
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

/// Server names that collide with fixed properties of the JS `mcp` global
/// (`mcp.servers`, `mcp.tools()`, and the removed-API migration traps).
/// Rejected at startup so a server namespace can never shadow them.
pub const RESERVED_SERVER_NAMES: &[&str] = &["servers", "tools", "callTool", "listTools"];

/// Validate downstream server names: no duplicates, no reserved names.
fn validate_server_names(configs: &[McpServerConfig]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for config in configs {
        if RESERVED_SERVER_NAMES.contains(&config.name.as_str()) {
            return Err(format!(
                "MCP server name '{}' is reserved: it would collide with a property of the \
                 JS `mcp` global. Rename the server. Reserved names: {}",
                config.name,
                RESERVED_SERVER_NAMES.join(", ")
            ));
        }
        if !seen.insert(config.name.clone()) {
            return Err(format!("Duplicate MCP server name: '{}'", config.name));
        }
    }
    Ok(())
}

/// Manages connections to multiple MCP servers. Thread-safe and cloneable
/// for sharing across V8 executions (stored in deno_core OpState).
///
/// `tools_by_server` is the live source of truth for tool listings (and the
/// basis for the auto-generated MCP tool stubs that MCPJS exposes to its own
/// clients). It is populated during `connect()`, refreshed on reconnect and
/// on `tools/list_changed` notifications, and can be populated independently
/// for tests via `from_tools_for_test()`.
#[derive(Clone)]
pub struct McpClientManager {
    tools_by_server: ToolCatalog,
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
        validate_server_names(&configs)?;

        let catalog: ToolCatalog = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let mut servers = HashMap::new();

        for config in configs {
            tracing::info!("Connecting to MCP server '{}'...", config.name);
            let connected = connect_one(&config, &catalog).await?;

            tracing::info!(
                "MCP server '{}': {} tool(s) available",
                config.name,
                connected.tools.len()
            );
            for tool in &connected.tools {
                tracing::info!("  - {}.{}", config.name, tool.name);
            }

            let name = config.name.clone();
            catalog
                .write()
                .expect("tool catalog lock poisoned")
                .insert(name.clone(), connected.tools.clone());
            let server = Arc::new(ServerConn {
                config,
                catalog: catalog.clone(),
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
            tools_by_server: catalog,
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
            tools_by_server: Arc::new(std::sync::RwLock::new(tools_by_server)),
            servers: Arc::new(HashMap::new()),
            stub_config: StubConfig::default(),
            runtime: None,
        }
    }

    /// Test-only: replace one server's tool list in the live catalog,
    /// simulating a `tools/list_changed` refresh.
    #[cfg(test)]
    pub fn set_tools_for_test(&self, server: &str, tools: Vec<Tool>) {
        self.tools_by_server
            .write()
            .expect("tool catalog lock poisoned")
            .insert(server.to_string(), tools);
    }

    /// List connected server names.
    pub fn server_names(&self) -> Vec<String> {
        self.tools_by_server
            .read()
            .expect("tool catalog lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    /// List tools, optionally filtered by server name.
    pub fn list_tools(&self, server_name: Option<&str>) -> Result<Vec<ToolInfo>, String> {
        let catalog = self
            .tools_by_server
            .read()
            .expect("tool catalog lock poisoned");
        match server_name {
            Some(name) => {
                let tools = catalog.get(name).ok_or_else(|| {
                    format!(
                        "MCP server '{}' not found. Available: {:?}",
                        name,
                        catalog.keys().cloned().collect::<Vec<_>>()
                    )
                })?;
                Ok(tools.iter().map(|t| ToolInfo::from_tool(name, t)).collect())
            }
            None => {
                let mut all = Vec::new();
                for (name, tools) in catalog.iter() {
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
    /// through the JavaScript runtime (`run_js` → `await mcp.<server>.<tool>(args)`).
    /// Returns an empty vec when stub exposure is disabled in the config.
    pub fn stub_tools(&self) -> Vec<Tool> {
        if !self.stub_config.enabled {
            return Vec::new();
        }
        let catalog = self
            .tools_by_server
            .read()
            .expect("tool catalog lock poisoned");
        let mut out = Vec::new();
        for (server, tools) in catalog.iter() {
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
        {
            let catalog = self
                .tools_by_server
                .read()
                .expect("tool catalog lock poisoned");
            let tools = catalog.get(&server)?;
            if !tools.iter().any(|t| t.name.as_ref() == tool) {
                return None;
            }
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
                        return Err(format!("mcp.{}.{}: {}", server_name, tool_name, e));
                    }
                    tracing::warn!(
                        "MCP server '{}' looks disconnected ({}); reconnecting and retrying",
                        server_name,
                        e
                    );
                    reconnect(&server).await.map_err(|re| {
                        format!(
                            "mcp.{}.{}: reconnect failed: {}",
                            server_name, tool_name, re
                        )
                    })?;
                    let peer = { server.live.read().await.peer.clone() };
                    peer.call_tool(make_req()).await.map_err(|e| {
                        format!(
                            "mcp.{}.{}: {} (after reconnect)",
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

            let mut response = serde_json::json!({
                "content": content_json,
                "isError": result.is_error.unwrap_or(false),
            });
            // Per the MCP spec, structuredContent is the authoritative result
            // when a tool declares an outputSchema; text content mirrors it
            // for backward compatibility. Pass it through so the JS unwrap
            // ladder can prefer it.
            if let Some(sc) = result.structured_content {
                response["structuredContent"] = sc;
            }
            Ok(response)
        };

        match &self.runtime {
            Some(rt) => rt
                .spawn(call)
                .await
                .map_err(|e| format!("mcp tool call: task join error: {e}"))?,
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

/// True when `s` is a valid JS identifier (dot-access safe).
fn is_js_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Render the canonical JS accessor for a server/tool pair: dot syntax when
/// the names are identifier-safe (`mcp.github.create_issue`), bracket syntax
/// otherwise (`mcp["my-server"]["my.tool"]`).
pub fn js_accessor(server: &str, tool: &str) -> String {
    let server_part = if is_js_ident(server) {
        format!("mcp.{}", server)
    } else {
        format!("mcp[{:?}]", server)
    };
    if is_js_ident(tool) {
        format!("{}.{}", server_part, tool)
    } else {
        format!("{}[{:?}]", server_part, tool)
    }
}

/// Build a stub `Tool` mirroring an upstream tool's schema. The description
/// is rewritten to make it clear the tool is invoked via `run_js`.
pub fn make_stub_tool(prefix: &str, server: &str, tool: &Tool) -> Tool {
    let stub_name = stub_tool_name(prefix, server, &tool.name);
    let original_desc = tool.description.as_deref().unwrap_or("");
    let header = format!(
        "[stub for upstream MCP tool {server}.{tool} — invoke via run_js: \
         `await {accessor}(args)`. Calling this tool \
         directly only returns instructions; it does not execute.]",
        server = server,
        tool = tool.name,
        accessor = js_accessor(server, &tool.name),
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
         const result = await {accessor}({pretty});\n\
         console.log(JSON.stringify(result));\n\
         \n\
         The call resolves to the tool's unwrapped result value (structured \
         content, or parsed JSON when the tool returns JSON text) and throws \
         McpToolError on tool errors (raw envelope on `error.result`).\n",
        accessor = js_accessor(server, tool),
        pretty = pretty,
    )
}

// ── Connection logic ─────────────────────────────────────────────────────

async fn connect_one(
    config: &McpServerConfig,
    catalog: &ToolCatalog,
) -> Result<ConnectedMcpServer, String> {
    use rmcp::ServiceExt;

    // Handler that keeps the shared catalog fresh on tools/list_changed.
    let handler = CatalogClientHandler {
        server_name: config.name.clone(),
        catalog: catalog.clone(),
    };

    match &config.transport {
        McpServerTransport::Stdio { command, args, env } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args);
            for (k, v) in env {
                cmd.env(k, v);
            }
            let transport = rmcp::transport::TokioChildProcess::new(cmd)
                .map_err(|e| format!("Failed to spawn '{}': {}", command, e))?;

            let service: rmcp::service::RunningService<RoleClient, CatalogClientHandler> =
                handler.serve(transport)
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
        McpServerTransport::Sse { url } => {
            // The standalone SSE client transport was removed in rmcp 1.x; the
            // Streamable HTTP client transport is its replacement and speaks to
            // the same `/mcp`-style endpoints modern MCP servers expose.
            let transport = rmcp::transport::StreamableHttpClientTransport::from_uri(url.clone());

            let service: rmcp::service::RunningService<RoleClient, CatalogClientHandler> =
                handler.serve(transport)
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
    /// Optional OPA policy chain for gating MCP tool calls from JS.
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
                    JsErrorBox::generic(format!("mcp tool call: invalid arguments JSON: {}", e))
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
                .map_err(|e| JsErrorBox::generic(format!("mcp tool call: failed to serialize policy input: {}", e)))?;
            let allowed = chain.evaluate(&input_value).await
                .map_err(|e| JsErrorBox::generic(format!("mcp tool call: policy evaluation error: {}", e)))?;
            if !allowed {
                return Err(JsErrorBox::generic(format!(
                    "mcp.{}.{} denied by policy",
                    server_name, tool_name
                )));
            }
        }

        let result = manager
            .call_tool(&server_name, &tool_name, arguments)
            .await
            .map_err(|e| JsErrorBox::generic(e))?;
        serde_json::to_string(&result)
            .map_err(|e| JsErrorBox::generic(format!("mcp tool call: serialization error: {}", e)))
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
        .map_err(|e| JsErrorBox::generic(format!("mcp.tools: serialization error: {}", e)))
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
///
/// Each connected server becomes a Proxy namespace (`mcp.<server>`) whose
/// property accesses dispatch tool calls through the ops against the live
/// catalog. The proxies carry no catalog data themselves, so instances
/// captured in heap snapshots keep working after tool lists change.
const MCP_JS_WRAPPER: &str = r#"
(function() {
    /**
     * Error thrown when an MCP tool returns an error result.
     * The raw result envelope ({content, structuredContent?, isError}) is
     * available on the `result` property.
     */
    class McpToolError extends Error {
        constructor(serverName, toolName, result) {
            var text = (result.content && result.content.length > 0 && result.content[0].text)
                ? result.content[0].text
                : 'Tool returned an error';
            super('mcp.' + serverName + '.' + toolName + ' failed: ' + text);
            this.name = 'McpToolError';
            this.result = result;
            this.serverName = serverName;
            this.toolName = toolName;
        }
    }
    globalThis.McpToolError = McpToolError;

    // Map a tool name to a JS-identifier spelling (dot-access friendly).
    function sanitize(name) {
        var s = String(name).replace(/[^A-Za-z0-9_$]/g, '_');
        if (/^[0-9]/.test(s)) s = '_' + s;
        return s;
    }

    function listToolsFor(serverName) {
        var raw = Deno.core.ops.op_mcp_list_tools(serverName || '');
        return JSON.parse(raw);
    }

    // Unwrap a raw result envelope into a plain value:
    //   1. structuredContent, when the tool provided it (authoritative per spec)
    //   2. all-text content: joined, JSON.parse with fallback to the string
    //   3. anything else (mixed/binary blocks): the raw content array
    function unwrap(result) {
        if (result.structuredContent !== undefined && result.structuredContent !== null) {
            return result.structuredContent;
        }
        var content = result.content || [];
        if (content.length === 0) return null;
        var allText = content.every(function(c) { return c && c.type === 'text' && typeof c.text === 'string'; });
        if (allText) {
            var text = content.map(function(c) { return c.text; }).join('\n');
            try { return JSON.parse(text); } catch (_) { return text; }
        }
        return content;
    }

    function makeInvoker(serverName, toolName) {
        return async function(args) {
            if (args === undefined || args === null) args = {};
            if (typeof args !== 'object' || Array.isArray(args)) {
                throw new TypeError('mcp.' + serverName + '.' + toolName + ': args must be a plain object');
            }
            var raw = await Deno.core.ops.op_mcp_call_tool(serverName, toolName, JSON.stringify(args));
            var result = JSON.parse(raw);
            if (result.isError) {
                throw new McpToolError(serverName, toolName, result);
            }
            return unwrap(result);
        };
    }

    // Property names that must never resolve to a tool invoker. `then` is
    // load-bearing: returning a function would make the namespace thenable
    // and break `await mcp.<server>`. Tools that share one of these names
    // are still listed by mcp.tools() but cannot be dot-invoked.
    var SKIP_PROPS = {
        then: true, catch: true, finally: true,
        constructor: true, prototype: true, __proto__: true,
        valueOf: true, inspect: true,
    };

    function findTool(serverName, prop) {
        var tools = listToolsFor(serverName);
        for (var i = 0; i < tools.length; i++) {
            if (tools[i].name === prop) return { tool: tools[i] };
        }
        var matches = tools.filter(function(t) { return sanitize(t.name) === prop; });
        if (matches.length === 1) return { tool: matches[0] };
        if (matches.length > 1) return { ambiguous: matches };
        return { unknown: tools };
    }

    function makeServerProxy(serverName) {
        return new Proxy({}, {
            get: function(_t, prop) {
                if (typeof prop !== 'string') return undefined;
                if (prop === 'toString') {
                    return function() {
                        var names = listToolsFor(serverName).map(function(t) { return t.name; });
                        return '[mcp server ' + serverName + ': ' + names.join(', ') + ']';
                    };
                }
                if (prop === 'toJSON') {
                    return function() {
                        return {
                            server: serverName,
                            tools: listToolsFor(serverName).map(function(t) { return t.name; }),
                        };
                    };
                }
                if (SKIP_PROPS[prop]) return undefined;
                var found = findTool(serverName, prop);
                if (found.tool) return makeInvoker(serverName, found.tool.name);
                if (found.ambiguous) {
                    var originals = found.ambiguous.map(function(t) { return t.name; });
                    throw new Error(
                        'mcp.' + serverName + '.' + prop + ' is ambiguous (matches: ' + originals.join(', ') +
                        '). Use the exact tool name, e.g. mcp[' + JSON.stringify(serverName) + '][' + JSON.stringify(originals[0]) + '](args)');
                }
                return function() {
                    var names = found.unknown.map(function(t) { return t.name; });
                    throw new Error(
                        'mcp.' + serverName + ' has no tool ' + JSON.stringify(prop) +
                        '. Available tools: ' + (names.length ? names.join(', ') : '(none)'));
                };
            },
            has: function(_t, prop) {
                if (typeof prop !== 'string') return false;
                return !!findTool(serverName, prop).tool;
            },
            ownKeys: function(_t) {
                var keys = [];
                var seen = {};
                listToolsFor(serverName).forEach(function(t) {
                    var k = sanitize(t.name);
                    if (seen[k]) k = t.name; // sanitized-name collision: keep the original spelling
                    if (!seen[k]) { seen[k] = true; keys.push(k); }
                });
                return keys;
            },
            getOwnPropertyDescriptor: function(_t, prop) {
                if (typeof prop !== 'string') return undefined;
                if (!findTool(serverName, prop).tool) return undefined;
                return { enumerable: true, configurable: true, writable: false, value: undefined };
            },
        });
    }

    var mcp = {
        /**
         * Get the list of connected MCP server names.
         * @returns {string[]}
         */
        get servers() {
            var raw = Deno.core.ops.op_mcp_list_servers();
            return JSON.parse(raw);
        },

        /**
         * List available tools, optionally filtered by server name. Always
         * reflects the live catalog (refreshed on reconnect/list_changed).
         * Each tool has: server, name, description, inputSchema.
         * @param {string} [serverName] - If provided, list only tools for this server
         * @returns {Array<{server: string, name: string, description: string|null, inputSchema: object}>}
         */
        tools: function(serverName) {
            return listToolsFor(serverName);
        },

        // Migration traps for the removed v1 API.
        callTool: function() {
            throw new Error(
                'mcp.callTool() was removed: call tools directly instead, e.g. ' +
                '`await mcp.github.list_issues({owner: "acme"})` (dynamic form: ' +
                '`await mcp[server][tool](args)`). Calls resolve to the unwrapped ' +
                'result value; tool errors throw McpToolError with the raw envelope ' +
                'on `error.result`. Discover tools with mcp.tools().');
        },
        listTools: function() {
            throw new Error('mcp.listTools() was replaced by mcp.tools(serverName?) — same return shape.');
        },
    };

    JSON.parse(Deno.core.ops.op_mcp_list_servers()).forEach(function(serverName) {
        var proxy = makeServerProxy(serverName);
        // defineProperty (not assignment) so a server named "__proto__"
        // could never mutate the prototype chain.
        Object.defineProperty(mcp, serverName, {
            value: proxy, enumerable: true, configurable: true, writable: false,
        });
        var alias = sanitize(serverName);
        if (alias !== serverName && !(alias in mcp)) {
            Object.defineProperty(mcp, alias, {
                value: proxy, enumerable: false, configurable: true, writable: false,
            });
        }
    });

    globalThis.mcp = mcp;
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
        assert!(
            desc.contains("mcp.github.create_issue"),
            "description should mention the proxy accessor: {}",
            desc
        );
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
        assert!(text.contains("await mcp.github.create_issue("));
        assert!(text.contains("\"title\""));
        assert!(text.contains("\"hello\""));
    }

    #[test]
    fn stub_call_instructions_handles_no_args() {
        let text = stub_call_instructions("srv", "ping", None);
        assert!(text.contains("await mcp.srv.ping("));
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
        assert!(text.contains("await mcp.github.create_issue("));
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

    #[test]
    fn js_accessor_uses_dot_syntax_for_identifiers() {
        assert_eq!(js_accessor("github", "create_issue"), "mcp.github.create_issue");
        assert_eq!(js_accessor("_srv", "$tool"), "mcp._srv.$tool");
    }

    #[test]
    fn js_accessor_uses_brackets_for_non_identifiers() {
        assert_eq!(js_accessor("my-server", "create_issue"), "mcp[\"my-server\"].create_issue");
        assert_eq!(js_accessor("github", "my.tool"), "mcp.github[\"my.tool\"]");
        assert_eq!(js_accessor("1srv", "2tool"), "mcp[\"1srv\"][\"2tool\"]");
        assert_eq!(js_accessor("", ""), "mcp[\"\"][\"\"]");
    }

    #[test]
    fn validate_rejects_reserved_server_names() {
        for reserved in RESERVED_SERVER_NAMES {
            let configs = vec![McpServerConfig {
                name: reserved.to_string(),
                transport: McpServerTransport::Sse { url: "http://localhost:1/mcp".into() },
            }];
            let err = validate_server_names(&configs).expect_err("reserved name must be rejected");
            assert!(err.contains("reserved"), "error should say reserved: {}", err);
            assert!(err.contains(reserved), "error should name the offender: {}", err);
        }
    }

    #[test]
    fn validate_rejects_duplicate_server_names() {
        let mk = |name: &str| McpServerConfig {
            name: name.to_string(),
            transport: McpServerTransport::Sse { url: "http://localhost:1/mcp".into() },
        };
        let err = validate_server_names(&[mk("github"), mk("github")])
            .expect_err("duplicate must be rejected");
        assert!(err.contains("Duplicate"), "error should say duplicate: {}", err);
        assert!(validate_server_names(&[mk("github"), mk("jira")]).is_ok());
    }

    #[test]
    fn catalog_updates_are_visible_to_listing_and_stubs() {
        // Simulates a tools/list_changed refresh: listings and stub tools
        // must reflect the new catalog immediately (live reads, no caching).
        let mut by_server = HashMap::new();
        by_server.insert("github".to_string(), vec![tool("create_issue", "doc")]);
        let mgr = McpClientManager::from_tools_for_test(by_server);

        assert_eq!(mgr.list_tools(Some("github")).unwrap().len(), 1);
        assert!(mgr.stub_call_response("runjs__github__close_issue", None).is_none());

        mgr.set_tools_for_test(
            "github",
            vec![tool("create_issue", "doc"), tool("close_issue", "doc")],
        );

        let names: Vec<String> = mgr
            .list_tools(Some("github"))
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["create_issue".to_string(), "close_issue".to_string()]);
        // Newly appeared tool is now stub-dispatchable and advertised.
        assert!(mgr.stub_call_response("runjs__github__close_issue", None).is_some());
        assert_eq!(mgr.stub_tools().len(), 2);
    }
}
