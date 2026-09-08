use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};

/// Maximum size of an `/api/exec` request body (16 MiB), for both the JSON
/// body and raw script uploads.
const MAX_EXEC_BODY_BYTES: usize = 16 * 1024 * 1024;

use crate::engine::fs_merge::Prefer;
use crate::engine::FsPushOutcome;
use crate::engine::{Engine, ExecutionRequest, FsEntryKind, FsErrorKind, FsView, RuntimeError};

// ── Embedded agent-discovery content ─────────────────────────────────

/// llms.txt — machine-readable guide for AI agents (https://llmstxt.org/)
const LLMS_TXT: &str = include_str!("llms_txt.md");

/// Full README for the /docs endpoint
const README_MD: &str = include_str!("../README.md");

// ── CLI download helpers ──────────────────────────────────────────────

/// The version of this server binary, from Cargo.toml at compile time.
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// Embedded CLI binaries — populated at compile time by build.rs.
// Each is an empty slice in dev builds (no MCP_V8_CLI_* env vars set).
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
// ── Request / Response types ─────────────────────────────────────────────

/// Request body for executing JavaScript code.
#[derive(Deserialize, ToSchema)]
pub struct ExecRequest {
    /// JavaScript (or TypeScript) source code to execute.
    pub code: String,
    /// Serialised heap snapshot key to restore before execution.
    #[serde(default)]
    pub heap: Option<String>,
    /// Filesystem snapshot handle to mount: a label name or 64-hex CA id.
    /// Independent of `heap`.
    #[serde(default)]
    pub fs: Option<String>,
    /// Session identifier used for tagging / logging.
    #[serde(default)]
    pub session: Option<String>,
    /// Per-execution V8 heap memory cap in megabytes.
    #[serde(default)]
    pub heap_memory_max_mb: Option<usize>,
    /// Per-execution timeout in seconds (overrides server default).
    #[serde(default)]
    pub execution_timeout_secs: Option<u64>,
    /// Arbitrary key/value tags attached to the resulting heap snapshot.
    #[serde(default)]
    pub tags: Option<HashMap<String, String>>,
}

/// Accepted response containing the new execution's ID.
#[derive(Serialize, ToSchema)]
pub struct ExecAccepted {
    /// Unique identifier for the queued execution.
    pub execution_id: String,
}

/// Detailed status of a single execution.
#[derive(Serialize, ToSchema)]
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
    /// Artifacts emitted via `artifact(key, mime, bytes)` during this
    /// execution. Fetch payloads with `GET /api/artifacts/{key}`.
    pub artifacts: Vec<ArtifactMeta>,
}

/// Metadata for one stored artifact (payload not included).
#[derive(Serialize, ToSchema)]
pub struct ArtifactMeta {
    /// Caller-chosen key, as passed to `artifact(key, mime, bytes)`.
    pub key: String,
    /// Mime type, e.g. `image/png`.
    pub mime_type: String,
    /// Payload size in bytes.
    pub size_bytes: u64,
    /// ISO-8601 timestamp when the artifact was (last) written.
    pub created_at: String,
    /// Execution that (last) wrote this artifact, when known.
    pub execution_id: Option<String>,
}

/// List of stored artifacts.
#[derive(Serialize, ToSchema)]
pub struct ArtifactList {
    pub artifacts: Vec<ArtifactMeta>,
}

/// A page of console output from an execution.
#[derive(Serialize, ToSchema)]
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
#[derive(Serialize, ToSchema)]
pub struct ExecutionSummary {
    pub execution_id: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// List of execution summaries.
#[derive(Serialize, ToSchema)]
pub struct ExecutionList {
    pub executions: Vec<serde_json::Value>,
}

/// Result of a cancel request.
#[derive(Serialize, ToSchema)]
pub struct CancelResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A single entry in the CLI download index.
#[derive(Serialize, ToSchema)]
pub struct CliAsset {
    /// Platform identifier (e.g. `linux-x86_64`).
    pub platform: String,
    /// Download URL for this binary via the server itself.
    pub url: String,
    /// Whether the binary is embedded in this server build.
    pub available: bool,
}

/// Index of available CLI binary downloads for the running server version.
#[derive(Serialize, ToSchema)]
pub struct CliIndex {
    /// Server (and CLI) version string, e.g. `"0.1.0"`.
    pub version: String,
    /// Available platform binaries.
    pub assets: Vec<CliAsset>,
}

/// Generic error body.
#[derive(Serialize, ToSchema)]
pub struct ApiError {
    pub error: String,
}

// ── Query params ─────────────────────────────────────────────────────────

/// Optional pagination query parameters for console output.
#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
pub struct OutputQuery {
    /// Return output starting at this line number (0-indexed).
    #[serde(default)]
    pub line_offset: Option<u64>,
    /// Maximum number of lines to return.
    #[serde(default)]
    pub line_limit: Option<u64>,
    /// Return output starting at this byte offset.
    #[serde(default)]
    pub byte_offset: Option<u64>,
    /// Maximum number of bytes to return.
    #[serde(default)]
    pub byte_limit: Option<u64>,
}

/// Optional query parameters for a label reflog read.
#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
pub struct FsLogQuery {
    /// Return only the most recent N reflog entries (oldest-first). Omit for the full history.
    #[serde(default)]
    pub limit: Option<usize>,
}

// ── OpenAPI document ─────────────────────────────────────────────────────

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mcp-v8",
        version = "0.1.0",
        description = "HTTP API for the mcp-v8 JavaScript execution server"
    ),
    paths(
        version_handler,
        exec_handler,
        list_executions_handler,
        get_execution_handler,
        get_execution_output_handler,
        cancel_execution_handler,
        list_artifacts_handler,
        get_artifact_handler,
        cli_index_handler,
        cli_download_handler,
        fs_labels_handler,
        fs_set_label_handler,
        fs_resolve_handler,
        fs_log_handler,
        fs_push_handler,
        fs_reset_handler,
        fs_merge_handler,
        capabilities_handler,
        session_file_get_handler,
        session_file_put_handler,
        session_file_delete_handler,
        session_entry_handler,
        session_dir_handler,
        session_fs_op_handler,
        session_snapshots_handler,
    ),
    components(schemas(
        ExecRequest,
        ExecAccepted,
        ExecutionInfo,
        ExecutionOutput,
        ExecutionList,
        ExecutionSummary,
        ArtifactMeta,
        ArtifactList,
        CancelResult,
        ApiError,
        OutputQuery,
        FsLogQuery,
        CliAsset,
        CliIndex,
        FsPushRequest,
        FsLabelRequest,
        FsResetRequest,
        FsMergeRequest,
        SessionFileReadQuery,
        SessionFileWriteQuery,
        SessionFileRemoveQuery,
        SessionEntryQuery,
        SessionFileEntry,
        SessionDirListing,
        SessionFsOpRequest,
        SessionSnapshotEntry,
        Capabilities,
    ))
)]
pub struct ApiDoc;

