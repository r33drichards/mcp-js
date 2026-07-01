//! Legacy HTTP+SSE transport handler.
//!
//! rmcp 1.x dropped the standalone HTTP+SSE server transport (GET /sse + POST
//! /message), so to keep `--sse-port` working this module is served by the
//! vendored rmcp 0.1.5 SSE server (`rmcp_legacy`). It is a hand-written 0.1.5
//! `ServerHandler` (no tool macros — the renamed crate's macros would emit
//! `::rmcp::` paths that resolve to the 1.x crate) that delegates tool calls to
//! the shared, transport-agnostic `mcp_dispatch`. The tool list mirrors the
//! primary handler's core surface (converted to 0.1.5 `Tool`s).
//!
//! Tasks are NOT offered here (0.1.5 predates the tasks utility); task-enabled
//! clients should use the Streamable HTTP transport (`--http-port`).

use std::sync::{Arc, OnceLock};

use rmcp_legacy::{
    Error as McpError, RoleServer, ServerHandler,
    model::{
        Annotated, CallToolRequestParam, CallToolResult, Content, InitializeRequestParam,
        InitializeResult, ListResourcesResult, ListToolsResult, PaginatedRequestParam,
        RawResource, ReadResourceRequestParam, ReadResourceResult, Resource, ResourceContents,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};
use serde_json::json;

use crate::engine::Engine;
use crate::session::SessionVerifier;

const LLMS_TXT: &str = include_str!("llms_txt.md");
const README_MD: &str = include_str!("../README.md");

#[derive(Clone)]
pub struct SseService {
    engine: Engine,
    verifier: Option<Arc<SessionVerifier>>,
    session_id: Arc<OnceLock<String>>,
    mcp_headers: Arc<OnceLock<serde_json::Value>>,
}

impl SseService {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        Self {
            engine,
            verifier,
            session_id: Arc::new(OnceLock::new()),
            mcp_headers: Arc::new(OnceLock::new()),
        }
    }
}

/// Convert a primary (rmcp 1.x) tool descriptor to a legacy (0.1.5) one. The
/// input schema is a plain `serde_json::Map` in both, so it transfers directly.
fn to_legacy_tool(tool: &rmcp::model::Tool) -> Tool {
    Tool {
        name: tool.name.to_string().into(),
        description: tool.description.as_ref().map(|d| d.to_string().into()),
        input_schema: tool.input_schema.clone(),
        annotations: None,
    }
}

fn ok_result(value: serde_json::Value) -> CallToolResult {
    match Content::json(value) {
        Ok(content) => CallToolResult::success(vec![content]),
        Err(e) => CallToolResult::success(vec![Content::text(format!(
            "Failed to serialize response: {e}"
        ))]),
    }
}

fn doc_resources() -> Vec<Resource> {
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

fn read_doc_resource(uri: &str, heap: bool, fs: bool) -> Option<ReadResourceResult> {
    use crate::api::ApiDoc;
    use utoipa::OpenApi as _;

    let text = match uri {
        "docs://readme" => (README_MD.to_string(), "text/markdown"),
        "docs://llms-txt" => (LLMS_TXT.to_string(), "text/markdown"),
        "docs://openapi" => (
            serde_json::to_string_pretty(&ApiDoc::openapi()).unwrap_or_default(),
            "application/json",
        ),
        "docs://tools" => (
            serde_json::to_string_pretty(&crate::mcp::built_in_tool_catalog(heap, fs)).unwrap_or_default(),
            "application/json",
        ),
        _ => return None,
    };

    Some(ReadResourceResult {
        contents: vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some(text.1.into()),
            text: text.0,
        }],
    })
}

impl ServerHandler for SseService {
    fn get_info(&self) -> ServerInfo {
        let instructions = self.engine.instructions_override()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let mode = match (self.engine.heap_enabled(), self.engine.fs_enabled()) {
                    (true, true) => "with per-session V8 heap persistence (globals persist across calls) and a per-session content-addressed filesystem at /work",
                    (true, false) => "with per-session V8 heap persistence (globals persist across calls)",
                    (false, true) => "with a per-session content-addressed filesystem at /work (files persist across calls; JS globals do NOT)",
                    (false, false) => "stateless (no state persists between calls)",
                };
                format!(
                    "JavaScript execution service {mode}. \
                     This is the legacy HTTP+SSE transport (no MCP tasks support — \
                     use the Streamable HTTP transport for tasks). \
                     Use resources/list and resources/read to explore docs://readme, \
                     docs://llms-txt, docs://openapi, and docs://tools before calling tools."
                )
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
            resources: doc_resources(),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        read_doc_resource(&request.uri, self.engine.heap_enabled(), self.engine.fs_enabled())
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
        let tools = crate::mcp::mode_tool_list(&self.engine)
            .iter()
            .map(to_legacy_tool)
            .collect();
        Ok(ListToolsResult { next_cursor: None, tools })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let name = request.name.as_ref();
        let args = request
            .arguments
            .map(serde_json::Value::Object)
            .unwrap_or_else(|| json!({}));
        let session = self.session_id.get().map(String::as_str);
        let headers = self.mcp_headers.get();

        // Mirror the two service modes: stateful exposes the full async tool
        // surface; stateless exposes only run_js (run to completion, return
        // output directly).
        let result = if self.engine.session_capable() {
            crate::mcp_dispatch::call_tool(&self.engine, session, headers, name, &args).await
        } else if name == "run_js" {
            crate::mcp_dispatch::run_js_blocking(&self.engine, headers, &args).await
        } else {
            json!({ "error": format!("unknown tool: {name}") })
        };
        Ok(ok_result(result))
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() {
            let headers = &http_request_part.headers;

            if let Some(ref verifier) = self.verifier {
                let token = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .or_else(|| headers.get("agent-session").and_then(|v| v.to_str().ok()));
                if let Some(token) = token {
                    if verifier.verify(token).await {
                        tracing::info!("JWT verified (SSE)");
                    } else {
                        tracing::warn!("JWT present but failed verification (SSE)");
                    }
                }
            }

            let mut map = serde_json::Map::new();
            for (name, value) in headers.iter() {
                if let Some(key) = name.as_str().strip_prefix("x-mcp-") {
                    if let Ok(v) = value.to_str() {
                        map.insert(key.to_string(), serde_json::Value::String(v.to_string()));
                    }
                }
            }
            if let Some(serde_json::Value::String(sid)) = map.get("session-id") {
                let _ = self.session_id.set(sid.clone());
            }
            if !map.is_empty() {
                let _ = self.mcp_headers.set(serde_json::Value::Object(map));
            }
        }
        Ok(self.get_info())
    }
}
