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

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};

use rmcp::model::{CallToolRequestParam, Tool};
use rmcp::service::Peer;
use rmcp::RoleClient;

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

/// A connected MCP server with its cached tool list.
struct ConnectedMcpServer {
    peer: Peer<RoleClient>,
    tools: Vec<Tool>,
    /// Holds the RunningService alive. Aborting this drops the connection.
    _keep_alive: tokio::task::AbortHandle,
}

// ── McpClientManager ─────────────────────────────────────────────────────

/// Manages connections to multiple MCP servers. Thread-safe and cloneable
/// for sharing across V8 executions (stored in deno_core OpState).
#[derive(Clone)]
pub struct McpClientManager {
    servers: Arc<HashMap<String, ConnectedMcpServer>>,
}

impl McpClientManager {
    /// Connect to all configured MCP servers. Fails fast if any connection fails.
    pub async fn connect(configs: Vec<McpServerConfig>) -> Result<Self, String> {
        let mut servers = HashMap::new();

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
            servers.insert(config.name.clone(), connected);
        }

        Ok(Self {
            servers: Arc::new(servers),
        })
    }

    /// List connected server names.
    pub fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    /// List tools, optionally filtered by server name.
    pub fn list_tools(&self, server_name: Option<&str>) -> Result<Vec<ToolInfo>, String> {
        match server_name {
            Some(name) => {
                let server = self.servers.get(name).ok_or_else(|| {
                    format!(
                        "MCP server '{}' not found. Available: {:?}",
                        name,
                        self.server_names()
                    )
                })?;
                Ok(server
                    .tools
                    .iter()
                    .map(|t| ToolInfo::from_tool(name, t))
                    .collect())
            }
            None => {
                let mut all = Vec::new();
                for (name, server) in self.servers.as_ref() {
                    for tool in &server.tools {
                        all.push(ToolInfo::from_tool(name, tool));
                    }
                }
                Ok(all)
            }
        }
    }

    /// Call a tool on a specific server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<serde_json::Value, String> {
        let server = self.servers.get(server_name).ok_or_else(|| {
            format!(
                "MCP server '{}' not found. Available: {:?}",
                server_name,
                self.server_names()
            )
        })?;

        let result = server
            .peer
            .call_tool(CallToolRequestParam {
                name: tool_name.to_string().into(),
                arguments,
            })
            .await
            .map_err(|e| format!("mcp.callTool({}.{}): {}", server_name, tool_name, e))?;

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
    }
}

// ── Connection logic ─────────────────────────────────────────────────────

async fn connect_one(config: &McpServerConfig) -> Result<ConnectedMcpServer, String> {
    use rmcp::ServiceExt;

    match &config.transport {
        McpServerTransport::Stdio { command, args, env } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args);
            for (k, v) in env {
                cmd.env(k, v);
            }
            let transport = rmcp::transport::TokioChildProcess::new(&mut cmd)
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
        McpServerTransport::Sse { url } => {
            let transport = rmcp::transport::SseTransport::start(url)
                .await
                .map_err(|e| {
                    format!(
                        "Failed to connect to SSE endpoint '{}' for '{}': {}",
                        url, config.name, e
                    )
                })?;

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