// ── Handlers ─────────────────────────────────────────────────────────────

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
#[utoipa::path(
    post,
    path = "/api/exec",
    request_body = ExecRequest,
    responses(
        (status = 202, description = "Execution queued", body = ExecAccepted),
        (status = 400, description = "Malformed request body", body = ApiError),
        (status = 415, description = "Unsupported Content-Type (e.g. multipart/form-data)", body = ApiError),
        (status = 500, description = "Internal error", body = ApiError),
    ),
    tag = "executions"
)]
async fn exec_handler(
    State(runtime): State<Arc<Engine>>,
    Query(params): Query<ExecUploadParams>,
    request: Request,
) -> (StatusCode, Json<serde_json::Value>) {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // multipart/form-data would require a multipart-parser dependency; steer
    // callers to the simpler raw-body upload instead.
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

    // JSON (or no Content-Type) → structured body; anything else → the raw body
    // is the script source, with optional params taken from the query string.
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

    submit_exec(runtime, exec_req).await
}

/// Query-string parameters accepted alongside a raw-body script upload to
/// `POST /api/exec`. They mirror the optional fields of [`ExecRequest`]
/// (`tags` is only available via the JSON body).
#[derive(Deserialize)]
struct ExecUploadParams {
    #[serde(default)]
    heap: Option<String>,
    #[serde(default)]
    fs: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    heap_memory_max_mb: Option<usize>,
    #[serde(default)]
    execution_timeout_secs: Option<u64>,
}

/// Queue an [`ExecRequest`] on the engine and map the result to an HTTP
/// response. Shared by the JSON and raw-upload code paths.
async fn submit_exec(
    runtime: Arc<Engine>,
    req: ExecRequest,
) -> (StatusCode, Json<serde_json::Value>) {
    let request = ExecutionRequest {
        code: req.code,
        file: None,
        heap: req.heap,
        fs: req.fs,
        session: req.session,
        heap_memory_max_mb: req.heap_memory_max_mb.map(|value| value as u64),
        execution_timeout_secs: req.execution_timeout_secs,
        tags: req.tags,
        mcp_headers: None,
    };
    match runtime.submit_execution(request).await {
        Ok(execution_id) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "execution_id": execution_id })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// Get the status and result of an execution.
#[utoipa::path(
    get,
    path = "/api/executions/{id}",
    params(
        ("id" = String, Path, description = "Execution ID returned by POST /api/exec")
    ),
    responses(
        (status = 200, description = "Execution found", body = ExecutionInfo),
        (status = 404, description = "Execution not found", body = ApiError),
    ),
    tag = "executions"
)]
async fn get_execution_handler(
    State(runtime): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match runtime.get_execution(id.clone()) {
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
                "artifacts": info.artifacts,
            })),
        ),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// List metadata for all stored artifacts.
