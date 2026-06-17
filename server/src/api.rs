use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{OpenApi, ToSchema};

/// Maximum size of an `/api/exec` request body (16 MiB), for both the JSON
/// body and raw script uploads.
const MAX_EXEC_BODY_BYTES: usize = 16 * 1024 * 1024;

use crate::engine::Engine;


/// llms.txt — machine-readable guide for AI agents (https://llmstxt.org/)
const LLMS_TXT: &str = include_str!("llms_txt.md");

/// Full README for the /docs endpoint
const README_MD: &str = include_str!("../README.md");


/// The version of this server binary, from Cargo.toml at compile time.
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

static CLI_LINUX_X86_64:  &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cli-linux-x86_64.bin"));
static CLI_LINUX_AARCH64: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cli-linux-aarch64.bin"));
static CLI_MACOS_AARCH64: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cli-macos-aarch64.bin"));

struct PlatformCli {
    platform: &'static str,
    filename: &'static str,
    bytes:    &'static [u8],
}

const PLATFORMS: &[PlatformCli] = &[
    PlatformCli { platform: "linux-x86_64",  filename: "mcp-v8-cli-linux-x86_64",  bytes: CLI_LINUX_X86_64  },
    PlatformCli { platform: "linux-aarch64", filename: "mcp-v8-cli-linux-arm64",   bytes: CLI_LINUX_AARCH64 },
    PlatformCli { platform: "macos-aarch64", filename: "mcp-v8-cli-macos-arm64",   bytes: CLI_MACOS_AARCH64 },
];

fn find_platform(platform: &str) -> Option<&'static PlatformCli> {
    PLATFORMS.iter().find(|p| p.platform == platform)
}

/// Request body for executing JavaScript code.

pub struct ExecRequest {
    /// JavaScript (or TypeScript) source code to execute.
    pub code: String,
    /// Serialised heap snapshot key to restore before execution.
    
    pub heap: Option<String>,
    /// Filesystem snapshot handle to mount: a label name or 64-hex CA id.
    /// Independent of `heap`.
    
    pub fs: Option<String>,
    /// Session identifier used for tagging / logging.
    
    pub session: Option<String>,
    /// Per-execution V8 heap memory cap in megabytes.
    
    pub heap_memory_max_mb: Option<usize>,
    /// Per-execution timeout in seconds (overrides server default).
    
    pub execution_timeout_secs: Option<u64>,
    /// Arbitrary key/value tags attached to the resulting heap snapshot.
    
    pub tags: Option<HashMap<String, String>>,
}

/// Accepted response containing the new execution's ID.

pub struct ExecAccepted {
    /// Unique identifier for the queued execution.
    pub execution_id: String,
}

/// Detailed status of a single execution.

pub struct ExecutionInfo {
    pub execution_id: String,
    /// Current status: `running`, `completed`, `failed`, `cancelled`, `timed_out`.
    pub status: String,
    /// Final return value serialised to JSON (present when `status` is `completed`).
    pub result: Option<String>,
    /// Heap snapshot key produced after execution.
    pub heap: Option<String>,
    /// Filesystem snapshot CA id produced after execution (when a mount was
    /// attached), independent of the heap.
    pub fs: Option<String>,
    /// Error message (present when `status` is `failed`).
    pub error: Option<String>,
    /// ISO-8601 timestamp when execution started.
    pub started_at: String,
    /// ISO-8601 timestamp when execution finished (absent while running).
    pub completed_at: Option<String>,
}

/// A page of console output from an execution.

pub struct ExecutionOutput {
    pub execution_id: String,
    /// Text content for the requested window.
    pub data: String,
    /// First line number in this page (0-indexed).
    pub start_line: u64,
    /// Last line number in this page (exclusive).
    pub end_line: u64,
    /// Line offset to use for the next page (pass as `line_offset`).
    pub next_line_offset: u64,
    /// Total lines written so far.
    pub total_lines: u64,
    /// First byte offset in this page.
    pub start_byte: u64,
    /// Last byte offset in this page (exclusive).
    pub end_byte: u64,
    /// Byte offset to use for the next page (pass as `byte_offset`).
    pub next_byte_offset: u64,
    /// Total bytes written so far.
    pub total_bytes: u64,
    /// Whether more output is available beyond this page.
    pub has_more: bool,
    /// Execution status at the time of this query.
    pub status: String,
}

