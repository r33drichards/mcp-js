use rmcp::{
    model::{ServerCapabilities, ServerInfo},
    Error as McpError, RoleServer, ServerHandler, model::*,
    service::RequestContext, tool,
};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::engine::Engine;
use crate::engine::heap_tags::HeapTagEntry;
use crate::engine::mcp_client::McpClientManager;
use crate::session::SessionVerifier;

// ── Embedded documentation resources ───────────────────────────────────

/// llms.txt content — machine-readable agent guide (https://llmstxt.org/)
const LLMS_TXT: &str = include_str!("llms_txt.md");

/// Full README
const README_MD: &str = include_str!("../README.md");

#[derive(Debug, Clone, Serialize)]
pub struct ToolDoc {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

impl ToolDoc {
    fn from_tool(tool: Tool) -> Self {
        Self {
            name: tool.name.to_string(),
            description: tool.description.as_ref().map(|value| value.to_string()),
            input_schema: serde_json::Value::Object(tool.input_schema.as_ref().clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCatalog {
    pub mode: &'static str,
    pub tools: Vec<ToolDoc>,
}

fn built_in_tools(stateful: bool) -> Vec<Tool> {
    if stateful {
        McpService::tool_box().list()
    } else {
        StatelessMcpService::tool_box().list()
    }
}

pub fn built_in_tool_catalog(stateful: bool) -> ToolCatalog {
    ToolCatalog {
        mode: if stateful { "stateful" } else { "stateless" },
        tools: built_in_tools(stateful)
            .into_iter()
            .map(ToolDoc::from_tool)
            .collect(),
    }
}

/// Build the list of static documentation resources exposed via MCP.
fn doc_resources(stateful: bool) -> Vec<Resource> {
    use crate::api::ApiDoc;
    use utoipa::OpenApi as _;

    let openapi_json = serde_json::to_string(&ApiDoc::openapi()).unwrap_or_default();
    let tools_json = serde_json::to_string(&built_in_tool_catalog(stateful)).unwrap_or_default();

    // Store the generated JSON in thread-local statics to extend the lifetime
    // to 'static so we can use them in ResourceContents::text().
    // We use once_cell-style initialisation via OnceLock boxes on the heap.
    // Actually, we use owned Strings, not &'static str, so we return the
    // ResourceContents by value — that's fine.
    let _ = (openapi_json, tools_json); // suppress unused warning before use below

    vec![
        Annotated::new(
            RawResource {
                uri: "docs://readme".into(),
                name: "README".into(),
                description: Some("Full mcp-v8 README with usage, CLI flags, and examples (Markdown)".into()),
                mime_type: Some("text/markdown".into()),
                size: Some(README_MD.len() as u32),
            },
            None,
        ),
        Annotated::new(
            RawResource {
                uri: "docs://llms-txt".into(),
                name: "llms.txt".into(),
                description: Some("Machine-readable agent guide: connection options, tools, REST API (Markdown)".into()),
                mime_type: Some("text/markdown".into()),
                size: Some(LLMS_TXT.len() as u32),
            },
            None,
        ),
        Annotated::new(
            RawResource {
                uri: "docs://openapi".into(),
                name: "OpenAPI spec".into(),
                description: Some("OpenAPI 3.0 JSON spec for the REST API (/api/exec, /api/executions/*, etc.)".into()),
                mime_type: Some("application/json".into()),
                size: None,
            },
            None,
        ),
        Annotated::new(
            RawResource {
                uri: "docs://tools".into(),
                name: "MCP tool list".into(),
                description: Some("JSON list of available MCP tools with descriptions, mode-aware".into()),
                mime_type: Some("application/json".into()),
                size: None,
            },
            None,
        ),
    ]
}

/// Read a single documentation resource by URI.
/// Returns `None` when the URI is not recognised.
fn read_doc_resource(uri: &str, stateful: bool) -> Option<ReadResourceResult> {
    use crate::api::ApiDoc;
    use utoipa::OpenApi as _;

    let openapi_json = serde_json::to_string_pretty(&ApiDoc::openapi()).unwrap_or_default();
    let tools_json = serde_json::to_string_pretty(&built_in_tool_catalog(stateful)).unwrap_or_default();

    let contents = match uri {
        "docs://readme" => vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("text/markdown".into()),
            text: README_MD.to_string(),
        }],
        "docs://llms-txt" => vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("text/markdown".into()),
            text: LLMS_TXT.to_string(),
        }],
        "docs://openapi" => vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("application/json".into()),
            text: openapi_json,
        }],
        "docs://tools" => vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("application/json".into()),
            text: tools_json,
        }],
        _ => return None,
    };

    Some(ReadResourceResult { contents })
}