#[utoipa::path(
    get,
    path = "/api/artifacts",
    responses(
        (status = 200, description = "Artifact metadata list", body = ArtifactList),
        (status = 500, description = "Artifact store unavailable", body = ApiError),
    ),
    tag = "artifacts"
)]
async fn list_artifacts_handler(
    State(engine): State<Arc<Engine>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine.list_artifacts() {
        Ok(artifacts) => (
            StatusCode::OK,
            Json(serde_json::json!({ "artifacts": artifacts })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ),
    }
}

/// Download an artifact's raw payload bytes.
///
/// The response body is the artifact's payload verbatim, served with the
/// stored mime type as `Content-Type` — no base64, unlike the MCP tool.
#[utoipa::path(
    get,
    path = "/api/artifacts/{key}",
    params(
        ("key" = String, Path, description = "Artifact key, as passed to artifact(key, mime, bytes)")
    ),
    responses(
        (status = 200, description = "Raw artifact bytes (Content-Type = stored mime type)"),
        (status = 404, description = "Artifact not found", body = ApiError),
    ),
    tag = "artifacts"
)]
async fn get_artifact_handler(
    State(engine): State<Arc<Engine>>,
    Path(key): Path<String>,
) -> Response {
    match engine.get_artifact(&key) {
        // nosniff + attachment: the payload and its Content-Type are script-
        // controlled, so keep browsers from rendering active content (e.g.
        // text/html) in the API's origin.
        Ok(artifact) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, artifact.meta.mime_type),
                (header::CONTENT_DISPOSITION, "attachment".to_string()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            artifact.bytes,
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// Read paginated console output from an execution.
///
/// Supports both line-based (`line_offset` / `line_limit`) and byte-based
/// (`byte_offset` / `byte_limit`) pagination.  Use `has_more` and
/// `next_line_offset` / `next_byte_offset` to iterate.
#[utoipa::path(
    get,
    path = "/api/executions/{id}/output",
    params(
        ("id" = String, Path, description = "Execution ID"),
        OutputQuery,
    ),
    responses(
        (status = 200, description = "Output page", body = ExecutionOutput),
        (status = 404, description = "Execution not found", body = ApiError),
    ),
    tag = "executions"
)]
async fn get_execution_output_handler(
    State(runtime): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(query): Query<OutputQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let status = runtime.get_execution(id.clone())
        .map(|info| info.status)
        .unwrap_or_else(|_| "unknown".to_string());

    match runtime.get_execution_output(id.clone(), query.line_offset, query.line_limit, query.byte_offset, query.byte_limit) {
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
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// Cancel a running execution.
#[utoipa::path(
    post,
    path = "/api/executions/{id}/cancel",
    params(
        ("id" = String, Path, description = "Execution ID to cancel")
    ),
    responses(
        (status = 200, description = "Cancel accepted", body = CancelResult),
        (status = 400, description = "Cannot cancel (e.g. already finished)", body = CancelResult),
    ),
    tag = "executions"
)]
async fn cancel_execution_handler(
    State(runtime): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match runtime.cancel_execution(id) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true })),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        ),
    }
}

/// List all known executions (running and recently completed).
#[utoipa::path(
    get,
    path = "/api/executions",
    responses(
        (status = 200, description = "Execution list", body = ExecutionList),
        (status = 500, description = "Internal error", body = ApiError),
    ),
    tag = "executions"
)]
async fn list_executions_handler(
    State(runtime): State<Arc<Engine>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match runtime.list_executions() {
        Ok(executions) => (
            StatusCode::OK,
            Json(serde_json::json!({ "executions": executions })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

// ── CLI download endpoints ────────────────────────────────────────────────

/// List available CLI binary downloads for the running server version.
///
/// Each `url` is a direct download from this server. `available: false` means
/// the binary was not embedded at build time (dev/local builds).
#[utoipa::path(
    get,
    path = "/api/cli",
    responses(
        (status = 200, description = "CLI download index", body = CliIndex),
    ),
    tag = "cli"
)]
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
#[utoipa::path(
    get,
    path = "/api/cli/{platform}",
    params(
        ("platform" = String, Path, description = "Target platform (linux-x86_64 | linux-aarch64 | macos-aarch64)"),
    ),
    responses(
        (status = 200, description = "CLI binary (application/octet-stream)"),
        (status = 404, description = "Unknown platform or binary not embedded", body = ApiError),
    ),
    tag = "cli"
)]
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

// ── Version endpoint ──────────────────────────────────────────────────────

/// Return the running server version.
#[utoipa::path(
    get,
    path = "/api/version",
    responses(
        (status = 200, description = "Server version"),
    ),
    tag = "meta"
)]
async fn version_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "version": SERVER_VERSION }))
}

// ── Agent-discovery endpoints ─────────────────────────────────────────────

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

// ── fs snapshot endpoints ─────────────────────────────────────────────────

/// Request body for advancing a filesystem snapshot label (`POST /api/fs/push`).
#[derive(Deserialize, ToSchema)]
pub struct FsPushRequest {
    /// The CA id (hex) to point the label at — typically the `fs` value from a
    /// completed execution.
    pub ca_id: String,
    /// Label to advance. Omit only when `detach` is true.
    #[serde(default)]
    pub label: Option<String>,
    /// The head the caller pulled. The push is rejected if the label has moved
    /// since (reject-and-rebase). Ignored when `force` is true.
    #[serde(default)]
    pub expected: Option<String>,
    /// Override the conflict check and move the label unconditionally.
    #[serde(default)]
    pub force: bool,
    /// Do not touch any label; just echo the CA id back.
    #[serde(default)]
    pub detach: bool,
    /// Optional human note recorded on the reflog entry, like a commit message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Request body for `POST /api/fs/labels` (create or repoint a label).
#[derive(Deserialize, ToSchema)]
pub struct FsLabelRequest {
    pub name: String,
    pub ca_id: String,
    /// Optional human note recorded on the reflog entry, like a commit message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Request body for `POST /api/fs/reset`.
#[derive(Deserialize, ToSchema)]
pub struct FsResetRequest {
    pub label: String,
    pub ca_id: String,
    /// Allow resetting to a CA id that is not in the label's reflog.
    #[serde(default)]
    pub allow_unlogged: bool,
    /// Optional human note recorded on the reflog entry, like a commit message.
    #[serde(default)]
    pub message: Option<String>,
}

/// List filesystem snapshot labels.
#[utoipa::path(
    get,
    path = "/api/fs/labels",
    responses((status = 200, description = "Labels and their head CA ids")),
    tag = "fs"
)]
async fn fs_labels_handler(
    State(runtime): State<Arc<Engine>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match runtime.fs_list_labels().await {
        Ok(labels) => (StatusCode::OK, Json(serde_json::json!({ "labels": labels }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e.to_string() }))),
    }
}

