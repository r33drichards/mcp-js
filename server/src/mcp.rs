use rmcp::{
    model::{ServerCapabilities, ServerInfo},
    Error as McpError, RoleServer, ServerHandler, model::*,
    service::RequestContext, tool,
};
use serde_json::json;

use crate::engine::Engine;

// ── MCP response types ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RunJsResponse {
    pub output: String,
    pub heap: Option<String>,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

impl IntoContents for RunJsResponse {
    fn into_contents(self) -> Vec<Content> {
        let mut value = serde_json::Map::new();
        value.insert("output".to_string(), json!(self.output));
        if let Some(h) = self.heap {
            value.insert("heap".to_string(), json!(h));
        }
        if !self.stdout.is_empty() {
            value.insert("stdout".to_string(), json!(self.stdout));
        }
        if !self.stderr.is_empty() {
            value.insert("stderr".to_string(), json!(self.stderr));
        }
        match Content::json(serde_json::Value::Object(value)) {
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
        stdin: Option<String>,
    ) -> RunJsResponse {
        match self.engine.run_js(code, heap, session, heap_memory_max_mb, execution_timeout_secs, stdin).await {
            Ok(result) => RunJsResponse {
                output: result.output,
                heap: result.heap,
                stdout: result.stdout,
                stderr: result.stderr,
            },
            Err(e) => RunJsResponse {
                output: format!("V8 error: {}", e),
                heap: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
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
        #[tool(param)]
        #[serde(default)]
        stdin: Option<String>,
    ) -> RunJsResponse {
        match self.engine.run_js(code, None, None, heap_memory_max_mb, execution_timeout_secs, stdin).await {
            Ok(result) => RunJsResponse {
                output: result.output,
                heap: None,
                stdout: result.stdout,
                stderr: result.stderr,
            },
            Err(e) => RunJsResponse {
                output: format!("V8 error: {}", e),
                heap: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
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