/// A brief summary of a single execution (used in list responses).

pub struct ExecutionSummary {
    pub execution_id: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// List of execution summaries.

pub struct ExecutionList {
    pub executions: Vec<serde_json::Value>,
}

/// Result of a cancel request.

pub struct CancelResult {
    pub ok: bool,
    "Option::is_none"
    pub error: Option<String>,
}

/// A single entry in the CLI download index.

pub struct CliAsset {
    /// Platform identifier (e.g. `linux-x86_64`).
    pub platform: String,
    /// Download URL for this binary via the server itself.
    pub url: String,
    /// Whether the binary is embedded in this server build.
    pub available: bool,
}

/// Index of available CLI binary downloads for the running server version.

pub struct CliIndex {
    /// Server (and CLI) version string, e.g. `"0.1.0"`.
    pub version: String,
    /// Available platform binaries.
    pub assets: Vec<CliAsset>,
}

/// Generic error body.

pub struct ApiError {
    pub error: String,
}


/// Optional pagination query parameters for console output.

pub struct OutputQuery {
    /// Return output starting at this line number (0-indexed).
    
    pub line_offset: Option<u64>,
    /// Maximum number of lines to return.
    
    pub line_limit: Option<u64>,
    /// Return output starting at this byte offset.
    
    pub byte_offset: Option<u64>,
    /// Maximum number of bytes to return.
    
    pub byte_limit: Option<u64>,
}

/// Optional query parameters for a label reflog read.

pub struct FsLogQuery {
    /// Return only the most recent N reflog entries (oldest-first). Omit for the full history.
    
    pub limit: Option<usize>,
}



"mcp-v8""0.1.0""HTTP API for the mcp-v8 JavaScript execution server"
pub struct ApiDoc;


/// Submit JavaScript code for asynchronous execution.
///
/// Returns immediately with an `execution_id`. Use `GET /api/executions/{id}`
/// to poll status and `GET /api/executions/{id}/output` to read console output.
///
/// Two request encodings are accepted, selected by `Content-Type`:
/// - `application/json` (or no `Content-Type`): a JSON `ExecRequest` body (the
///   schema below).
/// - any other type (e.g. `application/javascript`, `text/plain`): the raw
///   request body is taken as the script source — i.e. a file upload (`curl
///   --data-binary @script.js`). Optional `heap`, `session`,
///   `heap_memory_max_mb`, and `execution_timeout_secs` may be passed as
///   query-string parameters.
"/api/exec""Execution queued""Malformed request body""Unsupported Content-Type (e.g. multipart/form-data)""Internal error""executions"
async fn exec_handler(
    State(engine): State<Engine>,
    Query(params): Query<ExecUploadParams>,
    request: Request,
) -> (StatusCode, Json<serde_json::Value>) {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

            if content_type.starts_with("multipart/form-data") {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(serde_json::json!({
                "error": "multipart/form-data is not supported; upload the script as the raw request body with a non-JSON Content-Type (e.g. application/javascript), or send a JSON body"
            })),
        );
    }

    let bytes = match axum::body::to_bytes(request.into_body(), MAX_EXEC_BODY_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("failed to read request body: {}", e) })),
            )
        }
    };

            let exec_req = if content_type.is_empty() || content_type.contains("json") {
        match serde_json::from_slice::<ExecRequest>(&bytes) {
            Ok(req) => req,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("invalid JSON body: {}", e) })),
                )
            }
        }
    } else {
        let code = match String::from_utf8(bytes.to_vec()) {
            Ok(s) => s,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("request body is not valid UTF-8: {}", e) })),
                )
            }
        };
        ExecRequest {
            code,
            heap: params.heap,
            fs: params.fs,
            session: params.session,
            heap_memory_max_mb: params.heap_memory_max_mb,
            execution_timeout_secs: params.execution_timeout_secs,
            tags: None,
        }
    };

    submit_exec(engine, exec_req).await
}

/// Query-string parameters accepted alongside a raw-body script upload to
/// `POST /api/exec`. They mirror the optional fields of [`ExecRequest`]
/// (`tags` is only available via the JSON body).

struct ExecUploadParams {
    