/// Create or repoint a filesystem snapshot label.
#[utoipa::path(
    post,
    path = "/api/fs/labels",
    request_body = FsLabelRequest,
    responses((status = 200, description = "Label set")),
    tag = "fs"
)]
async fn fs_set_label_handler(
    State(runtime): State<Arc<Engine>>,
    Json(req): Json<FsLabelRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match runtime.fs_set_label(req.name.clone(), req.ca_id.clone(), req.message).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "label": req.name, "ca_id": req.ca_id })),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e.to_string() }))),
    }
}

/// Resolve a label to its current head CA id.
#[utoipa::path(
    get,
    path = "/api/fs/labels/{label}",
    params(("label" = String, Path, description = "Label name")),
    responses(
        (status = 200, description = "Current head CA id"),
        (status = 404, description = "Unknown label"),
    ),
    tag = "fs"
)]
async fn fs_resolve_handler(
    State(runtime): State<Arc<Engine>>,
    Path(label): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match runtime.fs_resolve_label(label.clone()).await {
        Ok(Some(ca_id)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "label": label, "ca_id": ca_id })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("unknown label: {label}") })),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e.to_string() }))),
    }
}

/// Show the reflog for a label.
#[utoipa::path(
    get,
    path = "/api/fs/labels/{label}/log",
    params(
        ("label" = String, Path, description = "Label name"),
        FsLogQuery,
    ),
    responses((status = 200, description = "Reflog entries, oldest first")),
    tag = "fs"
)]
async fn fs_log_handler(
    State(runtime): State<Arc<Engine>>,
    Path(label): Path<String>,
    Query(query): Query<FsLogQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    match runtime.fs_label_log(label.clone(), query.limit.map(|limit| limit as u64)).await {
        Ok(log) => (
            StatusCode::OK,
            Json(serde_json::json!({ "label": label, "log": log })),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e.to_string() }))),
    }
}

/// Advance a label to a CA id (reject-and-rebase by default).
#[utoipa::path(
    post,
    path = "/api/fs/push",
    request_body = FsPushRequest,
    responses(
        (status = 200, description = "Push advanced the label"),
        (status = 409, description = "Rejected — the label moved since the caller pulled"),
    ),
    tag = "fs"
)]
async fn fs_push_handler(
    State(runtime): State<Arc<Engine>>,
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
    match runtime.fs_push(label, req.ca_id, req.expected, req.force, req.message).await {
        Ok(outcome) => {
            let code = match &outcome {
                FsPushOutcome::Advanced { .. } => StatusCode::OK,
                FsPushOutcome::Rejected { .. } => StatusCode::CONFLICT,
            };
            (code, Json(serde_json::json!(outcome)))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e.to_string() }))),
    }
}

/// Reset a label to an earlier CA id from its reflog.
#[utoipa::path(
    post,
    path = "/api/fs/reset",
    request_body = FsResetRequest,
    responses(
        (status = 200, description = "Label reset"),
        (status = 400, description = "CA id not in reflog (and allow_unlogged not set)"),
    ),
    tag = "fs"
)]
async fn fs_reset_handler(
    State(runtime): State<Arc<Engine>>,
    Json(req): Json<FsResetRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match runtime.fs_reset(req.label.clone(), req.ca_id.clone(), req.allow_unlogged, req.message).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "label": req.label, "ca_id": req.ca_id })),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e.to_string() }))),
    }
}

/// Request body for `POST /api/fs/merge`.
#[derive(Deserialize, ToSchema)]
pub struct FsMergeRequest {
    /// One side of the merge (CA id, e.g. an execution's `fs` result).
    pub ours: String,
    /// The other side (CA id).
    pub theirs: String,
    /// The common ancestor both sides diverged from. Omit for a 2-way merge.
    #[serde(default)]
    pub base: Option<String>,
    /// `ours` or `theirs` to auto-resolve conflicts; omit to report them.
    #[serde(default)]
    pub prefer: Option<String>,
}

/// Three-way merge two snapshots into a new one.
#[utoipa::path(
    post,
    path = "/api/fs/merge",
    request_body = FsMergeRequest,
    responses(
        (status = 200, description = "Merge ran — body has status=merged (ca_id) or status=conflict. Text files auto-merge at line level; each conflict carries kind plus, for text, diff3 markers and unified diffs."),
        (status = 400, description = "Invalid CA id or prefer value"),
    ),
    tag = "fs"
)]
async fn fs_merge_handler(
    State(runtime): State<Arc<Engine>>,
    Json(req): Json<FsMergeRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let prefer = match req.prefer.as_deref() {
        None => Prefer::None,
        Some("ours") => Prefer::Ours,
        Some("theirs") => Prefer::Theirs,
        Some(value) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid prefer value: {value}; expected ours or theirs") })),
            )
        }
    };
    match runtime.fs_merge(req.ours, req.theirs, req.base, prefer).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

