use rmcp::{
    model::{ServerCapabilities, ServerInfo},
    Error as McpError, RoleServer, ServerHandler, model::*,
    service::RequestContext, tool,
};
use serde_json::json;
use std::collections::HashMap;

use crate::engine::Engine;
use crate::engine::heap_tags::HeapTagEntry;

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

// ── McpService ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct McpService {
    engine: Engine,
}

impl McpService {
    pub fn new(engine: Engine) -> Self {
        Self { engine }
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
        session: Option<String>,
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
        match self.engine.run_js(code, heap, session, heap_memory_max_mb, execution_timeout_secs, tags).await {
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

    #[tool(description = "List all named sessions (stateful mode only). Returns an array of session names that have been used with the session parameter in run_js.")]
    pub async fn list_sessions(&self) -> ListSessionsResponse {
        match self.engine.list_sessions().await {
            Ok(sessions) => ListSessionsResponse { sessions },
            Err(e) => ListSessionsResponse {
                sessions: vec![format!("Error: {}", e)],
            },
        }
    }

    #[tool(description = "List all log entries for a named session (stateful mode only). Each entry contains the input heap hash, output heap hash, code executed, and timestamp. Use the fields parameter to select specific fields (comma-separated: index,input_heap,output_heap,code,timestamp).")]
    pub async fn list_session_snapshots(
        &self,
        #[tool(param)] session: String,
        #[tool(param)]
        #[serde(default)]
        fields: Option<String>,
    ) -> ListSessionSnapshotsResponse {
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

#[tool(tool_box)]
impl ServerHandler for McpService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("JavaScript execution service (stateful mode - with heap persistence)".to_string()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
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
        }
        Ok(self.get_info())
    }
}

// ── StatelessMcpService ─────────────────────────────────────────────────
//
// Stateless mode: exposes `run_js` plus execution query/cancel tools,
// but no heap/session/tag parameters.

#[derive(Clone)]
pub struct StatelessMcpService {
    engine: Engine,
}

impl StatelessMcpService {
    pub fn new(engine: Engine) -> Self {
        Self { engine }
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
    ) -> RunJsResponse {
        match self.engine.run_js(code, None, None, heap_memory_max_mb, execution_timeout_secs, None).await {
            Ok(execution_id) => RunJsResponse { execution_id },
            Err(e) => RunJsResponse {
                execution_id: format!("error: {}", e),
            },
        }
    }

    #[tool(description = "Get the status and result of an execution. Returns execution_id, status (running/completed/failed/cancelled/timed_out), result (if completed), error (if failed), started_at, and completed_at.")]
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

    #[tool(description = "Get paginated console output for an execution. Supports two modes: line-based (line_offset + line_limit) or byte-based (byte_offset + byte_limit). If byte_offset is provided, byte mode takes precedence. Response includes both line and byte coordinates for cross-referencing.")]
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
}

#[tool(tool_box)]
impl ServerHandler for StatelessMcpService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("JavaScript execution service (stateless mode)".to_string()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
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
        }
        Ok(self.get_info())
    }
}
