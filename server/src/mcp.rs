//! MCP service handlers — exposes the V8 engine to MCP clients.
//!
//! Two orthogonal axes select between four `ServerHandler` impls:
//!   - Stateful (`McpService`)   vs Stateless (`StatelessMcpService`)
//!   - Legacy `execution_id` polling vs Tasks-API-spec compliant (`*TasksApiMcpService`)
//!
//! `main.rs` picks one at startup via `(--stateless, --use-tasks-api)`.
//! Shared state and helpers live in [`McpCore`].

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::engine::Engine;
use crate::engine::heap_tags::HeapTagEntry;
use crate::engine::mcp_client::McpClientManager;
use crate::session::SessionVerifier;

// ── Embedded documentation resources ───────────────────────────────────

const LLMS_TXT: &str = include_str!("llms_txt.md");
const README_MD: &str = include_str!("../README.md");

fn doc_resources(stateful: bool, tasks_api: bool) -> Vec<Resource> {
    let mut tools: Vec<serde_json::Value> = if tasks_api {
        vec![json!({
            "name": "run_js",
            "description": if stateful {
                "Submit JS/TS for async execution (stateful, Tasks API). Returns a Task; poll tasks/get and tasks/result."
            } else {
                "Submit JS/TS for async execution (stateless, Tasks API). Returns a Task; poll tasks/get and tasks/result."
            },
        })]
    } else {
        vec![
            json!({ "name": "run_js", "description": if stateful { "Submit JS/TS for async execution (stateful). Returns execution_id." } else { "Submit JS/TS for async execution (stateless). Returns execution_id." } }),
            json!({ "name": "get_execution",         "description": "Poll execution status and result." }),
            json!({ "name": "get_execution_output",  "description": "Read paginated console output (line or byte mode)." }),
            json!({ "name": "cancel_execution",      "description": "Terminate a running execution." }),
            json!({ "name": "list_executions",       "description": "List all executions with their status." }),
        ]
    };
    if stateful {
        tools.extend([
            json!({ "name": "list_sessions",           "description": "List all named sessions (stateful only)." }),
            json!({ "name": "list_session_snapshots",  "description": "Browse execution history for a session." }),
            json!({ "name": "get_heap_tags",           "description": "Get tags for a heap snapshot." }),
            json!({ "name": "set_heap_tags",           "description": "Set tags on a heap snapshot." }),
            json!({ "name": "delete_heap_tags",        "description": "Delete tag keys from a heap snapshot." }),
            json!({ "name": "query_heaps_by_tags",     "description": "Find heap snapshots matching tag criteria." }),
        ]);
    }
    let _ = tools; // intentionally unused beyond docs://tools generation

    vec![
        Annotated::new(
            RawResource {
                uri: "docs://readme".into(),
                name: "README".into(),
                title: None,
                description: Some("Full mcp-v8 README with usage, CLI flags, and examples (Markdown)".into()),
                mime_type: Some("text/markdown".into()),
                size: Some(README_MD.len() as u32),
                icons: None,
                meta: None,
            },
            None,
        ),
        Annotated::new(
            RawResource {
                uri: "docs://llms-txt".into(),
                name: "llms.txt".into(),
                title: None,
                description: Some("Machine-readable agent guide: connection options, tools, REST API (Markdown)".into()),
                mime_type: Some("text/markdown".into()),
                size: Some(LLMS_TXT.len() as u32),
                icons: None,
                meta: None,
            },
            None,
        ),
        Annotated::new(
            RawResource {
                uri: "docs://openapi".into(),
                name: "OpenAPI spec".into(),
                title: None,
                description: Some("OpenAPI 3.0 JSON spec for the REST API (/api/exec, /api/executions/*, etc.)".into()),
                mime_type: Some("application/json".into()),
                size: None,
                icons: None,
                meta: None,
            },
            None,
        ),
        Annotated::new(
            RawResource {
                uri: "docs://tools".into(),
                name: "MCP tool list".into(),
                title: None,
                description: Some("JSON list of available MCP tools with descriptions, mode-aware".into()),
                mime_type: Some("application/json".into()),
                size: None,
                icons: None,
                meta: None,
            },
            None,
        ),
    ]
}