    heap: Option<String>,
    
    fs: Option<String>,
    
    session: Option<String>,
    
    heap_memory_max_mb: Option<usize>,
    
    execution_timeout_secs: Option<u64>,
}

/// Queue an [`ExecRequest`] on the engine and map the result to an HTTP
/// response. Shared by the JSON and raw-upload code paths.
async fn submit_exec(
    engine: Engine,
    req: ExecRequest,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut r = engine.run_js(req.code);
    if let Some(h) = req.heap { r = r.heap(h); }
    r = r.maybe_fs(req.fs);
    if let Some(s) = req.session { r = r.session(s); }
    if let Some(mb) = req.heap_memory_max_mb { r = r.heap_memory_max_mb(mb); }
    if let Some(secs) = req.execution_timeout_secs { r = r.execution_timeout_secs(secs); }
    if let Some(t) = req.tags { r = r.tags(t); }
    match r.execute().await {
        Ok(execution_id) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "execution_id": execution_id })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ),
    }
}

/// Get the status and result of an execution.
"/api/executions/{id}""id""Execution ID returned by POST /api/exec""Execution found""Execution not found""executions"
async fn get_execution_handler(
    State(engine): State<Engine>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.get_execution(&id) {
        Ok(info) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "execution_id": info.id,
                "status": info.status,
                "result": info.result,
                "heap": info.heap,
                "fs": info.fs,
                "error": info.error,
                "started_at": info.started_at,
                "completed_at": info.completed_at,
            })),
        ),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e })),
        ),
    }
}

/// Read paginated console output from an execution.
///
/// Supports both line-based (`line_offset` / `line_limit`) and byte-based
/// (`byte_offset` / `byte_limit`) pagination.  Use `has_more` and
/// `next_line_offset` / `next_byte_offset` to iterate.
"/api/executions/{id}/output""id""Execution ID""Output page""Execution not found""executions"
async fn get_execution_output_handler(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    Query(query): Query<OutputQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let status = engine.get_execution(&id)
        .map(|info| info.status)
        .unwrap_or_else(|_| "unknown".to_string());

    match engine.get_execution_output(&id, query.line_offset, query.line_limit, query.byte_offset, query.byte_limit) {
        Ok(page) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "execution_id": id,
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
            })),
        ),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e })),
        ),
    }
}

/// Cancel a running execution.
"/api/executions/{id}/cancel""id""Execution ID to cancel""Cancel accepted""Cannot cancel (e.g. already finished)""executions"
async fn cancel_execution_handler(
    State(engine): State<Engine>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.cancel_execution(&id) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true })),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": e })),
        ),
    }
}

/// List all known executions (running and recently completed).
"/api/executions""Execution list""Internal error""executions"
async fn list_executions_handler(
    State(engine): State<Engine>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.list_executions() {
        Ok(executions) => (
            StatusCode::OK,
            Json(serde_json::json!({ "executions": executions })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ),
    }
}


/// List available CLI binary downloads for the running server version.
///
/// Each `url` is a direct download from this server. `available: false` means
/// the binary was not embedded at build time (dev/local builds).
"/api/cli""CLI download index""cli"
async fn cli_index_handler(
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    let assets: Vec<_> = PLATFORMS
        .iter()
        .map(|p| serde_json::json!({
            "platform":  p.platform,
            "url":       format!("http://{}/api/cli/{}", host, p.platform),
            "available": !p.bytes.is_empty(),
        }))
        .collect();

    Json(serde_json::json!({
        "version": SERVER_VERSION,
        "assets":  assets,
    }))
}

/// Download the CLI binary for a specific platform directly from this server.
///
/// The binary is embedded at build time and always matches the running server
/// version. Returns 404 if the server was built without embedded binaries
/// (dev/local builds).
///
/// Supported platforms: `linux-x86_64`, `linux-aarch64`, `macos-aarch64`.
"/api/cli/{platform}""platform""Target platform (linux-x86_64 | linux-aarch64 | macos-aarch64)""CLI binary (application/octet-stream)""Unknown platform or binary not embedded""cli"
async fn cli_download_handler(
    Path(platform): Path<String>,
) -> Response {
    match find_platform(&platform) {
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "Unknown platform '{}'. Valid platforms: {}",
                    platform,
                    PLATFORMS.iter().map(|p| p.platform).collect::<Vec<_>>().join(", ")
                )
            })),
        ).into_response(),

        Some(p) if p.bytes.is_empty() => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "CLI binary for '{}' is not embedded in this build. \
                     Set MCP_V8_CLI_{} at build time to embed it.",
                    platform,
                    platform.to_uppercase().replace('-', "_")
                )
            })),
        ).into_response(),

        Some(p) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE,        "application/octet-stream"),
                (header::CONTENT_DISPOSITION, &format!("attachment; filename=\"{}\"", p.filename)),
                (header::CONTENT_LENGTH,      &p.bytes.len().to_string()),
            ],
            p.bytes,
        ).into_response(),
    }
}


