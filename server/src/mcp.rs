use rmcp::{
    model::{ServerCapabilities, ServerInfo},
    Error as McpError, RoleServer, ServerHandler, model::*,
    service::RequestContext, tool,
};
use serde_json::json;

use crate::engine::Engine;
use crate::engine::{PipelineStage, PipelineResult};

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

#[derive(Debug, Clone)]
pub struct RunPipelineResponse {
    pub result: Option<PipelineResult>,
    pub error: Option<String>,
}

impl IntoContents for RunPipelineResponse {
    fn into_contents(self) -> Vec<Content> {
        if let Some(err) = self.error {
            let mut value = serde_json::Map::new();
            value.insert("error".to_string(), json!(err));
            match Content::json(serde_json::Value::Object(value)) {
                Ok(content) => vec![content],
                Err(_) => vec![Content::text(format!("Pipeline error: {}", err))],
            }
        } else if let Some(result) = self.result {
            match serde_json::to_value(&result) {
                Ok(val) => match Content::json(val) {
                    Ok(content) => vec![content],
                    Err(e) => vec![Content::text(format!("Failed to convert pipeline response: {}", e))],
                },
                Err(e) => vec![Content::text(format!("Failed to serialize pipeline response: {}", e))],
            }
        } else {
            vec![Content::text("Empty pipeline response")]
        }
    }
}

#[derive(Debug, Clone)]
pub struct GetBufferResponse {
    pub content: Option<String>,
    pub error: Option<String>,
}

impl IntoContents for GetBufferResponse {
    fn into_contents(self) -> Vec<Content> {
        if let Some(err) = self.error {
            match Content::json(json!({"error": err})) {
                Ok(content) => vec![content],
                Err(_) => vec![Content::text(format!("Buffer error: {}", err))],
            }
        } else if let Some(content) = self.content {
            vec![Content::text(content)]
        } else {
            vec![Content::text("")]
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

    #[tool(description = include_str!("run_pipeline_tool_description.md"))]
    pub async fn run_pipeline(
        &self,
        #[tool(param)] stages: Vec<serde_json::Value>,
        #[tool(param)]
        #[serde(default)]
        session: Option<String>,
    ) -> RunPipelineResponse {
        let parsed: Result<Vec<PipelineStage>, _> = stages
            .into_iter()
            .enumerate()
            .map(|(i, v)| serde_json::from_value(v).map_err(|e| format!("Invalid stage {}: {}", i, e)))
            .collect();
        match parsed {
            Ok(pipeline_stages) => match self.engine.run_pipeline(pipeline_stages, session).await {
                Ok(result) => RunPipelineResponse { result: Some(result), error: None },
                Err(e) => RunPipelineResponse { result: None, error: Some(e) },
            },
            Err(e) => RunPipelineResponse { result: None, error: Some(e) },
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

    #[tool(description = "Retrieve a stored buffer by its content hash. Buffers are created by run_pipeline and referenced via output_ref, stdout_ref, and stderr_ref in stage results.")]
    pub async fn get_buffer(
        &self,
        #[tool(param)] hash: String,
    ) -> GetBufferResponse {
        match self.engine.get_buffer(hash).await {
            Ok(content) => GetBufferResponse { content: Some(content), error: None },
            Err(e) => GetBufferResponse { content: None, error: Some(e) },
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

    #[tool(description = include_str!("run_pipeline_tool_stateless.md"))]
    pub async fn run_pipeline(
        &self,
        #[tool(param)] stages: Vec<serde_json::Value>,
    ) -> RunPipelineResponse {
        let parsed: Result<Vec<PipelineStage>, _> = stages
            .into_iter()
            .enumerate()
            .map(|(i, v)| serde_json::from_value(v).map_err(|e| format!("Invalid stage {}: {}", i, e)))
            .collect();
        match parsed {
            Ok(pipeline_stages) => match self.engine.run_pipeline(pipeline_stages, None).await {
                Ok(result) => RunPipelineResponse { result: Some(result), error: None },
                Err(e) => RunPipelineResponse { result: None, error: Some(e) },
            },
            Err(e) => RunPipelineResponse { result: None, error: Some(e) },
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
