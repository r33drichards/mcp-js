use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    task_handler,
    task_manager::OperationProcessor,
    tool, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

use crate::engine::Engine;
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

fn built_in_tools(heap: bool, fs: bool) -> Vec<Tool> {
    if heap || fs {
        let mut tools = McpService::tool_router().list_all();
        filter_tools_by_capability(&mut tools, heap, fs);
        tools
    } else {
        StatelessMcpService::tool_router().list_all()
    }
}

pub fn built_in_tool_catalog(heap: bool, fs: bool) -> ToolCatalog {
    ToolCatalog {
        mode: match (heap, fs) {
            (true, true) => "heap+fs",
            (true, false) => "heap",
            (false, true) => "fs",
            (false, false) => "stateless",
        },
        tools: built_in_tools(heap, fs)
            .into_iter()
            .map(ToolDoc::from_tool)
            .collect(),
    }
}

/// The capability-filtered core tool list for the engine's current mode, used
/// by the legacy SSE handler (`mcp_sse.rs`). Excludes the upstream-MCP / WASM
/// discovery stubs (a Streamable-HTTP convenience); SSE clients drive modules
/// from `run_js` directly.
pub fn mode_tool_list(engine: &Engine) -> Vec<Tool> {
    let mut tools = if engine.session_capable() {
        let mut tools = McpService::tool_router().list_all();
        filter_tools_by_capability(&mut tools, engine.heap_enabled(), engine.fs_enabled());
        tools
    } else {
        StatelessMcpService::tool_router().list_all()
    };
    apply_run_js_description_override(&mut tools, engine.run_js_description_override());
    tools
}

/// Build the list of static documentation resources exposed via MCP.
fn doc_resources(_heap: bool, _fs: bool) -> Vec<Resource> {
    vec![
        Annotated::new(
            RawResource::new("docs://readme", "README")
                .with_description("Full mcp-v8 README with usage, CLI flags, and examples (Markdown)")
                .with_mime_type("text/markdown")
                .with_size(README_MD.len() as u32),
            None,
        ),
        Annotated::new(
            RawResource::new("docs://llms-txt", "llms.txt")
                .with_description("Machine-readable agent guide: connection options, tools, REST API (Markdown)")
                .with_mime_type("text/markdown")
                .with_size(LLMS_TXT.len() as u32),
            None,
        ),
        Annotated::new(
            RawResource::new("docs://openapi", "OpenAPI spec")
                .with_description("OpenAPI 3.0 JSON spec for the REST API (/api/exec, /api/executions/*, etc.)")
                .with_mime_type("application/json"),
            None,
        ),
        Annotated::new(
            RawResource::new("docs://tools", "MCP tool list")
                .with_description("JSON list of available MCP tools with descriptions, mode-aware")
                .with_mime_type("application/json"),
            None,
        ),
    ]
}

/// Read a single documentation resource by URI.
/// Returns `None` when the URI is not recognised.
fn read_doc_resource(uri: &str, heap: bool, fs: bool) -> Option<ReadResourceResult> {
    use crate::api::ApiDoc;
    use utoipa::OpenApi as _;

    let openapi_json = serde_json::to_string_pretty(&ApiDoc::openapi()).unwrap_or_default();
    let tools_json = serde_json::to_string_pretty(&built_in_tool_catalog(heap, fs)).unwrap_or_default();

    let text = match uri {
        "docs://readme" => (README_MD.to_string(), "text/markdown"),
        "docs://llms-txt" => (LLMS_TXT.to_string(), "text/markdown"),
        "docs://openapi" => (openapi_json, "application/json"),
        "docs://tools" => (tools_json, "application/json"),
        _ => return None,
    };

    Some(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
        uri: uri.to_string(),
        mime_type: Some(text.1.into()),
        text: text.0,
        meta: None,
    }]))
}

// ── Tool result helper ──────────────────────────────────────────────────

/// Wrap a JSON value as a successful `CallToolResult` (single JSON content).
fn json_result(value: serde_json::Value) -> Result<CallToolResult, McpError> {
    match Content::json(value) {
        Ok(content) => Ok(CallToolResult::success(vec![content])),
        Err(e) => Ok(CallToolResult::success(vec![Content::text(format!(
            "Failed to serialize response: {e}"
        ))])),
    }
}

