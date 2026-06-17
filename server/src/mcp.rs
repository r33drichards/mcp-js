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


/// llms.txt content — machine-readable agent guide (https://llmstxt.org/)
const LLMS_TXT: &str = include_str!("llms_txt.md");

/// Full README
const README_MD: &str = include_str!("../README.md");


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


pub struct ToolCatalog {
    pub mode: &'static str,
    pub tools: Vec<ToolDoc>,
}

fn built_in_tools(heap: bool, fs: bool) -> Vec<Tool> {
    if heap || fs {
        let mut tools = McpService::tool_box().list();
        filter_tools_by_capability(&mut tools, heap, fs);
        tools
    } else {
        StatelessMcpService::tool_box().list()
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

/// Build the list of static documentation resources exposed via MCP.
fn doc_resources(heap: bool, fs: bool) -> Vec<Resource> {
    use crate::api::ApiDoc;
    use utoipa::OpenApi as _;

    let openapi_json = serde_json::to_string(&ApiDoc::openapi()).unwrap_or_default();
    let tools_json = serde_json::to_string(&built_in_tool_catalog(heap, fs)).unwrap_or_default();

                        let _ = (openapi_json, tools_json); 
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
fn read_doc_resource(uri: &str, heap: bool, fs: bool) -> Option<ReadResourceResult> {
    use crate::api::ApiDoc;
    use utoipa::OpenApi as _;

    let openapi_json = serde_json::to_string_pretty(&ApiDoc::openapi()).unwrap_or_default();
    let tools_json = serde_json::to_string_pretty(&built_in_tool_catalog(heap, fs)).unwrap_or_default();

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

/// Generic JSON response for the `fs_*` tools.

pub struct FsResponse {
    pub value: serde_json::Value,
}

impl FsResponse {
    fn ok(value: serde_json::Value) -> Self {
        Self { value }
    }
    fn err(error: impl Into<String>) -> Self {
        Self {
            value: json!({ "error": error.into() }),
        }
    }
}

impl IntoContents for FsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(self.value) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert fs response: {}", e))],
        }
    }
}


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


impl McpService {
    "run_js_tool_description.md"
    pub async fn run_js(
        &self,
        
        
        code: Option<String>,
        
        
        file: Option<String>,
        
        
        heap: Option<String>,
        
        
        fs: Option<String>,
        
        
        heap_memory_max_mb: Option<usize>,
        
        
        execution_timeout_secs: Option<u64>,
        
        
        tags: Option<HashMap<String, String>>,
    ) -> RunJsResponse {
        let mut req = self.engine.run_js(code.unwrap_or_default());
        req = req.maybe_file(file);
        if let Some(h) = heap { req = req.heap(h); }
        req = req.maybe_fs(fs);
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

    "Get the status and result of an execution. Returns execution_id, status (running/completed/failed/cancelled/timed_out), result (if completed), heap (if stateful), fs (resulting filesystem snapshot CA id, if a mount was attached), error (if failed), started_at, and completed_at."
    pub async fn get_execution(
        &self,
         execution_id: String,
    ) -> ExecutionStatusResponse {
        match self.engine.get_execution(&execution_id) {
            Ok(info) => ExecutionStatusResponse {
                value: json!({
                    "execution_id": info.id,
                    "status": info.status,
                    "result": info.result,
                    "heap": info.heap,
                    "fs": info.fs,
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

    "Get paginated console output for an execution. Supports two modes: line-based (line_offset + line_limit) or byte-based (byte_offset + byte_limit). If byte_offset is provided, byte mode takes precedence. Response includes both line and byte coordinates for cross-referencing. Use next_line_offset or next_byte_offset from a previous response to resume reading."
    pub async fn get_execution_output(
        &self,
         execution_id: String,
        
        
        line_offset: Option<u64>,
        
        
        line_limit: Option<u64>,
        
        
        byte_offset: Option<u64>,
        
        
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

    "Cancel a running execution. Terminates the V8 isolate."
    pub async fn cancel_execution(
        &self,
         execution_id: String,
    ) -> OkResponse {
        match self.engine.cancel_execution(&execution_id) {
            Ok(()) => OkResponse { ok: true, error: None },
            Err(e) => OkResponse { ok: false, error: Some(e) },
        }
    }

    "List all executions with their status."
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

    "List all named sessions (stateful mode only). Returns an array of session names that have been used via REST session fields or the X-MCP-Session-Id header."
    pub async fn list_sessions(&self) -> ListSessionsResponse {
        match self.engine.list_sessions().await {
            Ok(sessions) => ListSessionsResponse { sessions },
            Err(e) => ListSessionsResponse {
                sessions: vec![format!("Error: {}", e)],
            },
        }
    }

    "List all log entries for the current session (stateful mode only). Each entry contains the input heap hash, output heap hash, code executed, and timestamp. Use the fields parameter to select specific fields (comma-separated: index,input_heap,output_heap,code,timestamp)."
    pub async fn list_session_snapshots(
        &self,
        
        
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

    "Get tags for a heap snapshot (stateful mode only). Returns a map of key-value tags associated with the given heap content hash."
    pub async fn get_heap_tags(
        &self,
         heap: String,
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

    "Set or replace tags on a heap snapshot (stateful mode only). Provide a map of key-value string pairs. This replaces all existing tags for the heap."
    pub async fn set_heap_tags(
        &self,
         heap: String,
         tags: HashMap<String, String>,
    ) -> OkResponse {
        match self.engine.set_heap_tags(heap, tags).await {
            Ok(()) => OkResponse { ok: true, error: None },
            Err(e) => OkResponse { ok: false, error: Some(e) },
        }
    }

    "Delete tags from a heap snapshot (stateful mode only). If keys is provided (comma-separated), only those tag keys are removed. If keys is omitted, all tags are deleted."
    pub async fn delete_heap_tags(
        &self,
         heap: String,
        
        
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

    "Query heap snapshots by tags (stateful mode only). Provide a map of key-value pairs to match. Returns all heaps whose tags contain all the specified key-value pairs."
    pub async fn query_heaps_by_tags(
        &self,
         tags: HashMap<String, String>,
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

    
    "List filesystem snapshot labels. Returns each label name and its current head CA id (hex)."
    pub async fn fs_ls(&self) -> FsResponse {
        match self.engine.fs_list_labels().await {
            Ok(labels) => FsResponse::ok(json!({ "labels": labels })),
            Err(e) => FsResponse::err(e),
        }
    }

    "Resolve a filesystem snapshot label to its current head CA id (hex). Use this as the `fs` argument to run_js to mount it."
    pub async fn fs_pull(&self,  label: String) -> FsResponse {
        match self.engine.fs_resolve_label(&label).await {
            Ok(Some(ca_id)) => FsResponse::ok(json!({ "label": label, "ca_id": ca_id })),
            Ok(None) => FsResponse::err(format!("unknown label: {label}")),
            Err(e) => FsResponse::err(e),
        }
    }

    "Create or repoint a filesystem snapshot label to a CA id (hex). Pass an optional `message` (a commit-style note) to record on the reflog entry."
    pub async fn fs_label(
        &self,
         name: String,
         ca_id: String,
        
        
        message: Option<String>,
    ) -> FsResponse {
        match self.engine.fs_set_label(&name, &ca_id, message).await {
            Ok(()) => FsResponse::ok(json!({ "label": name, "ca_id": ca_id })),
            Err(e) => FsResponse::err(e),
        }
    }

    "Show the reflog (move history) for a filesystem snapshot label, oldest first. Each entry has at, from, to (CA ids), op (create/push/reset/force), and an optional message. Use a `to` value as the ca_id for fs_reset. Pass `limit` to return only the most recent N entries (bounding the scan over long histories)."
    pub async fn fs_log(
        &self,
         label: String,
        
        
        limit: Option<usize>,
    ) -> FsResponse {
        match self.engine.fs_label_log(&label, limit).await {
            Ok(entries) => FsResponse::ok(json!({ "label": label, "log": entries })),
            Err(e) => FsResponse::err(e),
        }
    }

    "Advance a filesystem snapshot label to a CA id (typically the `fs` value returned by a completed run_js execution). Default is reject-and-rebase: pass `expected` (the head you pulled) and the push fails if the label moved since. Set force=true to override, or detach=true to just return the CA id without touching the label. Pass an optional `message` (a commit-style note, max 4096 bytes) to record on the reflog entry."
    pub async fn fs_push(
        &self,
         ca_id: String,
        
        
        label: Option<String>,
        
        
        expected: Option<String>,
        
        
        force: Option<bool>,
        
        
        detach: Option<bool>,
        
        
        message: Option<String>,
    ) -> FsResponse {
        if detach.unwrap_or(false) {
            return FsResponse::ok(json!({ "status": "detached", "ca_id": ca_id }));
        }
        let Some(label) = label else {
            return FsResponse::err(
                "fs_push requires a `label` unless detach=true".to_string(),
            );
        };
        match self
            .engine
            .fs_push(&label, &ca_id, expected, force.unwrap_or(false), message)
            .await
        {
            Ok(outcome) => match serde_json::to_value(&outcome) {
                Ok(v) => FsResponse::ok(v),
                Err(e) => FsResponse::err(e.to_string()),
            },
            Err(e) => FsResponse::err(e),
        }
    }

    "Reset a filesystem snapshot label to an earlier CA id from its reflog (rollback). The CA id must appear in the label's reflog (see fs_log) unless allow_unlogged=true. Pass an optional `message` (a commit-style note) to record on the reflog entry."
    pub async fn fs_reset(
        &self,
         label: String,
         ca_id: String,
        
        
        allow_unlogged: Option<bool>,
        
        
        message: Option<String>,
    ) -> FsResponse {
        match self
            .engine
            .fs_reset(&label, &ca_id, allow_unlogged.unwrap_or(false), message)
            .await
        {
            Ok(()) => FsResponse::ok(json!({ "label": label, "ca_id": ca_id })),
            Err(e) => FsResponse::err(e),
        }
    }

    "Three-way merge two filesystem snapshots (CA ids) into a new snapshot. Pass `base` — the snapshot both sides diverged from (e.g. the label head you mounted before two runs) — so only paths BOTH sides changed conflict; omit it for a 2-way merge. Text files are merged at line level: edits to different lines of the same file auto-merge cleanly. On success returns the merged snapshot's ca_id (push it to a label separately). On conflict returns status=conflict with, per path: each side's content id (null = absent), kind (text/binary/sqlite/modify-delete), and for text the diff3 conflict `markers` plus unified `diff_ours`/`diff_theirs` so you can resolve at line level (edit the markers, write the file back, push). Set prefer=ours|theirs to auto-resolve remaining conflicts to that side."
    pub async fn fs_merge(
        &self,
         ours: String,
         theirs: String,
        
        
        base: Option<String>,
        
        
        prefer: Option<String>,
    ) -> FsResponse {
        let prefer = match crate::engine::fs_merge::Prefer::parse(prefer.as_deref()) {
            Ok(p) => p,
            Err(e) => return FsResponse::err(e),
        };
        match self.engine.fs_merge(&ours, &theirs, base, prefer).await {
            Ok(result) => match serde_json::to_value(&result) {
                Ok(v) => FsResponse::ok(v),
                Err(e) => FsResponse::err(e.to_string()),
            },
            Err(e) => FsResponse::err(e),
        }
    }
}

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
            resources: doc_resources(self.engine.heap_enabled(), self.engine.fs_enabled()),
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
        let mut tools = Self::tool_box().list();
                        filter_tools_by_capability(&mut tools, self.engine.heap_enabled(), self.engine.fs_enabled());
        apply_run_js_description_override(&mut tools, self.engine.run_js_description_override());
        if let Some(client) = &self.mcp_client {
            tools.extend(client.stub_tools());
        }
                tools.extend(self.engine.wasm_stub_tools());
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
                if let Some(result) = self.engine.wasm_stub_call_response(&request.name, request.arguments.as_ref()) {
            return Ok(result);
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
        Ok(self.get_info())
    }
}



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


impl StatelessMcpService {
    "run_js_tool_stateless.md"
    pub async fn run_js(
        &self,
        
        
        code: Option<String>,
        
        
        file: Option<String>,
        
        
        heap_memory_max_mb: Option<usize>,
        
        
        execution_timeout_secs: Option<u64>,
    ) -> StatelessRunJsResponse {
                let mut req = self.engine.run_js(code.unwrap_or_default());
        req = req.maybe_file(file);
        if let Some(mb) = heap_memory_max_mb { req = req.heap_memory_max_mb(mb); }
        if let Some(secs) = execution_timeout_secs { req = req.execution_timeout_secs(secs); }
        req = req.maybe_mcp_headers(self.mcp_headers.get().cloned());
        let exec_id = match req.execute().await {
            Ok(id) => id,
            Err(e) => return StatelessRunJsResponse {
                value: json!({ "error": e }),
            },
        };

                let poll_interval = tokio::time::Duration::from_millis(50);
        let max_polls = 6000;         let mut status = String::new();
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

                let output = match self.engine.get_execution_output(&exec_id, None, Some(u64::MAX), None, None) {
            Ok(page) => page.data,
            Err(_) => String::new(),
        };

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
            resources: doc_resources(false, false),
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
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
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = Self::tool_box().list();
                        filter_tools_by_capability(&mut tools, self.engine.heap_enabled(), self.engine.fs_enabled());
        apply_run_js_description_override(&mut tools, self.engine.run_js_description_override());
        if let Some(client) = &self.mcp_client {
            tools.extend(client.stub_tools());
        }
                tools.extend(self.engine.wasm_stub_tools());
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
                if let Some(result) = self.engine.wasm_stub_call_response(&request.name, request.arguments.as_ref()) {
            return Ok(result);
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

    
    fn override_replaces_only_run_js_description() {
        let mut tools = vec![tool("run_js", "original"), tool("get_execution", "other")];
        apply_run_js_description_override(&mut tools, Some(Arc::from("custom description")));

        assert_eq!(tools[0].description.as_deref(), Some("custom description"));
                assert_eq!(tools[1].description.as_deref(), Some("other"));
    }

    
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
        Tool {
            name: "run_js".into(),
            description: Some("heapy".into()),
            input_schema: Arc::new(schema),
            annotations: None,
        }
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

    
    fn fs_only_hides_heap_tools_and_params() {
        let mut tools = full_surface();
        filter_tools_by_capability(&mut tools, false, true);
        let n = names(&tools);
        assert!(n.contains(&"run_js".to_string()));
        assert!(n.contains(&"fs_ls".to_string()));
                assert!(!n.contains(&"get_heap_tags".to_string()));
        assert!(!n.contains(&"query_heaps_by_tags".to_string()));
                let props = run_js_props(&tools);
        assert!(props.contains(&"code".to_string()));
        assert!(props.contains(&"fs".to_string()));
        assert!(!props.contains(&"heap".to_string()));
        assert!(!props.contains(&"tags".to_string()));
                let rj = tools.iter().find(|t| t.name.as_ref() == "run_js").unwrap();
        assert!(rj.description.as_deref().unwrap().contains("globals"));
    }

    
    fn heap_only_hides_fs_tools_keeps_heap_params() {
        let mut tools = full_surface();
        filter_tools_by_capability(&mut tools, true, false);
        let n = names(&tools);
        assert!(n.contains(&"get_heap_tags".to_string()));
        assert!(!n.contains(&"fs_ls".to_string()));
        assert!(!n.contains(&"fs_push".to_string()));
                assert!(run_js_props(&tools).contains(&"heap".to_string()));
    }

    
    fn both_keeps_everything() {
        let mut tools = full_surface();
        filter_tools_by_capability(&mut tools, true, true);
        let n = names(&tools);
        assert!(n.contains(&"get_heap_tags".to_string()));
        assert!(n.contains(&"fs_ls".to_string()));
        assert!(run_js_props(&tools).contains(&"heap".to_string()));
    }
}