// ── MCP response types ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RunJsResponse {
    pub execution_id: String,
}

impl IntoContents for RunJsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({ "execution_id": self.execution_id })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert run_js response: {}", e))],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionStatusResponse {
    pub value: serde_json::Value,
}

impl IntoContents for ExecutionStatusResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(self.value) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert execution status response: {}", e))],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConsoleOutputResponse {
    pub value: serde_json::Value,
}

impl IntoContents for ConsoleOutputResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(self.value) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert console output response: {}", e))],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListExecutionsResponse {
    pub value: serde_json::Value,
}

impl IntoContents for ListExecutionsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(self.value) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert list executions response: {}", e))],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListSessionsResponse {
    pub sessions: Vec<String>,
}

impl IntoContents for ListSessionsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({ "sessions": self.sessions })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert list_sessions response: {}", e))],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListSessionSnapshotsResponse {
    pub entries: Vec<serde_json::Value>,
}

impl IntoContents for ListSessionSnapshotsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({ "entries": self.entries })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert list_session_snapshots response: {}", e))],
        }
    }
}

#[derive(Debug, Clone)]
pub struct GetHeapTagsResponse {
    pub tags: HashMap<String, String>,
}

impl IntoContents for GetHeapTagsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({ "tags": self.tags })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert get_heap_tags response: {}", e))],
        }
    }
}

#[derive(Debug, Clone)]
pub struct OkResponse {
    pub ok: bool,
    pub error: Option<String>,
}

impl IntoContents for OkResponse {
    fn into_contents(self) -> Vec<Content> {
        let value = match self.error {
            Some(e) => json!({ "ok": self.ok, "error": e }),
            None => json!({ "ok": self.ok }),
        };
        match Content::json(value) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert response: {}", e))],
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueryHeapTagsResponse {
    pub results: Vec<HeapTagEntry>,
}

impl IntoContents for QueryHeapTagsResponse {
    fn into_contents(self) -> Vec<Content> {
        let entries: Vec<serde_json::Value> = self.results.into_iter().map(|e| {
            json!({ "heap": e.heap, "tags": e.tags })
        }).collect();
        match Content::json(json!({ "results": entries })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert query_heaps_by_tags response: {}", e))],
        }
    }
}

/// Replace the `run_js` tool's description with an operator-provided override,
/// if one was configured via `--run-js-description`. Other tools are left
/// untouched.
fn apply_run_js_description_override(tools: &mut [Tool], override_desc: Option<Arc<str>>) {
    if let Some(desc) = override_desc {
        for tool in tools.iter_mut() {
            if tool.name.as_ref() == "run_js" {
                tool.description = Some(desc.to_string().into());
            }
        }
    }
}

// ── McpService ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct McpService {
    engine: Engine,
    verifier: Option<Arc<SessionVerifier>>,
    /// Optional manager for upstream MCP servers. When set, those servers'
    /// tools are exposed as stubs in this service's tool list, and calls to
    /// those stubs return run_js instructions instead of dispatching.
    mcp_client: Option<Arc<McpClientManager>>,
    /// Set once during `initialize` from X-MCP-Session-Id header.
    session_id: Arc<OnceLock<String>>,
    /// X-MCP-* headers from the initialize request, available for policy evaluation.
    mcp_headers: Arc<OnceLock<serde_json::Value>>,
}

impl McpService {
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
}

#[tool(tool_box)]
impl McpService {
    #[tool(description = include_str!("run_js_tool_description.md"))]
    pub async fn run_js(
        &self,
        #[tool(param)] code: String,
        #[tool(param)]
        #[serde(default)]
        heap: Option<String>,
        #[tool(param)]
        #[serde(default)]
        heap_memory_max_mb: Option<usize>,
        #[tool(param)]
        #[serde(default)]
        execution_timeout_secs: Option<u64>,
        #[tool(param)]
        #[serde(default)]
        tags: Option<HashMap<String, String>>,
    ) -> RunJsResponse {
        let mut req = self.engine.run_js(code);
        if let Some(h) = heap { req = req.heap(h); }
        if let Some(s) = self.session_id.get() { req = req.session(s.clone()); }
        if let Some(mb) = heap_memory_max_mb { req = req.heap_memory_max_mb(mb); }
        if let Some(secs) = execution_timeout_secs { req = req.execution_timeout_secs(secs); }
        if let Some(t) = tags { req = req.tags(t); }
        req = req.maybe_mcp_headers(self.mcp_headers.get().cloned());
        match req.execute().await {
            Ok(execution_id) => RunJsResponse { execution_id },
            Err(e) => RunJsResponse {
                execution_id: format!("error: {}", e),
            },
        }
    }