// ── Tool argument structs ─────────────────────────────────────────────────
//
// These exist for the rmcp tool macros' input-schema generation. The actual
// tool logic lives in `mcp_dispatch` (shared with the legacy SSE handler), so
// the tool methods just forward their (serialized) arguments there.

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RunJsArgs {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub heap: Option<String>,
    #[serde(default)]
    pub fs: Option<String>,
    #[serde(default)]
    pub heap_memory_max_mb: Option<usize>,
    #[serde(default)]
    pub execution_timeout_secs: Option<u64>,
    #[serde(default)]
    pub tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct StatelessRunJsArgs {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub heap_memory_max_mb: Option<usize>,
    #[serde(default)]
    pub execution_timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ExecutionIdArg {
    pub execution_id: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetExecutionOutputArgs {
    pub execution_id: String,
    #[serde(default)]
    pub line_offset: Option<u64>,
    #[serde(default)]
    pub line_limit: Option<u64>,
    #[serde(default)]
    pub byte_offset: Option<u64>,
    #[serde(default)]
    pub byte_limit: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ListSessionSnapshotsArgs {
    #[serde(default)]
    pub fields: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct HeapArg {
    pub heap: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SetHeapTagsArgs {
    pub heap: String,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DeleteHeapTagsArgs {
    pub heap: String,
    #[serde(default)]
    pub keys: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct TagsArg {
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FsPullArgs {
    pub label: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FsLabelArgs {
    pub name: String,
    pub ca_id: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FsLogArgs {
    pub label: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FsPushArgs {
    pub ca_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub expected: Option<String>,
    #[serde(default)]
    pub force: Option<bool>,
    #[serde(default)]
    pub detach: Option<bool>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FsResetArgs {
    pub label: String,
    pub ca_id: String,
    #[serde(default)]
    pub allow_unlogged: Option<bool>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FsMergeArgs {
    pub ours: String,
    pub theirs: String,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub prefer: Option<String>,
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

/// `run_js` description for a session-capable server WITHOUT heap persistence
/// (fs-only): it must not claim JS globals persist between calls.
const RUN_JS_FS_ONLY_DESC: &str = include_str!("run_js_tool_fs_only.md");

/// Heap-only tools (heap-tag management). Hidden when heap persistence is off.
const HEAP_ONLY_TOOLS: &[&str] =
    &["get_heap_tags", "set_heap_tags", "delete_heap_tags", "query_heaps_by_tags"];

/// Filesystem snapshot tools. Hidden when fs persistence is off.
const FS_TOOLS: &[&str] =
    &["fs_ls", "fs_pull", "fs_label", "fs_log", "fs_push", "fs_reset", "fs_merge"];

/// True if `name` is a tool that requires a capability the engine doesn't have.
fn tool_requires_missing_capability(name: &str, heap: bool, fs: bool) -> bool {
    (!heap && HEAP_ONLY_TOOLS.contains(&name)) || (!fs && FS_TOOLS.contains(&name))
}

/// Strip heap-specific `run_js` parameters (`heap`, `tags`) from a tool's input
/// schema when heap persistence is off, so the surface matches the capabilities.
fn strip_run_js_heap_params(tool: &mut Tool) {
    let schema = Arc::make_mut(&mut tool.input_schema);
    if let Some(props) = schema.get_mut("properties").and_then(|v| v.as_object_mut()) {
        props.remove("heap");
        props.remove("tags");
    }
    if let Some(required) = schema.get_mut("required").and_then(|v| v.as_array_mut()) {
        required.retain(|v| v.as_str() != Some("heap") && v.as_str() != Some("tags"));
    }
}

/// Restrict the McpService tool surface to the engine's enabled capabilities:
/// drop heap-only tools when heap is off and fs tools when fs is off, and pick a
/// `run_js` description / schema that matches (no "globals persist" when heap is
/// off). `run_js`, execution, and session tools are always kept.
fn filter_tools_by_capability(tools: &mut Vec<Tool>, heap: bool, fs: bool) {
    tools.retain(|t| !tool_requires_missing_capability(t.name.as_ref(), heap, fs));
    if !heap {
        for tool in tools.iter_mut() {
            if tool.name.as_ref() == "run_js" {
                if fs {
                    tool.description = Some(RUN_JS_FS_ONLY_DESC.into());
                }
                strip_run_js_heap_params(tool);
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
    /// Tool registry generated by `#[tool_router]`.
    tool_router: ToolRouter<McpService>,
    /// Backing store for asynchronous task execution (`#[task_handler]`).
    processor: Arc<Mutex<OperationProcessor>>,
}

impl McpService {
    /// Forward a tool call to the transport-agnostic dispatcher and wrap the
    /// result as a `CallToolResult`.
    async fn dispatch<T: Serialize>(&self, name: &str, args: &T) -> Result<CallToolResult, McpError> {
        let value = serde_json::to_value(args).unwrap_or_else(|_| json!({}));
        let result = crate::mcp_dispatch::call_tool(
            &self.engine,
            self.session_id.get().map(String::as_str),
            self.mcp_headers.get(),
            name,
            &value,
        )
        .await;
        json_result(result)
    }
}

#[tool_router]
impl McpService {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        let mcp_client = engine.mcp_client_manager();
        Self {
            engine,
            verifier,
            mcp_client,
            session_id: Arc::new(OnceLock::new()),
            mcp_headers: Arc::new(OnceLock::new()),
            tool_router: Self::tool_router(),
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
        }
    }

    #[doc = include_str!("run_js_tool_description.md")]
    #[tool(execution(task_support = "optional"))]
    pub async fn run_js(
        &self,
        Parameters(args): Parameters<RunJsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("run_js", &args).await
    }

    #[tool(description = "Get the status and result of an execution. Returns execution_id, status (running/completed/failed/cancelled/timed_out), result (if completed), heap (if stateful), fs (resulting filesystem snapshot CA id, if a mount was attached), error (if failed), started_at, and completed_at.")]
    pub async fn get_execution(
        &self,
        Parameters(args): Parameters<ExecutionIdArg>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("get_execution", &args).await
    }

    #[tool(description = "Get paginated console output for an execution. Supports two modes: line-based (line_offset + line_limit) or byte-based (byte_offset + byte_limit). If byte_offset is provided, byte mode takes precedence. Response includes both line and byte coordinates for cross-referencing. Use next_line_offset or next_byte_offset from a previous response to resume reading.")]
    pub async fn get_execution_output(
        &self,
        Parameters(args): Parameters<GetExecutionOutputArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("get_execution_output", &args).await
    }

    #[tool(description = "Cancel a running execution. Terminates the V8 isolate.")]
    pub async fn cancel_execution(
        &self,
        Parameters(args): Parameters<ExecutionIdArg>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("cancel_execution", &args).await
    }

    #[tool(description = "List all executions with their status.")]
    pub async fn list_executions(&self) -> Result<CallToolResult, McpError> {
        self.dispatch("list_executions", &json!({})).await
    }

    #[tool(description = "List all named sessions (stateful mode only). Returns an array of session names that have been used via REST session fields or the X-MCP-Session-Id header.")]
    pub async fn list_sessions(&self) -> Result<CallToolResult, McpError> {
        self.dispatch("list_sessions", &json!({})).await
    }

    #[tool(description = "List all log entries for the current session (stateful mode only). Each entry contains the input heap hash, output heap hash, code executed, and timestamp. Use the fields parameter to select specific fields (comma-separated: index,input_heap,output_heap,code,timestamp).")]
    pub async fn list_session_snapshots(
        &self,
        Parameters(args): Parameters<ListSessionSnapshotsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("list_session_snapshots", &args).await
    }

    #[tool(description = "Get tags for a heap snapshot (stateful mode only). Returns a map of key-value tags associated with the given heap content hash.")]
    pub async fn get_heap_tags(
        &self,
        Parameters(args): Parameters<HeapArg>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("get_heap_tags", &args).await
    }

    #[tool(description = "Set or replace tags on a heap snapshot (stateful mode only). Provide a map of key-value string pairs. This replaces all existing tags for the heap.")]
    pub async fn set_heap_tags(
        &self,
        Parameters(args): Parameters<SetHeapTagsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("set_heap_tags", &args).await
    }

    #[tool(description = "Delete tags from a heap snapshot (stateful mode only). If keys is provided (comma-separated), only those tag keys are removed. If keys is omitted, all tags are deleted.")]
    pub async fn delete_heap_tags(
        &self,
        Parameters(args): Parameters<DeleteHeapTagsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("delete_heap_tags", &args).await
    }

    #[tool(description = "Query heap snapshots by tags (stateful mode only). Provide a map of key-value pairs to match. Returns all heaps whose tags contain all the specified key-value pairs.")]
    pub async fn query_heaps_by_tags(
        &self,
        Parameters(args): Parameters<TagsArg>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("query_heaps_by_tags", &args).await
    }

    // ── fs snapshot tools ────────────────────────────────────────────────

    #[tool(description = "List filesystem snapshot labels. Returns each label name and its current head CA id (hex).")]
    pub async fn fs_ls(&self) -> Result<CallToolResult, McpError> {
        self.dispatch("fs_ls", &json!({})).await
    }

    #[tool(description = "Resolve a filesystem snapshot label to its current head CA id (hex). Use this as the `fs` argument to run_js to mount it.")]
    pub async fn fs_pull(
        &self,
        Parameters(args): Parameters<FsPullArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("fs_pull", &args).await
    }

    #[tool(description = "Create or repoint a filesystem snapshot label to a CA id (hex). Pass an optional `message` (a commit-style note) to record on the reflog entry.")]
    pub async fn fs_label(
        &self,
        Parameters(args): Parameters<FsLabelArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("fs_label", &args).await
    }

    #[tool(description = "Show the reflog (move history) for a filesystem snapshot label, oldest first. Each entry has at, from, to (CA ids), op (create/push/reset/force), and an optional message. Use a `to` value as the ca_id for fs_reset. Pass `limit` to return only the most recent N entries (bounding the scan over long histories).")]
    pub async fn fs_log(
        &self,
        Parameters(args): Parameters<FsLogArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("fs_log", &args).await
    }

    #[tool(description = "Advance a filesystem snapshot label to a CA id (typically the `fs` value returned by a completed run_js execution). Default is reject-and-rebase: pass `expected` (the head you pulled) and the push fails if the label moved since. Set force=true to override, or detach=true to just return the CA id without touching the label. Pass an optional `message` (a commit-style note, max 4096 bytes) to record on the reflog entry.")]
    pub async fn fs_push(
        &self,
        Parameters(args): Parameters<FsPushArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("fs_push", &args).await
    }

    #[tool(description = "Reset a filesystem snapshot label to an earlier CA id from its reflog (rollback). The CA id must appear in the label's reflog (see fs_log) unless allow_unlogged=true. Pass an optional `message` (a commit-style note) to record on the reflog entry.")]
    pub async fn fs_reset(
        &self,
        Parameters(args): Parameters<FsResetArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("fs_reset", &args).await
    }

    #[tool(description = "Three-way merge two filesystem snapshots (CA ids) into a new snapshot. Pass `base` — the snapshot both sides diverged from (e.g. the label head you mounted before two runs) — so only paths BOTH sides changed conflict; omit it for a 2-way merge. Text files are merged at line level: edits to different lines of the same file auto-merge cleanly. On success returns the merged snapshot's ca_id (push it to a label separately). On conflict returns status=conflict with, per path: each side's content id (null = absent), kind (text/binary/sqlite/modify-delete), and for text the diff3 conflict `markers` plus unified `diff_ours`/`diff_theirs` so you can resolve at line level (edit the markers, write the file back, push). Set prefer=ours|theirs to auto-resolve remaining conflicts to that side.")]
    pub async fn fs_merge(
        &self,
        Parameters(args): Parameters<FsMergeArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.dispatch("fs_merge", &args).await
    }
}

/// Build the capability-filtered, override-applied, stub-augmented tool list.
fn list_tools_for<S: Send + Sync + 'static>(
    router: &ToolRouter<S>,
    engine: &Engine,
    mcp_client: &Option<Arc<McpClientManager>>,
) -> Vec<Tool> {
    let mut tools = router.list_all();
    filter_tools_by_capability(&mut tools, engine.heap_enabled(), engine.fs_enabled());
    apply_run_js_description_override(&mut tools, engine.run_js_description_override());
    if let Some(client) = mcp_client {
        tools.extend(client.stub_tools());
    }
    tools.extend(engine.wasm_stub_tools());
    tools
}

#[task_handler]
impl ServerHandler for McpService {
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
                     Use resources/list and resources/read to explore docs://readme, \
                     docs://llms-txt, docs://openapi, and docs://tools before calling tools."
                )
            });
        let mut info = ServerInfo::default();
        info.instructions = Some(instructions);
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_tasks()
            .build();
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            next_cursor: None,
            resources: doc_resources(self.engine.heap_enabled(), self.engine.fs_enabled()),
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
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
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            next_cursor: None,
            tools: list_tools_for(&self.tool_router, &self.engine, &self.mcp_client),
            meta: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(client) = &self.mcp_client {
            if let Some(result) = client.stub_call_response(&request.name, request.arguments.as_ref()) {
                return Ok(result);
            }
        }
        // WASM module stubs return run_js usage instructions instead of dispatching.
        if let Some(result) = self.engine.wasm_stub_call_response(&request.name, request.arguments.as_ref()) {
            return Ok(result);
        }
        self.tool_router.call(ToolCallContext::new(self, request, context)).await
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        capture_mcp_headers(&context, Some(&self.session_id), &self.mcp_headers, self.verifier.as_ref()).await;
        Ok(self.get_info())
    }
}

// ── StatelessMcpService ─────────────────────────────────────────────────
//
// Stateless shell mode: single `run_js` tool that executes code and returns
// console output directly. No execution IDs are exposed to callers — session
// isolation is automatic.

#[derive(Clone)]
pub struct StatelessMcpService {
    engine: Engine,
    verifier: Option<Arc<SessionVerifier>>,
    mcp_client: Option<Arc<McpClientManager>>,
    /// X-MCP-* headers from the initialize request, available for policy evaluation.
    mcp_headers: Arc<OnceLock<serde_json::Value>>,
    tool_router: ToolRouter<StatelessMcpService>,
    processor: Arc<Mutex<OperationProcessor>>,
}

#[tool_router]
impl StatelessMcpService {
    pub fn new(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> Self {
        let mcp_client = engine.mcp_client_manager();
        Self {
            engine,
            verifier,
            mcp_client,
            mcp_headers: Arc::new(OnceLock::new()),
            tool_router: Self::tool_router(),
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
        }
    }

    #[doc = include_str!("run_js_tool_stateless.md")]
    #[tool(execution(task_support = "optional"))]
    pub async fn run_js(
        &self,
        Parameters(args): Parameters<StatelessRunJsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let value = serde_json::to_value(&args).unwrap_or_else(|_| json!({}));
        let result =
            crate::mcp_dispatch::run_js_blocking(&self.engine, self.mcp_headers.get(), &value).await;
        json_result(result)
    }
}

#[task_handler]
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
        let mut info = ServerInfo::default();
        info.instructions = Some(instructions);
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_tasks()
            .build();
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            next_cursor: None,
            resources: doc_resources(false, false),
            meta: None,
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
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            next_cursor: None,
            tools: list_tools_for(&self.tool_router, &self.engine, &self.mcp_client),
            meta: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(client) = &self.mcp_client {
            if let Some(result) = client.stub_call_response(&request.name, request.arguments.as_ref()) {
                return Ok(result);
            }
        }
        if let Some(result) = self.engine.wasm_stub_call_response(&request.name, request.arguments.as_ref()) {
            return Ok(result);
        }
        self.tool_router.call(ToolCallContext::new(self, request, context)).await
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        capture_mcp_headers(&context, None, &self.mcp_headers, self.verifier.as_ref()).await;
        Ok(self.get_info())
    }
}

/// Shared initialize-time header handling: JWT verification (if configured),
/// capturing X-MCP-* headers, and (optionally) the session id.
pub(crate) async fn capture_mcp_headers(
    context: &RequestContext<RoleServer>,
    session_id: Option<&Arc<OnceLock<String>>>,
    mcp_headers: &Arc<OnceLock<serde_json::Value>>,
    verifier: Option<&Arc<SessionVerifier>>,
) {
    let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() else {
        return;
    };
    let initialize_headers = &http_request_part.headers;
    let initialize_uri = &http_request_part.uri;
    tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");

    if let Some(verifier) = verifier {
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
            None => tracing::debug!("No Authorization/AgentSession header in initialize request"),
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

    if let Some(session_id) = session_id {
        if let Some(serde_json::Value::String(sid)) = mcp_header_map.get("session-id") {
            tracing::info!(session_id = sid.as_str(), "Session ID from X-MCP-Session-Id header");
            let _ = session_id.set(sid.clone());
        }
    }

    if !mcp_header_map.is_empty() {
        tracing::info!(?mcp_header_map, "X-MCP-* headers captured");
        let _ = mcp_headers.set(serde_json::Value::Object(mcp_header_map));
    }
}

#[cfg(test)]
mod tests {
    use super::apply_run_js_description_override;
    use rmcp::model::Tool;
    use std::sync::Arc;

    fn tool(name: &'static str, desc: &'static str) -> Tool {
        Tool::new(name, desc, Arc::new(serde_json::Map::new()))
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

    use super::filter_tools_by_capability;

    fn run_js_with_heap_param() -> Tool {
        let mut schema = serde_json::Map::new();
        let mut props = serde_json::Map::new();
        props.insert("code".to_string(), serde_json::json!({"type": "string"}));
        props.insert("heap".to_string(), serde_json::json!({"type": "string"}));
        props.insert("fs".to_string(), serde_json::json!({"type": "string"}));
        props.insert("tags".to_string(), serde_json::json!({"type": "object"}));
        schema.insert("properties".to_string(), serde_json::Value::Object(props));
        Tool::new("run_js", "heapy", Arc::new(schema))
    }

    fn names(tools: &[Tool]) -> Vec<String> {
        tools.iter().map(|t| t.name.to_string()).collect()
    }

    fn full_surface() -> Vec<Tool> {
        vec![
            run_js_with_heap_param(),
            tool("get_execution", "x"),
            tool("get_heap_tags", "x"),
            tool("set_heap_tags", "x"),
            tool("query_heaps_by_tags", "x"),
            tool("fs_ls", "x"),
            tool("fs_push", "x"),
        ]
    }

    fn run_js_props(tools: &[Tool]) -> Vec<String> {
        let rj = tools.iter().find(|t| t.name.as_ref() == "run_js").unwrap();
        rj.input_schema
            .get("properties")
            .and_then(|v| v.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn fs_only_hides_heap_tools_and_params() {
        let mut tools = full_surface();
        filter_tools_by_capability(&mut tools, false, true);
        let n = names(&tools);
        assert!(n.contains(&"run_js".to_string()));
        assert!(n.contains(&"fs_ls".to_string()));
        // Heap-tag tools gone.
        assert!(!n.contains(&"get_heap_tags".to_string()));
        assert!(!n.contains(&"query_heaps_by_tags".to_string()));
        // run_js loses heap/tags params but keeps code/fs.
        let props = run_js_props(&tools);
        assert!(props.contains(&"code".to_string()));
        assert!(props.contains(&"fs".to_string()));
        assert!(!props.contains(&"heap".to_string()));
        assert!(!props.contains(&"tags".to_string()));
        // Description must not promise globals persist.
        let rj = tools.iter().find(|t| t.name.as_ref() == "run_js").unwrap();
        assert!(rj.description.as_deref().unwrap().contains("globals"));
    }

    #[test]
    fn heap_only_hides_fs_tools_keeps_heap_params() {
        let mut tools = full_surface();
        filter_tools_by_capability(&mut tools, true, false);
        let n = names(&tools);
        assert!(n.contains(&"get_heap_tags".to_string()));
        assert!(!n.contains(&"fs_ls".to_string()));
        assert!(!n.contains(&"fs_push".to_string()));
        // heap param retained when heap is on.
        assert!(run_js_props(&tools).contains(&"heap".to_string()));
    }

    #[test]
    fn both_keeps_everything() {
        let mut tools = full_surface();
        filter_tools_by_capability(&mut tools, true, true);
        let n = names(&tools);
        assert!(n.contains(&"get_heap_tags".to_string()));
        assert!(n.contains(&"fs_ls".to_string()));
        assert!(run_js_props(&tools).contains(&"heap".to_string()));
    }
}