// ── Session file views ───────────────────────────────────────────────────
//
// The HTTP counterpart of the native `Engine::fs_view(session)`: the same
// typed operations on a session's filesystem snapshot, so a remote client can
// use one engine session for `run_js` and its file tools exactly as an
// embedded client does. Bytes travel as raw request/response bodies.

/// Query for `GET /api/sessions/{session}/files/{path}`.
#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
pub struct SessionFileReadQuery {
    /// Start reading at this byte offset (with `max_bytes`).
    #[serde(default)]
    pub offset: Option<u64>,
    /// Read at most this many bytes; fewer are returned only at end of file.
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

/// Query for `PUT /api/sessions/{session}/files/{path}`.
#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
pub struct SessionFileWriteQuery {
    /// Append to the file instead of replacing it.
    #[serde(default)]
    pub append: bool,
}

/// Query for `DELETE /api/sessions/{session}/files/{path}`.
#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
pub struct SessionFileRemoveQuery {
    /// Remove a directory and its contents.
    #[serde(default)]
    pub recursive: bool,
}

/// Query for `GET /api/sessions/{session}/entries/{path}`.
#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
pub struct SessionEntryQuery {
    /// Follow a final symlink (Node `fs.stat`); `false` is `fs.lstat`.
    #[serde(default = "default_true")]
    pub follow: bool,
}

fn default_true() -> bool {
    true
}

/// Metadata for one path in a session snapshot.
#[derive(Serialize, ToSchema)]
pub struct SessionFileEntry {
    /// `file`, `directory`, `symlink`, or `other`.
    pub kind: String,
    pub size: u64,
    pub readonly: bool,
    /// Unix mode bits, type bits included.
    pub mode: u32,
    /// Modification time in milliseconds since the Unix epoch, when known.
    pub modified_ms: Option<f64>,
}

/// Names of a directory's direct children.
#[derive(Serialize, ToSchema)]
pub struct SessionDirListing {
    pub names: Vec<String>,
}

/// Request body for `POST /api/sessions/{session}/fs`: the operations that
/// carry no file bytes.
#[derive(Deserialize, ToSchema)]
pub struct SessionFsOpRequest {
    /// `mkdir`, `rename`, `exists`, `readlink`, or `canonical`.
    pub op: String,
    pub path: String,
    /// `rename` only: the destination path.
    #[serde(default)]
    pub to: Option<String>,
    /// `mkdir` only: create missing parents.
    #[serde(default)]
    pub recursive: bool,
}

/// One session-log entry.
#[derive(Serialize, ToSchema)]
pub struct SessionSnapshotEntry {
    pub index: u64,
    pub input_heap: Option<String>,
    pub output_heap: String,
    pub output_fs: Option<String>,
    pub code: String,
    pub timestamp: String,
}

/// Runtime capabilities of this server.
#[derive(Serialize, ToSchema)]
pub struct Capabilities {
    /// Heap persistence is configured.
    pub heap: bool,
    /// Filesystem snapshots are configured.
    pub filesystem: bool,
    /// Per-session state (heap and/or filesystem) is available.
    pub sessions: bool,
}

fn snapshot_path(path: &str) -> String {
    format!("/{}", path.trim_start_matches('/'))
}

fn fs_kind_name(kind: FsErrorKind) -> &'static str {
    match kind {
        FsErrorKind::NotFound => "not_found",
        FsErrorKind::PermissionDenied => "permission_denied",
        FsErrorKind::AlreadyExists => "already_exists",
        FsErrorKind::NotDirectory => "not_directory",
        FsErrorKind::IsDirectory => "is_directory",
        FsErrorKind::NotEmpty => "not_empty",
        FsErrorKind::InvalidData => "invalid_data",
        FsErrorKind::NotSupported => "not_supported",
        FsErrorKind::Other => "other",
    }
}

/// Map a native failure to a status and a JSON body carrying the same
/// message and classification the native binding would report.
fn fs_error_response(error: RuntimeError) -> Response {
    match error {
        RuntimeError::FileSystem { kind, message } => {
            let status = match kind {
                FsErrorKind::NotFound => StatusCode::NOT_FOUND,
                FsErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
                FsErrorKind::AlreadyExists | FsErrorKind::NotEmpty => StatusCode::CONFLICT,
                FsErrorKind::NotSupported => StatusCode::NOT_IMPLEMENTED,
                _ => StatusCode::BAD_REQUEST,
            };
            (status, Json(serde_json::json!({ "error": message, "kind": fs_kind_name(kind) }))).into_response()
        }
        other => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": other.message(), "kind": "other" })),
        )
            .into_response(),
    }
}

fn session_view(runtime: Arc<Engine>, session: &str) -> Result<Arc<FsView>, Response> {
    runtime.fs_view(Some(session.to_string())).map_err(fs_error_response)
}