fn read_doc_resource(uri: &str, stateful: bool, tasks_api: bool) -> Option<ReadResourceResult> {
    use crate::api::ApiDoc;
    use utoipa::OpenApi as _;

    let openapi_json = serde_json::to_string_pretty(&ApiDoc::openapi()).unwrap_or_default();

    let mut tools: Vec<serde_json::Value> = if tasks_api {
        vec![json!({
            "name": "run_js",
            "description": if stateful {
                "Submit JS/TS for async execution (stateful, Tasks API). Returns a Task."
            } else {
                "Submit JS/TS for async execution (stateless, Tasks API). Returns a Task."
            },
        })]
    } else {
        vec![
            json!({ "name": "run_js",               "description": if stateful { "Submit JS/TS for async execution (stateful). Returns execution_id." } else { "Submit JS/TS for async execution (stateless). Returns execution_id." } }),
            json!({ "name": "get_execution",         "description": "Poll execution status and result." }),
            json!({ "name": "get_execution_output",  "description": "Read paginated console output (line or byte mode)." }),
            json!({ "name": "cancel_execution",      "description": "Terminate a running execution." }),
            json!({ "name": "list_executions",       "description": "List all executions with their status." }),
        ]
    };
    if stateful {
        tools.extend([
            json!({ "name": "list_sessions",           "description": "List all named sessions (stateful only)." }),
            json!({ "name": "list_session_snapshots",  "description": "Browse execution history for a session." }),
            json!({ "name": "get_heap_tags",           "description": "Get tags for a heap snapshot." }),
            json!({ "name": "set_heap_tags",           "description": "Set tags on a heap snapshot." }),
            json!({ "name": "delete_heap_tags",        "description": "Delete tag keys from a heap snapshot." }),
            json!({ "name": "query_heaps_by_tags",     "description": "Find heap snapshots matching tag criteria." }),
        ]);
    }
    let tools_json = serde_json::to_string_pretty(&json!({
        "mode": if stateful { "stateful" } else { "stateless" },
        "tasksApi": tasks_api,
        "tools": tools,
    })).unwrap_or_default();

    let contents = match uri {
        "docs://readme" => vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("text/markdown".into()),
            text: README_MD.to_string(),
            meta: None,
        }],
        "docs://llms-txt" => vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("text/markdown".into()),
            text: LLMS_TXT.to_string(),
            meta: None,
        }],
        "docs://openapi" => vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("application/json".into()),
            text: openapi_json,
            meta: None,
        }],
        "docs://tools" => vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("application/json".into()),
            text: tools_json,
            meta: None,
        }],
        _ => return None,
    };

    Some(ReadResourceResult::new(contents))
}