    #[tool(description = "Get the status and result of an execution. Returns execution_id, status (running/completed/failed/cancelled/timed_out), result (if completed), heap (if stateful), error (if failed), started_at, and completed_at.")]
    pub async fn get_execution(
        &self,
        #[tool(param)] execution_id: String,
    ) -> ExecutionStatusResponse {
        match self.engine.get_execution(&execution_id) {
            Ok(info) => ExecutionStatusResponse {
                value: json!({
                    "execution_id": info.id,
                    "status": info.status,
                    "result": info.result,
                    "heap": info.heap,
                    "error": info.error,
                    "started_at": info.started_at,
                    "completed_at": info.completed_at,
                }),
            },
            Err(e) => ExecutionStatusResponse {
                value: json!({ "error": e }),
            },
        }
    }

    #[tool(description = "Get paginated console output for an execution. Supports two modes: line-based (line_offset + line_limit) or byte-based (byte_offset + byte_limit). If byte_offset is provided, byte mode takes precedence. Response includes both line and byte coordinates for cross-referencing. Use next_line_offset or next_byte_offset from a previous response to resume reading.")]
    pub async fn get_execution_output(
        &self,
        #[tool(param)] execution_id: String,
        #[tool(param)]
        #[serde(default)]
        line_offset: Option<u64>,
        #[tool(param)]
        #[serde(default)]
        line_limit: Option<u64>,
        #[tool(param)]
        #[serde(default)]
        byte_offset: Option<u64>,
        #[tool(param)]
        #[serde(default)]
        byte_limit: Option<u64>,
    ) -> ConsoleOutputResponse {
        let status = self.engine.get_execution(&execution_id)
            .map(|info| info.status)
            .unwrap_or_else(|_| "unknown".to_string());

        match self.engine.get_execution_output(&execution_id, line_offset, line_limit, byte_offset, byte_limit) {
            Ok(page) => ConsoleOutputResponse {
                value: json!({
                    "execution_id": execution_id,
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
                }),
            },
            Err(e) => ConsoleOutputResponse {
                value: json!({ "error": e }),
            },
        }
    }

    #[tool(description = "Cancel a running execution. Terminates the V8 isolate.")]
    pub async fn cancel_execution(
        &self,
        #[tool(param)] execution_id: String,
    ) -> OkResponse {
        match self.engine.cancel_execution(&execution_id) {
            Ok(()) => OkResponse { ok: true, error: None },
            Err(e) => OkResponse { ok: false, error: Some(e) },
        }
    }

    #[tool(description = "List all executions with their status.")]
    pub async fn list_executions(&self) -> ListExecutionsResponse {
        match self.engine.list_executions() {
            Ok(executions) => ListExecutionsResponse {
                value: json!({ "executions": executions }),
            },
            Err(e) => ListExecutionsResponse {
                value: json!({ "error": e }),
            },
        }
    }

    #[tool(description = "List all named sessions (stateful mode only). Returns an array of session names that have been used via REST session fields or the X-MCP-Session-Id header.")]
    pub async fn list_sessions(&self) -> ListSessionsResponse {
        match self.engine.list_sessions().await {
            Ok(sessions) => ListSessionsResponse { sessions },
            Err(e) => ListSessionsResponse {
                sessions: vec![format!("Error: {}", e)],
            },
        }
    }