/// Read a file from a session's filesystem snapshot.
#[utoipa::path(
    get,
    path = "/api/sessions/{session}/files/{path}",
    params(
        ("session" = String, Path, description = "Engine session name"),
        ("path" = String, Path, description = "Path inside the snapshot, without the leading slash"),
        SessionFileReadQuery,
    ),
    responses(
        (status = 200, description = "File bytes", content_type = "application/octet-stream"),
        (status = 404, description = "Not found", body = ApiError),
        (status = 403, description = "Denied by the filesystem hook chain", body = ApiError),
    ),
    tag = "sessions"
)]
async fn session_file_get_handler(
    State(runtime): State<Arc<Engine>>,
    Path((session, path)): Path<(String, String)>,
    Query(query): Query<SessionFileReadQuery>,
) -> Response {
    let view = match session_view(runtime, &session) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let path = snapshot_path(&path);
    let bytes = match (query.offset, query.max_bytes) {
        (None, None) => view.read_file(path).await,
        (offset, max_bytes) => {
            view.read_file_range(path, offset.unwrap_or(0), max_bytes.unwrap_or(u64::MAX)).await
        }
    };
    match bytes {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(error) => fs_error_response(error),
    }
}

/// Create, replace, or append to a file in a session's filesystem snapshot.
/// The raw request body is the file content.
#[utoipa::path(
    put,
    path = "/api/sessions/{session}/files/{path}",
    params(
        ("session" = String, Path, description = "Engine session name"),
        ("path" = String, Path, description = "Path inside the snapshot, without the leading slash"),
        SessionFileWriteQuery,
    ),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 204, description = "Written"),
        (status = 403, description = "Denied by the filesystem hook chain", body = ApiError),
    ),
    tag = "sessions"
)]
async fn session_file_put_handler(
    State(runtime): State<Arc<Engine>>,
    Path((session, path)): Path<(String, String)>,
    Query(query): Query<SessionFileWriteQuery>,
    body: axum::body::Bytes,
) -> Response {
    let view = match session_view(runtime, &session) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let path = snapshot_path(&path);
    let result = if query.append {
        view.append_file(path, body.to_vec()).await
    } else {
        view.write_file(path, body.to_vec()).await
    };
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => fs_error_response(error),
    }
}

/// Remove a file or directory from a session's filesystem snapshot.
#[utoipa::path(
    delete,
    path = "/api/sessions/{session}/files/{path}",
    params(
        ("session" = String, Path, description = "Engine session name"),
        ("path" = String, Path, description = "Path inside the snapshot, without the leading slash"),
        SessionFileRemoveQuery,
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 404, description = "Not found", body = ApiError),
    ),
    tag = "sessions"
)]
async fn session_file_delete_handler(
    State(runtime): State<Arc<Engine>>,
    Path((session, path)): Path<(String, String)>,
    Query(query): Query<SessionFileRemoveQuery>,
) -> Response {
    let view = match session_view(runtime, &session) {
        Ok(view) => view,
        Err(response) => return response,
    };
    match view.remove(snapshot_path(&path), query.recursive).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => fs_error_response(error),
    }
}

/// Metadata for a path in a session's filesystem snapshot.
#[utoipa::path(
    get,
    path = "/api/sessions/{session}/entries/{path}",
    params(
        ("session" = String, Path, description = "Engine session name"),
        ("path" = String, Path, description = "Path inside the snapshot, without the leading slash"),
        SessionEntryQuery,
    ),
    responses(
        (status = 200, description = "Entry metadata", body = SessionFileEntry),
        (status = 404, description = "Not found", body = ApiError),
    ),
    tag = "sessions"
)]
async fn session_entry_handler(
    State(runtime): State<Arc<Engine>>,
    Path((session, path)): Path<(String, String)>,
    Query(query): Query<SessionEntryQuery>,
) -> Response {
    let view = match session_view(runtime, &session) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let path = snapshot_path(&path);
    let metadata = if query.follow { view.stat(path).await } else { view.lstat(path).await };
    match metadata {
        Ok(metadata) => Json(SessionFileEntry {
            kind: match metadata.kind {
                FsEntryKind::File => "file",
                FsEntryKind::Directory => "directory",
                FsEntryKind::Symlink => "symlink",
                FsEntryKind::Other => "other",
            }
            .to_string(),
            size: metadata.size,
            readonly: metadata.readonly,
            mode: metadata.mode,
            modified_ms: metadata.modified_ms,
        })
        .into_response(),
        Err(error) => fs_error_response(error),
    }
}

/// Names of a directory's direct children in a session's filesystem snapshot.
#[utoipa::path(
    get,
    path = "/api/sessions/{session}/dir/{path}",
    params(
        ("session" = String, Path, description = "Engine session name"),
        ("path" = String, Path, description = "Directory path inside the snapshot, without the leading slash"),
    ),
    responses(
        (status = 200, description = "Directory listing", body = SessionDirListing),
        (status = 404, description = "Not found", body = ApiError),
    ),
    tag = "sessions"
)]
async fn session_dir_handler(
    State(runtime): State<Arc<Engine>>,
    Path((session, path)): Path<(String, String)>,
) -> Response {
    let view = match session_view(runtime, &session) {
        Ok(view) => view,
        Err(response) => return response,
    };
    match view.read_dir(snapshot_path(&path)).await {
        Ok(names) => Json(SessionDirListing { names }).into_response(),
        Err(error) => fs_error_response(error),
    }
}