// ── Tool argument structs ───────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RunJsArgs {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap_memory_max_mb: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct StatelessRunJsArgs {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap_memory_max_mb: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ExecutionIdArgs {
    pub execution_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetExecutionOutputArgs {
    pub execution_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_limit: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ListSessionSnapshotsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct HeapArgs {
    pub heap: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SetHeapTagsArgs {
    pub heap: String,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DeleteHeapTagsArgs {
    pub heap: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct QueryHeapTagsArgs {
    pub tags: HashMap<String, String>,
}

// ── Content builders ────────────────────────────────────────────────────

fn ok_text(value: serde_json::Value) -> CallToolResult {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    CallToolResult::success(vec![Content::text(text)])
}

fn ok_bool(ok: bool, error: Option<String>) -> CallToolResult {
    let value = match error {
        Some(e) => json!({ "ok": ok, "error": e }),
        None => json!({ "ok": ok }),
    };
    ok_text(value)
}

// ── McpCore: shared state and helpers ───────────────────────────────────

/// Shared state used by every `ServerHandler` impl in this module.
#[derive(Clone)]
pub struct McpCore {
    pub engine: Engine,
    pub verifier: Option<Arc<SessionVerifier>>,
    pub mcp_client: Option<Arc<McpClientManager>>,
    /// Set once during `initialize` from `X-MCP-Session-Id`.
    pub session_id: Arc<OnceLock<String>>,
    /// `X-MCP-*` headers from the initialize request.
    pub mcp_headers: Arc<OnceLock<serde_json::Value>>,
}

impl McpCore {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        let mcp_client = engine.mcp_client_manager();
        Self {
            engine,
            verifier,
            mcp_client,
            session_id: Arc::new(OnceLock::new()),
            mcp_headers: Arc::new(OnceLock::new()),
        }
    }

    /// Process the initialize request: JWT check + capture `X-MCP-*` headers
    /// and `X-MCP-Session-Id` from the incoming HTTP request (if any).
    pub async fn handle_initialize(&self, context: &RequestContext<RoleServer>) {
        let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() else {
            return;
        };
        let initialize_headers = &http_request_part.headers;
        let initialize_uri = &http_request_part.uri;
        tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");

        if let Some(ref verifier) = self.verifier {
            let token = http_request_part.headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .or_else(|| {
                    http_request_part.headers
                        .get("agent-session")
                        .and_then(|v| v.to_str().ok())
                });
            match token {
                Some(token) => if verifier.verify(token).await {
                    tracing::info!("JWT verified");
                } else {
                    tracing::warn!("JWT present but failed verification");
                },
                None => {
                    tracing::debug!("No Authorization/AgentSession header in initialize request");
                }
            }
        }

        let mut mcp_header_map = serde_json::Map::new();
        for (name, value) in initialize_headers.iter() {
            if let Some(key) = name.as_str().strip_prefix("x-mcp-") {
                if let Ok(v) = value.to_str() {
                    mcp_header_map.insert(key.to_string(), serde_json::Value::String(v.to_string()));
                }
            }
        }

        if let Some(serde_json::Value::String(sid)) = mcp_header_map.get("session-id") {
            tracing::info!(session_id = sid.as_str(), "Session ID from X-MCP-Session-Id header");
            let _ = self.session_id.set(sid.clone());
        }

        if !mcp_header_map.is_empty() {
            tracing::info!(?mcp_header_map, "X-MCP-* headers captured");
            let _ = self.mcp_headers.set(serde_json::Value::Object(mcp_header_map));
        }
    }

    /// Submit a `run_js` request and return the execution ID. Stateful flavour
    /// (carries the session ID forward).
    pub async fn start_run_js_stateful(&self, args: RunJsArgs) -> Result<String, String> {
        let mut req = self.engine.run_js(args.code);
        if let Some(h) = args.heap { req = req.heap(h); }
        if let Some(s) = self.session_id.get() { req = req.session(s.clone()); }
        if let Some(mb) = args.heap_memory_max_mb { req = req.heap_memory_max_mb(mb); }
        if let Some(secs) = args.execution_timeout_secs { req = req.execution_timeout_secs(secs); }
        if let Some(t) = args.tags { req = req.tags(t); }
        req = req.maybe_mcp_headers(self.mcp_headers.get().cloned());
        req.execute().await
    }

    /// Submit a `run_js` request and return the execution ID. Stateless flavour
    /// (still records the session ID so `tasks/list` can scope results).
    pub async fn start_run_js_stateless(&self, args: StatelessRunJsArgs) -> Result<String, String> {
        let mut req = self.engine.run_js(args.code);
        if let Some(s) = self.session_id.get() { req = req.session(s.clone()); }
        if let Some(mb) = args.heap_memory_max_mb { req = req.heap_memory_max_mb(mb); }
        if let Some(secs) = args.execution_timeout_secs { req = req.execution_timeout_secs(secs); }
        req = req.maybe_mcp_headers(self.mcp_headers.get().cloned());
        req.execute().await
    }

    /// Poll the execution registry until the given execution reaches a
    /// terminal status, then return its `ExecutionInfo`. Used by stateless
    /// `run_js` (legacy) and the Tasks API `tasks/result` handler.
    pub async fn wait_for_terminal(
        &self,
        exec_id: &str,
    ) -> Result<crate::engine::execution::ExecutionInfo, String> {
        let poll_interval = tokio::time::Duration::from_millis(50);
        let max_polls = 6000; // 5 minutes at 50ms intervals
        for _ in 0..max_polls {
            tokio::time::sleep(poll_interval).await;
            if let Ok(info) = self.engine.get_execution(exec_id) {
                match info.status.as_str() {
                    "running" => continue,
                    _ => return Ok(info),
                }
            }
        }
        Err("Execution did not complete within polling timeout".to_string())
    }

    /// Build a CallToolResult for a finished execution, mirroring the
    /// payload that legacy stateless `run_js` returned: console output (and
    /// error string when the execution did not complete successfully).
    pub fn build_stateless_run_result(
        &self,
        exec_id: &str,
        info: &crate::engine::execution::ExecutionInfo,
    ) -> CallToolResult {
        let output = match self.engine.get_execution_output(exec_id, None, Some(u64::MAX), None, None) {
            Ok(page) => page.data,
            Err(_) => String::new(),
        };
        let value = match info.status.as_str() {
            "completed" => json!({ "output": output }),
            _ => json!({ "output": output, "error": info.error }),
        };
        let is_error = info.status.as_str() != "completed";
        let mut r = ok_text(value);
        r.is_error = Some(is_error);
        r
    }

    /// Build a CallToolResult for a finished stateful execution, matching
    /// the JSON returned by legacy `get_execution`.
    pub fn build_stateful_run_result(
        &self,
        info: &crate::engine::execution::ExecutionInfo,
    ) -> CallToolResult {
        let value = json!({
            "execution_id": info.id,
            "status": info.status,
            "result": info.result,
            "heap": info.heap,
            "error": info.error,
            "started_at": info.started_at,
            "completed_at": info.completed_at,
        });
        let is_error = info.status.as_str() != "completed";
        let mut r = ok_text(value);
        r.is_error = Some(is_error);
        r
    }

    /// Map an `ExecutionInfo`'s status string to a [`TaskStatus`].
    fn map_status(status: &str) -> TaskStatus {
        match status {
            "running"   => TaskStatus::Working,
            "completed" => TaskStatus::Completed,
            "failed" | "timed_out" => TaskStatus::Failed,
            "cancelled" => TaskStatus::Cancelled,
            _           => TaskStatus::Working,
        }
    }

    /// Build a `Task` value for `tasks/get` and `tasks/cancel` responses.
    fn make_task(info: &crate::engine::execution::ExecutionInfo, ttl: Option<u64>) -> Task {
        let last_updated_at = info.completed_at.clone().unwrap_or_else(|| info.started_at.clone());
        let mut task = Task::new(
            info.id.clone(),
            Self::map_status(&info.status),
            info.started_at.clone(),
            last_updated_at,
        );
        if let Some(t) = ttl { task = task.with_ttl(t); }
        task = task.with_poll_interval(500);
        if let Some(err) = &info.error {
            task = task.with_status_message(err.clone());
        }
        task
    }
}

// ── Helpers shared by stub-tool dispatch ────────────────────────────────

fn try_stub_call(
    mcp_client: &Option<Arc<McpClientManager>>,
    name: &str,
    arguments: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<CallToolResult> {
    let client = mcp_client.as_ref()?;
    client.stub_call_response(name, arguments)
}

fn append_stub_tools(mcp_client: &Option<Arc<McpClientManager>>, tools: &mut Vec<Tool>) {
    if let Some(client) = mcp_client {
        tools.extend(client.stub_tools());
    }
}

// ───────────────────────────────────────────────────────────────────────
// 1. McpService — stateful, legacy execution_id model
// ───────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct McpService {
    core: McpCore,
    tool_router: ToolRouter<Self>,
}

impl McpService {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        Self {
            core: McpCore::new(engine, verifier),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl McpService {
    #[doc = include_str!("run_js_tool_description.md")]
    #[tool]
    pub async fn run_js(
        &self,
        Parameters(args): Parameters<RunJsArgs>,
    ) -> Result<CallToolResult, McpError> {
        match self.core.start_run_js_stateful(args).await {
            Ok(execution_id) => Ok(ok_text(json!({ "execution_id": execution_id }))),
            Err(e) => Ok(ok_text(json!({ "execution_id": format!("error: {}", e) }))),
        }
    }

    #[tool(description = "Get the status and result of an execution. Returns execution_id, status (running/completed/failed/cancelled/timed_out), result (if completed), heap (if stateful), error (if failed), started_at, and completed_at.")]
    pub async fn get_execution(
        &self,
        Parameters(ExecutionIdArgs { execution_id }): Parameters<ExecutionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        match self.core.engine.get_execution(&execution_id) {
            Ok(info) => Ok(ok_text(json!({
                "execution_id": info.id,
                "status": info.status,
                "result": info.result,
                "heap": info.heap,
                "error": info.error,
                "started_at": info.started_at,
                "completed_at": info.completed_at,
            }))),
            Err(e) => Ok(ok_text(json!({ "error": e }))),
        }
    }

    #[tool(description = "Get paginated console output for an execution. Supports two modes: line-based (line_offset + line_limit) or byte-based (byte_offset + byte_limit). If byte_offset is provided, byte mode takes precedence. Response includes both line and byte coordinates for cross-referencing. Use next_line_offset or next_byte_offset from a previous response to resume reading.")]
    pub async fn get_execution_output(
        &self,
        Parameters(args): Parameters<GetExecutionOutputArgs>,
    ) -> Result<CallToolResult, McpError> {
        let status = self.core.engine.get_execution(&args.execution_id)
            .map(|info| info.status)
            .unwrap_or_else(|_| "unknown".to_string());

        let r = self.core.engine.get_execution_output(
            &args.execution_id,
            args.line_offset,
            args.line_limit,
            args.byte_offset,
            args.byte_limit,
        );
        match r {
            Ok(page) => Ok(ok_text(json!({
                "execution_id": args.execution_id,
                "data": page.data,
                "start_line": page.start_line,
                "end_line": page.end_line,
                "next_line_offset": page.next_line_offset,
                "total_lines": page.total_lines,
                "start_byte": page.start_byte,
                "end_byte": page.end_byte,
                "next_byte_offset": page.next_byte_offset,
                "total_bytes": page.total_bytes,
                "has_more": page.has_more,
                "status": status,
            }))),
            Err(e) => Ok(ok_text(json!({ "error": e }))),
        }
    }

    #[tool(description = "Cancel a running execution. Terminates the V8 isolate.")]
    pub async fn cancel_execution(
        &self,
        Parameters(ExecutionIdArgs { execution_id }): Parameters<ExecutionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(match self.core.engine.cancel_execution(&execution_id) {
            Ok(()) => ok_bool(true, None),
            Err(e) => ok_bool(false, Some(e)),
        })
    }

    #[tool(description = "List all executions with their status.")]
    pub async fn list_executions(&self) -> Result<CallToolResult, McpError> {
        Ok(match self.core.engine.list_executions() {
            Ok(executions) => ok_text(json!({ "executions": executions })),
            Err(e) => ok_text(json!({ "error": e })),
        })
    }

    #[tool(description = "List all named sessions (stateful mode only). Returns an array of session names that have been used with the session parameter in run_js.")]
    pub async fn list_sessions(&self) -> Result<CallToolResult, McpError> {
        Ok(match self.core.engine.list_sessions().await {
            Ok(sessions) => ok_text(json!({ "sessions": sessions })),
            Err(e) => ok_text(json!({ "sessions": [format!("Error: {}", e)] })),
        })
    }

    #[tool(description = "List all log entries for the current session (stateful mode only). Each entry contains the input heap hash, output heap hash, code executed, and timestamp. Use the fields parameter to select specific fields (comma-separated: index,input_heap,output_heap,code,timestamp).")]
    pub async fn list_session_snapshots(
        &self,
        Parameters(args): Parameters<ListSessionSnapshotsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let session = match self.core.session_id.get() {
            Some(id) => id.clone(),
            None => return Ok(ok_text(json!({
                "entries": [{"error": "no session ID available (send X-MCP-Session-Id header)"}],
            }))),
        };
        let parsed_fields = args.fields.map(|f| {
            f.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
        });
        Ok(match self.core.engine.list_session_snapshots(session, parsed_fields).await {
            Ok(entries) => ok_text(json!({ "entries": entries })),
            Err(e) => ok_text(json!({ "entries": [{"error": e}] })),
        })
    }

    #[tool(description = "Get tags for a heap snapshot (stateful mode only). Returns a map of key-value tags associated with the given heap content hash.")]
    pub async fn get_heap_tags(
        &self,
        Parameters(HeapArgs { heap }): Parameters<HeapArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(match self.core.engine.get_heap_tags(heap).await {
            Ok(tags) => ok_text(json!({ "tags": tags })),
            Err(e) => {
                let mut m = HashMap::new();
                m.insert("error".to_string(), e);
                ok_text(json!({ "tags": m }))
            },
        })
    }

    #[tool(description = "Set or replace tags on a heap snapshot (stateful mode only). Provide a map of key-value string pairs. This replaces all existing tags for the heap.")]
    pub async fn set_heap_tags(
        &self,
        Parameters(SetHeapTagsArgs { heap, tags }): Parameters<SetHeapTagsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(match self.core.engine.set_heap_tags(heap, tags).await {
            Ok(()) => ok_bool(true, None),
            Err(e) => ok_bool(false, Some(e)),
        })
    }

    #[tool(description = "Delete tags from a heap snapshot (stateful mode only). If keys is provided (comma-separated), only those tag keys are removed. If keys is omitted, all tags are deleted.")]
    pub async fn delete_heap_tags(
        &self,
        Parameters(args): Parameters<DeleteHeapTagsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let parsed_keys = args.keys.map(|k| {
            k.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
        });
        Ok(match self.core.engine.delete_heap_tags(args.heap, parsed_keys).await {
            Ok(()) => ok_bool(true, None),
            Err(e) => ok_bool(false, Some(e)),
        })
    }

    #[tool(description = "Query heap snapshots by tags (stateful mode only). Provide a map of key-value pairs to match. Returns all heaps whose tags contain all the specified key-value pairs.")]
    pub async fn query_heaps_by_tags(
        &self,
        Parameters(QueryHeapTagsArgs { tags }): Parameters<QueryHeapTagsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(match self.core.engine.query_heaps_by_tags(tags).await {
            Ok(results) => {
                let entries: Vec<serde_json::Value> = results.into_iter().map(|e: HeapTagEntry| {
                    json!({ "heap": e.heap, "tags": e.tags })
                }).collect();
                ok_text(json!({ "results": entries }))
            },
            Err(e) => {
                let mut m = HashMap::new();
                m.insert("error".to_string(), e);
                ok_text(json!({
                    "results": [{ "heap": "error", "tags": m }],
                }))
            },
        })
    }
}

#[tool_handler]
impl ServerHandler for McpService {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.instructions = Some(
            "JavaScript execution service (stateful mode - with heap persistence). \
             Use resources/list and resources/read to explore docs://readme, \
             docs://llms-txt, docs://openapi, and docs://tools before calling tools."
                .to_string(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            meta: None,
            next_cursor: None,
            resources: doc_resources(true, false),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        read_doc_resource(&request.uri, true, false)
            .ok_or_else(|| McpError::resource_not_found(
                format!("Unknown resource URI: {}", request.uri),
                None,
            ))
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut base = self.tool_router.list_all();
        append_stub_tools(&self.core.mcp_client, &mut base);
        let _ = (request, context);
        Ok(ListToolsResult { meta: None, next_cursor: None, tools: base })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(result) = try_stub_call(&self.core.mcp_client, &request.name, request.arguments.as_ref()) {
            return Ok(result);
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        self.core.handle_initialize(&context).await;
        Ok(self.get_info())
    }
}

// ───────────────────────────────────────────────────────────────────────
// 2. StatelessMcpService — stateless, legacy execution_id model
// ───────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct StatelessMcpService {
    core: McpCore,
    tool_router: ToolRouter<Self>,
}

impl StatelessMcpService {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        Self {
            core: McpCore::new(engine, verifier),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl StatelessMcpService {
    #[doc = include_str!("run_js_tool_stateless.md")]
    #[tool]
    pub async fn run_js(
        &self,
        Parameters(args): Parameters<StatelessRunJsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let exec_id = match self.core.start_run_js_stateless(args).await {
            Ok(id) => id,
            Err(e) => return Ok(ok_text(json!({ "error": e }))),
        };
        let info = match self.core.wait_for_terminal(&exec_id).await {
            Ok(info) => info,
            Err(e) => return Ok(ok_text(json!({ "error": e }))),
        };
        Ok(self.core.build_stateless_run_result(&exec_id, &info))
    }
}

#[tool_handler]
impl ServerHandler for StatelessMcpService {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.instructions = Some(
            "JavaScript execution service (stateless mode — no heap persistence). \
             Use resources/list and resources/read to explore docs://readme, \
             docs://llms-txt, docs://openapi, and docs://tools before calling tools."
                .to_string(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            meta: None,
            next_cursor: None,
            resources: doc_resources(false, false),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        read_doc_resource(&request.uri, false, false)
            .ok_or_else(|| McpError::resource_not_found(
                format!("Unknown resource URI: {}", request.uri),
                None,
            ))
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut base = self.tool_router.list_all();
        append_stub_tools(&self.core.mcp_client, &mut base);
        let _ = (request, context);
        Ok(ListToolsResult { meta: None, next_cursor: None, tools: base })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(result) = try_stub_call(&self.core.mcp_client, &request.name, request.arguments.as_ref()) {
            return Ok(result);
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        self.core.handle_initialize(&context).await;
        Ok(self.get_info())
    }
}

// ───────────────────────────────────────────────────────────────────────
// 3. TasksApiMcpService — stateful, MCP 2025-11-25 Tasks API
// ───────────────────────────────────────────────────────────────────────

const TASKS_LIST_PAGE_SIZE: usize = 50;
const RUN_JS_TOOL_NAME: &str = "run_js";

#[derive(Clone)]
pub struct TasksApiMcpService {
    core: McpCore,
    tool_router: ToolRouter<Self>,
}

impl TasksApiMcpService {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        Self {
            core: McpCore::new(engine, verifier),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl TasksApiMcpService {
    /// `run_js` is the only tool exposed in Tasks-API mode. The handler
    /// returns an error to clients that invoke `tools/call` without
    /// task augmentation (`_meta.task` present); task-augmented calls
    /// reach this method via `enqueue_task` instead.
    #[doc = include_str!("run_js_tool_description.md")]
    #[tool]
    pub async fn run_js(
        &self,
        Parameters(_args): Parameters<RunJsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Err(McpError::invalid_params(
            "run_js requires task augmentation (_meta.task) when --use-tasks-api is set; \
             call this tool with _meta.task populated and poll tasks/get + tasks/result.",
            None,
        ))
    }
}

#[tool_handler]
impl ServerHandler for TasksApiMcpService {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_tasks()
            .build();
        info.instructions = Some(
            "JavaScript execution service (stateful, Tasks API). \
             `run_js` is task-augmented (execution.taskSupport = required). \
             Submit code with tools/call + _meta.task; poll tasks/get and tasks/result."
                .to_string(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            meta: None,
            next_cursor: None,
            resources: doc_resources(true, true),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        read_doc_resource(&request.uri, true, true)
            .ok_or_else(|| McpError::resource_not_found(
                format!("Unknown resource URI: {}", request.uri),
                None,
            ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut base = self.tool_router.list_all();
        mark_tool_task_required(&mut base, RUN_JS_TOOL_NAME);
        append_stub_tools(&self.core.mcp_client, &mut base);
        Ok(ListToolsResult { meta: None, next_cursor: None, tools: base })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(result) = try_stub_call(&self.core.mcp_client, &request.name, request.arguments.as_ref()) {
            return Ok(result);
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        self.core.handle_initialize(&context).await;
        Ok(self.get_info())
    }

    async fn enqueue_task(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CreateTaskResult, McpError> {
        enqueue_task_run_js_stateful(&self.core, request).await
    }

    async fn list_tasks(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListTasksResult, McpError> {
        Ok(list_tasks_impl(&self.core, request))
    }

    async fn get_task_info(
        &self,
        request: GetTaskInfoParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, McpError> {
        get_task_info_impl(&self.core, request)
    }

    async fn get_task_result(
        &self,
        request: GetTaskResultParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskPayloadResult, McpError> {
        get_task_result_stateful_impl(&self.core, request).await
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CancelTaskResult, McpError> {
        cancel_task_impl(&self.core, request)
    }
}

// ───────────────────────────────────────────────────────────────────────
// 4. StatelessTasksApiMcpService — stateless, MCP 2025-11-25 Tasks API
// ───────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct StatelessTasksApiMcpService {
    core: McpCore,
    tool_router: ToolRouter<Self>,
}

impl StatelessTasksApiMcpService {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        Self {
            core: McpCore::new(engine, verifier),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl StatelessTasksApiMcpService {
    #[doc = include_str!("run_js_tool_stateless.md")]
    #[tool]
    pub async fn run_js(
        &self,
        Parameters(_args): Parameters<StatelessRunJsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Err(McpError::invalid_params(
            "run_js requires task augmentation (_meta.task) when --use-tasks-api is set; \
             call this tool with _meta.task populated and poll tasks/get + tasks/result.",
            None,
        ))
    }
}

#[tool_handler]
impl ServerHandler for StatelessTasksApiMcpService {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_tasks()
            .build();
        info.instructions = Some(
            "JavaScript execution service (stateless, Tasks API). \
             `run_js` is task-augmented (execution.taskSupport = required). \
             Submit code with tools/call + _meta.task; poll tasks/get and tasks/result."
                .to_string(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            meta: None,
            next_cursor: None,
            resources: doc_resources(false, true),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        read_doc_resource(&request.uri, false, true)
            .ok_or_else(|| McpError::resource_not_found(
                format!("Unknown resource URI: {}", request.uri),
                None,
            ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut base = self.tool_router.list_all();
        mark_tool_task_required(&mut base, RUN_JS_TOOL_NAME);
        append_stub_tools(&self.core.mcp_client, &mut base);
        Ok(ListToolsResult { meta: None, next_cursor: None, tools: base })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(result) = try_stub_call(&self.core.mcp_client, &request.name, request.arguments.as_ref()) {
            return Ok(result);
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        self.core.handle_initialize(&context).await;
        Ok(self.get_info())
    }

    async fn enqueue_task(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CreateTaskResult, McpError> {
        enqueue_task_run_js_stateless(&self.core, request).await
    }

    async fn list_tasks(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListTasksResult, McpError> {
        Ok(list_tasks_impl(&self.core, request))
    }

    async fn get_task_info(
        &self,
        request: GetTaskInfoParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, McpError> {
        get_task_info_impl(&self.core, request)
    }

    async fn get_task_result(
        &self,
        request: GetTaskResultParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskPayloadResult, McpError> {
        get_task_result_stateless_impl(&self.core, request).await
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CancelTaskResult, McpError> {
        cancel_task_impl(&self.core, request)
    }
}

// ── Tasks API helpers ───────────────────────────────────────────────────

/// Annotate the tool whose name matches `name` with
/// `execution.taskSupport = "required"` so spec-aware clients know they
/// must task-augment the call.
fn mark_tool_task_required(tools: &mut [Tool], name: &str) {
    for tool in tools.iter_mut() {
        if tool.name.as_ref() == name {
            tool.execution = Some(
                ToolExecution::new().with_task_support(TaskSupport::Required),
            );
        }
    }
}

/// Common pre-flight for `enqueue_task`: only `run_js` may be task-augmented.
/// Returns the (taskId, parsed args) on success.
fn validate_run_js_enqueue<T: for<'de> Deserialize<'de>>(
    request: &CallToolRequestParams,
) -> Result<T, McpError> {
    if request.name.as_ref() != RUN_JS_TOOL_NAME {
        return Err(McpError::method_not_found::<CallToolRequestMethod>());
    }
    let args = request.arguments.clone().unwrap_or_default();
    serde_json::from_value::<T>(serde_json::Value::Object(args))
        .map_err(|e| McpError::invalid_params(format!("invalid run_js args: {}", e), None))
}

async fn enqueue_task_run_js_stateful(
    core: &McpCore,
    request: CallToolRequestParams,
) -> Result<CreateTaskResult, McpError> {
    let args: RunJsArgs = validate_run_js_enqueue(&request)?;
    let ttl = extract_task_ttl(&request);
    match core.start_run_js_stateful(args).await {
        Ok(execution_id) => {
            let now = chrono::Utc::now().to_rfc3339();
            let mut task = Task::new(
                execution_id,
                TaskStatus::Working,
                now.clone(),
                now,
            );
            if let Some(t) = ttl { task = task.with_ttl(t); }
            task = task.with_poll_interval(500);
            Ok(CreateTaskResult::new(task))
        }
        Err(e) => Err(McpError::internal_error(format!("run_js submission failed: {}", e), None)),
    }
}

async fn enqueue_task_run_js_stateless(
    core: &McpCore,
    request: CallToolRequestParams,
) -> Result<CreateTaskResult, McpError> {
    let args: StatelessRunJsArgs = validate_run_js_enqueue(&request)?;
    let ttl = extract_task_ttl(&request);
    match core.start_run_js_stateless(args).await {
        Ok(execution_id) => {
            let now = chrono::Utc::now().to_rfc3339();
            let mut task = Task::new(
                execution_id,
                TaskStatus::Working,
                now.clone(),
                now,
            );
            if let Some(t) = ttl { task = task.with_ttl(t); }
            task = task.with_poll_interval(500);
            Ok(CreateTaskResult::new(task))
        }
        Err(e) => Err(McpError::internal_error(format!("run_js submission failed: {}", e), None)),
    }
}

/// Extract `_meta.task.ttl` (milliseconds) from a request, if present.
fn extract_task_ttl(request: &CallToolRequestParams) -> Option<u64> {
    let meta = request.meta.as_ref()?;
    let value = serde_json::to_value(meta).ok()?;
    value.get("task")?.get("ttl")?.as_u64()
}

fn list_tasks_impl(core: &McpCore, request: Option<PaginatedRequestParams>) -> ListTasksResult {
    let cursor = request.as_ref().and_then(|r| r.cursor.as_deref());
    let session_id = core.session_id.get().map(|s| s.as_str());
    let (page, next) = match core.engine.list_executions_for_session(session_id, cursor, TASKS_LIST_PAGE_SIZE) {
        Ok(v) => v,
        Err(_) => (Vec::new(), None),
    };
    let tasks: Vec<Task> = page.into_iter().map(|d| {
        let last_updated_at = d.completed_at.clone().unwrap_or_else(|| d.started_at.clone());
        let mut t = Task::new(
            d.id,
            McpCore::map_status(&d.status),
            d.started_at,
            last_updated_at,
        );
        t = t.with_poll_interval(500);
        if let Some(err) = d.error { t = t.with_status_message(err); }
        t
    }).collect();
    let mut result = ListTasksResult::new(tasks);
    result.next_cursor = next;
    result
}

fn get_task_info_impl(
    core: &McpCore,
    request: GetTaskInfoParams,
) -> Result<GetTaskResult, McpError> {
    let info = core.engine.get_execution(&request.task_id)
        .map_err(|_| McpError::invalid_params(format!("task '{}' not found", request.task_id), None))?;
    Ok(GetTaskResult {
        meta: None,
        task: McpCore::make_task(&info, None),
    })
}

async fn get_task_result_stateful_impl(
    core: &McpCore,
    request: GetTaskResultParams,
) -> Result<GetTaskPayloadResult, McpError> {
    let info = core.engine.get_execution(&request.task_id)
        .map_err(|_| McpError::invalid_params(format!("task '{}' not found", request.task_id), None))?;
    let info = if info.status == "running" {
        core.wait_for_terminal(&request.task_id).await
            .map_err(|e| McpError::internal_error(e, None))?
    } else {
        info
    };
    let payload = core.build_stateful_run_result(&info);
    let value = serde_json::to_value(payload)
        .map_err(|e| McpError::internal_error(format!("payload serialize: {}", e), None))?;
    Ok(GetTaskPayloadResult::new(value))
}

async fn get_task_result_stateless_impl(
    core: &McpCore,
    request: GetTaskResultParams,
) -> Result<GetTaskPayloadResult, McpError> {
    let info = core.engine.get_execution(&request.task_id)
        .map_err(|_| McpError::invalid_params(format!("task '{}' not found", request.task_id), None))?;
    let info = if info.status == "running" {
        core.wait_for_terminal(&request.task_id).await
            .map_err(|e| McpError::internal_error(e, None))?
    } else {
        info
    };
    let payload = core.build_stateless_run_result(&request.task_id, &info);
    let value = serde_json::to_value(payload)
        .map_err(|e| McpError::internal_error(format!("payload serialize: {}", e), None))?;
    Ok(GetTaskPayloadResult::new(value))
}

fn cancel_task_impl(
    core: &McpCore,
    request: CancelTaskParams,
) -> Result<CancelTaskResult, McpError> {
    let info = core.engine.get_execution(&request.task_id)
        .map_err(|_| McpError::invalid_params(format!("task '{}' not found", request.task_id), None))?;
    if info.status != "running" {
        return Err(McpError::invalid_params(
            format!("task '{}' is not running (status: {})", request.task_id, info.status),
            None,
        ));
    }
    let _ = core.engine.cancel_execution(&request.task_id);
    let info = core.engine.get_execution(&request.task_id)
        .map_err(|e| McpError::internal_error(e, None))?;
    Ok(CancelTaskResult {
        meta: None,
        task: McpCore::make_task(&info, None),
    })
}