    #[tool(description = "List all log entries for the current session (stateful mode only). Each entry contains the input heap hash, output heap hash, code executed, and timestamp. Use the fields parameter to select specific fields (comma-separated: index,input_heap,output_heap,code,timestamp).")]
    pub async fn list_session_snapshots(
        &self,
        #[tool(param)]
        #[serde(default)]
        fields: Option<String>,
    ) -> ListSessionSnapshotsResponse {
        let session = match self.session_id.get() {
            Some(id) => id.clone(),
            None => return ListSessionSnapshotsResponse {
                entries: vec![serde_json::json!({"error": "no session ID available (send X-MCP-Session-Id header)"})],
            },
        };
        let parsed_fields = fields.map(|f| {
            f.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
        });
        match self.engine.list_session_snapshots(session, parsed_fields).await {
            Ok(entries) => ListSessionSnapshotsResponse { entries },
            Err(e) => ListSessionSnapshotsResponse {
                entries: vec![serde_json::json!({"error": e})],
            },
        }
    }

    #[tool(description = "Get tags for a heap snapshot (stateful mode only). Returns a map of key-value tags associated with the given heap content hash.")]
    pub async fn get_heap_tags(
        &self,
        #[tool(param)] heap: String,
    ) -> GetHeapTagsResponse {
        match self.engine.get_heap_tags(heap).await {
            Ok(tags) => GetHeapTagsResponse { tags },
            Err(e) => GetHeapTagsResponse {
                tags: {
                    let mut m = HashMap::new();
                    m.insert("error".to_string(), e);
                    m
                },
            },
        }
    }

    #[tool(description = "Set or replace tags on a heap snapshot (stateful mode only). Provide a map of key-value string pairs. This replaces all existing tags for the heap.")]
    pub async fn set_heap_tags(
        &self,
        #[tool(param)] heap: String,
        #[tool(param)] tags: HashMap<String, String>,
    ) -> OkResponse {
        match self.engine.set_heap_tags(heap, tags).await {
            Ok(()) => OkResponse { ok: true, error: None },
            Err(e) => OkResponse { ok: false, error: Some(e) },
        }
    }

    #[tool(description = "Delete tags from a heap snapshot (stateful mode only). If keys is provided (comma-separated), only those tag keys are removed. If keys is omitted, all tags are deleted.")]
    pub async fn delete_heap_tags(
        &self,
        #[tool(param)] heap: String,
        #[tool(param)]
        #[serde(default)]
        keys: Option<String>,
    ) -> OkResponse {
        let parsed_keys = keys.map(|k| {
            k.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
        });
        match self.engine.delete_heap_tags(heap, parsed_keys).await {
            Ok(()) => OkResponse { ok: true, error: None },
            Err(e) => OkResponse { ok: false, error: Some(e) },
        }
    }