/// List the root directory of a session's filesystem snapshot.
async fn session_root_dir_handler(
    State(runtime): State<Arc<Engine>>,
    Path(session): Path<String>,
) -> Response {
    session_dir_handler(State(runtime), Path((session, String::new()))).await
}

/// Filesystem operations on a session snapshot that carry no file bytes:
/// `mkdir`, `rename`, `exists`, `readlink`, and `canonical`.
#[utoipa::path(
    post,
    path = "/api/sessions/{session}/fs",
    params(("session" = String, Path, description = "Engine session name")),
    request_body = SessionFsOpRequest,
    responses(
        (status = 200, description = "Operation result"),
        (status = 400, description = "Unknown operation", body = ApiError),
        (status = 404, description = "Not found", body = ApiError),
    ),
    tag = "sessions"
)]
async fn session_fs_op_handler(
    State(runtime): State<Arc<Engine>>,
    Path(session): Path<String>,
    Json(req): Json<SessionFsOpRequest>,
) -> Response {
    let view = match session_view(runtime, &session) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let path = snapshot_path(&req.path);
    let result = match req.op.as_str() {
        "mkdir" => view.make_dir(path, req.recursive).await.map(|()| serde_json::json!({ "ok": true })),
        "rename" => match req.to {
            Some(to) => view
                .rename(path, snapshot_path(&to))
                .await
                .map(|()| serde_json::json!({ "ok": true })),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "rename requires 'to'", "kind": "other" })),
                )
                    .into_response()
            }
        },
        "exists" => view.exists(path).await.map(|exists| serde_json::json!({ "exists": exists })),
        "readlink" => view.read_link(path).await.map(|target| serde_json::json!({ "target": target })),
        "canonical" => view.canonical_path(path).await.map(|path| serde_json::json!({ "path": path })),
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("unknown fs op: {other}"), "kind": "other" })),
            )
                .into_response()
        }
    };
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => fs_error_response(error),
    }
}

/// The session log: every run and native mutation recorded for a session,
/// oldest first, with the heap and filesystem snapshot each produced.
#[utoipa::path(
    get,
    path = "/api/sessions/{session}/snapshots",
    params(("session" = String, Path, description = "Engine session name")),
    responses(
        (status = 200, description = "Session log entries", body = Vec<SessionSnapshotEntry>),
        (status = 400, description = "Sessions are not configured", body = ApiError),
    ),
    tag = "sessions"
)]
async fn session_snapshots_handler(
    State(runtime): State<Arc<Engine>>,
    Path(session): Path<String>,
) -> Response {
    match runtime.list_session_snapshots(session).await {
        Ok(entries) => Json(
            entries
                .into_iter()
                .map(|entry| SessionSnapshotEntry {
                    index: entry.index,
                    input_heap: entry.input_heap,
                    output_heap: entry.output_heap,
                    output_fs: entry.output_fs,
                    code: entry.code,
                    timestamp: entry.timestamp,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error.message() })),
        )
            .into_response(),
    }
}

/// Which per-session state this server offers.
#[utoipa::path(
    get,
    path = "/api/capabilities",
    responses((status = 200, description = "Capabilities", body = Capabilities)),
    tag = "server"
)]
async fn capabilities_handler(State(runtime): State<Arc<Engine>>) -> Json<Capabilities> {
    let capabilities = runtime.capabilities();
    Json(Capabilities {
        heap: capabilities.heap,
        filesystem: capabilities.filesystem,
        sessions: capabilities.sessions,
    })
}

// ── Router builders ──────────────────────────────────────────────────────

/// Build the plain Axum router (no OpenAPI metadata attached).
///
/// Used when running in stdio mode where no HTTP server is present, and
/// for merging into SSE / Streamable-HTTP transport servers.
pub fn api_router(runtime: Arc<Engine>) -> Router {
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
        .route("/api/artifacts", get(list_artifacts_handler))
        .route("/api/artifacts/{key}", get(get_artifact_handler))
        .route("/api/cli", get(cli_index_handler))
        .route("/api/cli/{platform}", get(cli_download_handler))
        .route("/api/fs/labels", get(fs_labels_handler).post(fs_set_label_handler))
        .route("/api/fs/labels/{label}", get(fs_resolve_handler))
        .route("/api/fs/labels/{label}/log", get(fs_log_handler))
        .route("/api/fs/push", post(fs_push_handler))
        .route("/api/fs/reset", post(fs_reset_handler))
        .route("/api/fs/merge", post(fs_merge_handler))
        .route("/api/capabilities", get(capabilities_handler))
        .route(
            "/api/sessions/{session}/files/{*path}",
            get(session_file_get_handler)
                .put(session_file_put_handler)
                .delete(session_file_delete_handler),
        )
        .route("/api/sessions/{session}/entries/{*path}", get(session_entry_handler))
        .route("/api/sessions/{session}/dir", get(session_root_dir_handler))
        .route("/api/sessions/{session}/dir/{*path}", get(session_dir_handler))
        .route("/api/sessions/{session}/fs", post(session_fs_op_handler))
        .route("/api/sessions/{session}/snapshots", get(session_snapshots_handler))
        .with_state(runtime)
}