/// Return the running server version.
"/api/version""Server version""meta"
async fn version_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "version": SERVER_VERSION }))
}


/// Redirect / → /llms.txt so agents that follow RFC 7231 redirects land on
/// the machine-readable guide immediately.
async fn root_redirect_handler() -> Response {
    axum::response::Redirect::permanent("/llms.txt").into_response()
}

/// Serve the embedded llms.txt (https://llmstxt.org/) as plain Markdown.
/// Agents can fetch this to understand the API, available MCP tools, and
/// how to connect before making any other requests.
async fn llms_txt_handler() -> Response {
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/markdown; charset=utf-8")
        .header("X-Content-Type-Options", "nosniff")
        .body(axum::body::Body::from(LLMS_TXT))
        .unwrap()
}

/// Serve the full README as Markdown at /docs.
/// Useful for agents that want deep context before exploring the API.
async fn docs_handler() -> Response {
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/markdown; charset=utf-8")
        .header("X-Content-Type-Options", "nosniff")
        .body(axum::body::Body::from(README_MD))
        .unwrap()
}


/// Request body for advancing a filesystem snapshot label (`POST /api/fs/push`).

pub struct FsPushRequest {
    /// The CA id (hex) to point the label at — typically the `fs` value from a
    /// completed execution.
    pub ca_id: String,
    /// Label to advance. Omit only when `detach` is true.
    
    pub label: Option<String>,
    /// The head the caller pulled. The push is rejected if the label has moved
    /// since (reject-and-rebase). Ignored when `force` is true.
    
    pub expected: Option<String>,
    /// Override the conflict check and move the label unconditionally.
    
    pub force: bool,
    /// Do not touch any label; just echo the CA id back.
    
    pub detach: bool,
    /// Optional human note recorded on the reflog entry, like a commit message.
    
    pub message: Option<String>,
}

/// Request body for `POST /api/fs/labels` (create or repoint a label).

pub struct FsLabelRequest {
    pub name: String,
    pub ca_id: String,
    /// Optional human note recorded on the reflog entry, like a commit message.
    
    pub message: Option<String>,
}

/// Request body for `POST /api/fs/reset`.

pub struct FsResetRequest {
    pub label: String,
    pub ca_id: String,
    /// Allow resetting to a CA id that is not in the label's reflog.
    
    pub allow_unlogged: bool,
    /// Optional human note recorded on the reflog entry, like a commit message.
    
    pub message: Option<String>,
}

/// List filesystem snapshot labels.
"/api/fs/labels""Labels and their head CA ids""fs"
async fn fs_labels_handler(
    State(engine): State<Engine>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.fs_list_labels().await {
        Ok(labels) => (StatusCode::OK, Json(serde_json::json!({ "labels": labels }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))),
    }
}

/// Create or repoint a filesystem snapshot label.
"/api/fs/labels""Label set""fs"
async fn fs_set_label_handler(
    State(engine): State<Engine>,
    Json(req): Json<FsLabelRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.fs_set_label(&req.name, &req.ca_id, req.message).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "label": req.name, "ca_id": req.ca_id })),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))),
    }
}

/// Resolve a label to its current head CA id.
"/api/fs/labels/{label}""label""Label name""Current head CA id""Unknown label""fs"
async fn fs_resolve_handler(
    State(engine): State<Engine>,
    Path(label): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.fs_resolve_label(&label).await {
        Ok(Some(ca_id)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "label": label, "ca_id": ca_id })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("unknown label: {label}") })),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))),
    }
}