    #[tool(description = "Query heap snapshots by tags (stateful mode only). Provide a map of key-value pairs to match. Returns all heaps whose tags contain all the specified key-value pairs.")]
    pub async fn query_heaps_by_tags(
        &self,
        #[tool(param)] tags: HashMap<String, String>,
    ) -> QueryHeapTagsResponse {
        match self.engine.query_heaps_by_tags(tags).await {
            Ok(results) => QueryHeapTagsResponse { results },
            Err(e) => QueryHeapTagsResponse {
                results: vec![HeapTagEntry {
                    heap: "error".to_string(),
                    tags: {
                        let mut m = HashMap::new();
                        m.insert("error".to_string(), e);
                        m
                    },
                }],
            },
        }
    }
}

impl ServerHandler for McpService {
    fn get_info(&self) -> ServerInfo {
        let instructions = self.engine.instructions_override()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "JavaScript execution service (stateful mode - with heap persistence). \
                 Use resources/list and resources/read to explore docs://readme, \
                 docs://llms-txt, docs://openapi, and docs://tools before calling tools."
                .to_string()
            });
        ServerInfo {
            instructions: Some(instructions),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            ..Default::default()
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            next_cursor: None,
            resources: doc_resources(true),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        read_doc_resource(&request.uri, true)
            .ok_or_else(|| McpError::resource_not_found(
                format!("Unknown resource URI: {}", request.uri),
                None,
            ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = Self::tool_box().list();
        apply_run_js_description_override(&mut tools, self.engine.run_js_description_override());
        if let Some(client) = &self.mcp_client {
            tools.extend(client.stub_tools());
        }
        Ok(ListToolsResult { next_cursor: None, tools })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(client) = &self.mcp_client {
            if let Some(result) = client.stub_call_response(&request.name, request.arguments.as_ref()) {
                return Ok(result);
            }
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        Self::tool_box().call(tcc).await
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() {
            let initialize_headers = &http_request_part.headers;
            let initialize_uri = &http_request_part.uri;
            tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");

            // JWT verification (if --jwks-url was provided)
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

            // Read X-MCP-* headers (always, regardless of JWT)
            let mut mcp_header_map = serde_json::Map::new();
            for (name, value) in initialize_headers.iter() {
                if let Some(key) = name.as_str().strip_prefix("x-mcp-") {
                    if let Ok(v) = value.to_str() {
                        mcp_header_map.insert(key.to_string(), serde_json::Value::String(v.to_string()));
                    }
                }
            }

            // Extract session_id from X-MCP-Session-Id header
            if let Some(serde_json::Value::String(sid)) = mcp_header_map.get("session-id") {
                tracing::info!(session_id = sid.as_str(), "Session ID from X-MCP-Session-Id header");
                let _ = self.session_id.set(sid.clone());
            }

            if !mcp_header_map.is_empty() {
                tracing::info!(?mcp_header_map, "X-MCP-* headers captured");
                let _ = self.mcp_headers.set(serde_json::Value::Object(mcp_header_map));
            }
        }
        Ok(self.get_info())
    }
}

// ── StatelessMcpService ─────────────────────────────────────────────────
//
// Stateless shell mode: single `run_js` tool that executes code and returns
// console output directly. No execution IDs are exposed to callers — session
// isolation is automatic.

#[derive(Debug, Clone)]
pub struct StatelessRunJsResponse {
    pub value: serde_json::Value,
}

impl IntoContents for StatelessRunJsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(self.value) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert response: {}", e))],
        }
    }
}

#[derive(Clone)]
pub struct StatelessMcpService {
    engine: Engine,
    verifier: Option<Arc<SessionVerifier>>,
    mcp_client: Option<Arc<McpClientManager>>,
    /// X-MCP-* headers from the initialize request, available for policy evaluation.
    mcp_headers: Arc<OnceLock<serde_json::Value>>,
}

impl StatelessMcpService {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        let mcp_client = engine.mcp_client_manager();
        Self {
            engine,
            verifier,
            mcp_client,
            mcp_headers: Arc::new(OnceLock::new()),
        }
    }
}

#[tool(tool_box)]
impl StatelessMcpService {
    #[tool(description = include_str!("run_js_tool_stateless.md"))]
    pub async fn run_js(
        &self,
        #[tool(param)] code: String,
        #[tool(param)]
        #[serde(default)]
        heap_memory_max_mb: Option<usize>,
        #[tool(param)]
        #[serde(default)]
        execution_timeout_secs: Option<u64>,
    ) -> StatelessRunJsResponse {
        // 1. Submit to engine (fire-and-forget internally)
        let mut req = self.engine.run_js(code);
        if let Some(mb) = heap_memory_max_mb { req = req.heap_memory_max_mb(mb); }
        if let Some(secs) = execution_timeout_secs { req = req.execution_timeout_secs(secs); }
        req = req.maybe_mcp_headers(self.mcp_headers.get().cloned());
        let exec_id = match req.execute().await {
            Ok(id) => id,
            Err(e) => return StatelessRunJsResponse {
                value: json!({ "error": e }),
            },
        };

        // 2. Poll until terminal state
        let poll_interval = tokio::time::Duration::from_millis(50);
        let max_polls = 6000; // 5 minutes at 50ms intervals
        let mut status = String::new();
        let mut error_msg: Option<String> = None;

        for _ in 0..max_polls {
            tokio::time::sleep(poll_interval).await;
            match self.engine.get_execution(&exec_id) {
                Ok(info) => {
                    match info.status.as_str() {
                        "completed" => { status = info.status; break; }
                        "failed" => { status = info.status; error_msg = info.error; break; }
                        "timed_out" => { status = info.status; error_msg = info.error; break; }
                        "cancelled" => { status = info.status; error_msg = info.error; break; }
                        _ => continue,
                    }
                }
                Err(_) => continue,
            }
        }

        if status.is_empty() {
            return StatelessRunJsResponse {
                value: json!({ "error": "Execution did not complete within polling timeout" }),
            };
        }

        // 3. Collect all console output
        let output = match self.engine.get_execution_output(&exec_id, None, Some(u64::MAX), None, None) {
            Ok(page) => page.data,
            Err(_) => String::new(),
        };

        // 4. Return console output (and error if execution failed)
        match status.as_str() {
            "completed" => StatelessRunJsResponse {
                value: json!({ "output": output }),
            },
            _ => StatelessRunJsResponse {
                value: json!({ "output": output, "error": error_msg }),
            },
        }
    }
}