#[cfg(test)]
mod session_view_tests {
    use super::*;
    use crate::engine::ffi_config::{
        BlobStoreBuilder, EngineConfigBuilder, ExecutionLimitsBuilder, FilesystemAccessBuilder,
        StoreBackend,
    };
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn engine(dir: &std::path::Path) -> Arc<Engine> {
        let policy = dir.join("policy.rego");
        std::fs::write(
            &policy,
            "package mcp.filesystem\ndefault allow = false\nallow if { startswith(input.path, \"/work\") }\n",
        )
        .unwrap();
        let config = EngineConfigBuilder::new()
            .limits(
                ExecutionLimitsBuilder::new()
                    .heap_memory_max_mb(64)
                    .execution_timeout_secs(5)
                    .build()
                    .unwrap(),
            )
            .data_dir(dir.to_str().unwrap().to_string())
            .filesystem(
                FilesystemAccessBuilder::new()
                    .policies_json(
                        serde_json::json!({"policies": [{"url": format!("file://{}", policy.display())}]})
                            .to_string(),
                    )
                    .build()
                    .unwrap(),
            )
            .fs_snapshot_store(BlobStoreBuilder::new().backend(StoreBackend::Directory).build().unwrap())
            .heap_store(BlobStoreBuilder::new().backend(StoreBackend::Directory).build().unwrap())
            .build()
            .unwrap();
        Engine::create(config).unwrap()
    }

    async fn send(router: &Router, request: Request) -> (StatusCode, Vec<u8>) {
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, body)
    }

    fn request(method: &str, uri: &str, body: Body) -> Request {
        Request::builder().method(method).uri(uri).body(body).unwrap()
    }

    fn json_request(method: &str, uri: &str, value: serde_json::Value) -> Request {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn session_file_endpoints_share_the_snapshot_with_executions() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(dir.path());
        let router = api_router(engine.clone());

        let (status, body) = send(&router, request("GET", "/api/capabilities", Body::empty())).await;
        assert_eq!(status, StatusCode::OK);
        let capabilities: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(capabilities["filesystem"], true);
        assert_eq!(capabilities["heap"], true);

        // Write through HTTP, read back whole and ranged.
        let (status, _) = send(
            &router,
            request("PUT", "/api/sessions/s1/files/work/a.txt", Body::from("hello world")),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(&router, request("GET", "/api/sessions/s1/files/work/a.txt", Body::empty())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"hello world");
        let (_, body) = send(
            &router,
            request("GET", "/api/sessions/s1/files/work/a.txt?offset=6&max_bytes=3", Body::empty()),
        )
        .await;
        assert_eq!(body, b"wor");
        let (status, _) = send(
            &router,
            request("PUT", "/api/sessions/s1/files/work/a.txt?append=true", Body::from("!")),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // An execution in the same session sees the file and writes another.
        let (status, body) = send(
            &router,
            json_request(
                "POST",
                "/api/exec",
                serde_json::json!({
                    "code": "await fs.writeFile('/work/b.txt', await fs.readFile('/work/a.txt', 'utf8'))",
                    "session": "s1"
                }),
            ),
        )
        .await;
        assert!(status.is_success(), "{status}: {}", String::from_utf8_lossy(&body));
        let accepted: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = accepted["execution_id"].as_str().unwrap().to_string();
        let info = engine.clone().await_execution(id).await.unwrap();
        assert_eq!(info.status, "completed", "{:?}", info.error);

        let (status, body) = send(&router, request("GET", "/api/sessions/s1/dir/work", Body::empty())).await;
        assert_eq!(status, StatusCode::OK);
        let listing: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let mut names: Vec<&str> = listing["names"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "b.txt"]);
        let (_, body) = send(&router, request("GET", "/api/sessions/s1/files/work/b.txt", Body::empty())).await;
        assert_eq!(body, b"hello world!");

        // Metadata, bytes-free operations, and the session log.
        let (status, body) = send(
            &router,
            request("GET", "/api/sessions/s1/entries/work/b.txt?follow=false", Body::empty()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let entry: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(entry["kind"], "file");
        assert_eq!(entry["size"], 12);
        let (status, body) = send(
            &router,
            json_request(
                "POST",
                "/api/sessions/s1/fs",
                serde_json::json!({ "op": "rename", "path": "/work/b.txt", "to": "/work/c.txt" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let (_, body) = send(
            &router,
            json_request("POST", "/api/sessions/s1/fs", serde_json::json!({ "op": "exists", "path": "/work/b.txt" })),
        )
        .await;
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["exists"], false);
        let (status, _) = send(&router, request("DELETE", "/api/sessions/s1/files/work/c.txt", Body::empty())).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(&router, request("GET", "/api/sessions/s1/files/work/c.txt", Body::empty())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(error["kind"], "not_found");
        assert!(error["error"].as_str().unwrap().contains("ENOENT"), "{error}");
        let (status, body) = send(&router, request("GET", "/api/sessions/s1/files/etc/hostname", Body::empty())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{}", String::from_utf8_lossy(&body));
        let (status, body) = send(&router, request("GET", "/api/sessions/s1/snapshots", Body::empty())).await;
        assert_eq!(status, StatusCode::OK);
        let snapshots: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = snapshots.as_array().unwrap();
        assert!(entries.len() >= 5, "{snapshots}");
        assert!(entries.iter().all(|entry| entry["output_fs"].is_string()));
        engine.shutdown().await;
    }
}