/// Show the reflog for a label.
"/api/fs/labels/{label}/log""label""Label name""Reflog entries, oldest first""fs"
async fn fs_log_handler(
    State(engine): State<Engine>,
    Path(label): Path<String>,
    Query(query): Query<FsLogQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.fs_label_log(&label, query.limit).await {
        Ok(log) => (
            StatusCode::OK,
            Json(serde_json::json!({ "label": label, "log": log })),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))),
    }
}

/// Advance a label to a CA id (reject-and-rebase by default).
"/api/fs/push""Push advanced the label""Rejected — the label moved since the caller pulled""fs"
async fn fs_push_handler(
    State(engine): State<Engine>,
    Json(req): Json<FsPushRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if req.detach {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "detached", "ca_id": req.ca_id })),
        );
    }
    let Some(label) = req.label else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "fs push requires a label unless detach is true" })),
        );
    };
    match engine.fs_push(&label, &req.ca_id, req.expected, req.force, req.message).await {
        Ok(outcome) => {
            let value = serde_json::to_value(&outcome).unwrap_or_default();
            let is_rejected = matches!(outcome, crate::engine::FsPushOutcome::Rejected { .. });
            let code = if is_rejected { StatusCode::CONFLICT } else { StatusCode::OK };
            (code, Json(value))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))),
    }
}

/// Reset a label to an earlier CA id from its reflog.
"/api/fs/reset""Label reset""CA id not in reflog (and allow_unlogged not set)""fs"
async fn fs_reset_handler(
    State(engine): State<Engine>,
    Json(req): Json<FsResetRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.fs_reset(&req.label, &req.ca_id, req.allow_unlogged, req.message).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "label": req.label, "ca_id": req.ca_id })),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))),
    }
}

/// Request body for `POST /api/fs/merge`.

pub struct FsMergeRequest {
    /// One side of the merge (CA id, e.g. an execution's `fs` result).
    pub ours: String,
    /// The other side (CA id).
    pub theirs: String,
    /// The common ancestor both sides diverged from. Omit for a 2-way merge.
    
    pub base: Option<String>,
    /// `ours` or `theirs` to auto-resolve conflicts; omit to report them.
    
    pub prefer: Option<String>,
}

/// Three-way merge two snapshots into a new one.
"/api/fs/merge""Merge ran — body has status=merged (ca_id) or status=conflict. Text files auto-merge at line level; each conflict carries kind plus, for text, diff3 markers and unified diffs.""Invalid CA id or prefer value""fs"
async fn fs_merge_handler(
    State(engine): State<Engine>,
    Json(req): Json<FsMergeRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let prefer = match crate::engine::fs_merge::Prefer::parse(req.prefer.as_deref()) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))),
    };
    match engine.fs_merge(&req.ours, &req.theirs, req.base, prefer).await {
                        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(&result).unwrap_or_default())),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))),
    }
}


/// Build the plain Axum router (no OpenAPI metadata attached).
///
/// Used when running in stdio mode where no HTTP server is present, and
/// for merging into SSE / Streamable-HTTP transport servers.
pub fn api_router(engine: Engine) -> Router {
    Router::new()
        .route("/", get(root_redirect_handler))
        .route("/llms.txt", get(llms_txt_handler))
        .route("/docs", get(docs_handler))
        .route("/api/version", get(version_handler))
        .route("/api/exec", post(exec_handler))
        .route("/api/executions", get(list_executions_handler))
        .route("/api/executions/{id}", get(get_execution_handler))
        .route("/api/executions/{id}/output", get(get_execution_output_handler))
        .route("/api/executions/{id}/cancel", post(cancel_execution_handler))
        .route("/api/cli", get(cli_index_handler))
        .route("/api/cli/{platform}", get(cli_download_handler))
        .route("/api/fs/labels", get(fs_labels_handler).post(fs_set_label_handler))
        .route("/api/fs/labels/{label}", get(fs_resolve_handler))
        .route("/api/fs/labels/{label}/log", get(fs_log_handler))
        .route("/api/fs/push", post(fs_push_handler))
        .route("/api/fs/reset", post(fs_reset_handler))
        .route("/api/fs/merge", post(fs_merge_handler))
        .with_state(engine)
}
