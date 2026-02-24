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
    pub output: String,
    pub heap: Option<String>,
}

impl IntoContents for RunJsResponse {
    fn into_contents(self) -> Vec<Content> {
        let value = match self.heap {
            Some(h) => json!({ "output": self.output, "heap": h }),
            None => json!({ "output": self.output }),
        };
        match Content::json(value) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert run_js response to content: {}", e))],
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
            Ok(result) => RunJsResponse {
                output: result.output,
                heap: result.heap,
            },
            Err(e) => RunJsResponse {
                output: format!("V8 error: {}", e),
                heap: None,
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
// Stateless mode: only exposes `run_js` with no heap/session parameters.
// This prevents agents from attempting to use stateful features that
// don't exist in this mode.

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
            Ok(result) => RunJsResponse {
                output: result.output,
                heap: None,
            },
            Err(e) => RunJsResponse {
                output: format!("V8 error: {}", e),
                heap: None,
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