impl ServerHandler for StatelessMcpService {
    fn get_info(&self) -> ServerInfo {
        let instructions = self.engine.instructions_override()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "JavaScript execution service (stateless mode — no heap persistence). \
                 Use resources/list and resources/read to explore docs://readme, \
                 docs://llms-txt, docs://openapi, and docs://tools before calling tools."
                .to_string()
            });
        ServerInfo {
            instructions: Some(instructions),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            ..Default::default()
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            next_cursor: None,
            resources: doc_resources(false),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        read_doc_resource(&request.uri, false)
            .ok_or_else(|| McpError::resource_not_found(
                format!("Unknown resource URI: {}", request.uri),
                None,
            ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = Self::tool_box().list();
        apply_run_js_description_override(&mut tools, self.engine.run_js_description_override());
        if let Some(client) = &self.mcp_client {
            tools.extend(client.stub_tools());
        }
        Ok(ListToolsResult { next_cursor: None, tools })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(client) = &self.mcp_client {
            if let Some(result) = client.stub_call_response(&request.name, request.arguments.as_ref()) {
                return Ok(result);
            }
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        Self::tool_box().call(tcc).await
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() {
            let initialize_headers = &http_request_part.headers;
            let initialize_uri = &http_request_part.uri;
            tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");

            // JWT verification (if --jwks-url was provided)
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
                        tracing::info!("JWT verified in stateless mode");
                    } else {
                        tracing::warn!("JWT present but failed verification");
                    },
                    None => {
                        tracing::debug!("No Authorization/AgentSession header in initialize request");
                    }
                }
            }

            // Read X-MCP-* headers (always, regardless of JWT)
            let mut mcp_header_map = serde_json::Map::new();
            for (name, value) in initialize_headers.iter() {
                if let Some(key) = name.as_str().strip_prefix("x-mcp-") {
                    if let Ok(v) = value.to_str() {
                        mcp_header_map.insert(key.to_string(), serde_json::Value::String(v.to_string()));
                    }
                }
            }

            if !mcp_header_map.is_empty() {
                tracing::info!(?mcp_header_map, "X-MCP-* headers captured");
                let _ = self.mcp_headers.set(serde_json::Value::Object(mcp_header_map));
            }
        }
        Ok(self.get_info())
    }
}

#[cfg(test)]
mod tests {
    use super::apply_run_js_description_override;
    use rmcp::model::Tool;
    use std::sync::Arc;

    fn tool(name: &'static str, desc: &'static str) -> Tool {
        Tool {
            name: name.into(),
            description: Some(desc.into()),
            input_schema: Arc::new(serde_json::Map::new()),
            annotations: None,
        }
    }

    #[test]
    fn override_replaces_only_run_js_description() {
        let mut tools = vec![tool("run_js", "original"), tool("get_execution", "other")];
        apply_run_js_description_override(&mut tools, Some(Arc::from("custom description")));

        assert_eq!(tools[0].description.as_deref(), Some("custom description"));
        // Non-run_js tools are untouched.
        assert_eq!(tools[1].description.as_deref(), Some("other"));
    }

    #[test]
    fn no_override_leaves_descriptions_unchanged() {
        let mut tools = vec![tool("run_js", "original")];
        apply_run_js_description_override(&mut tools, None);
        assert_eq!(tools[0].description.as_deref(), Some("original"));
    }
}
