use axum::{
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{OpenApi, ToSchema};

/// Maximum size of a JSON `/api/exec` body (16 MiB). Multipart uploads are
/// read field-by-field and are not bounded by this constant.
const MAX_EXEC_JSON_BYTES: usize = 16 * 1024 * 1024;

use crate::engine::Engine;

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
    /// Error message (present when `status` is `failed`).
    pub error: Option<String>,
    /// ISO-8601 timestamp when execution started.
    pub started_at: String,
    /// ISO-8601 timestamp when execution finished (absent while running).
    pub completed_at: Option<String>,
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
        cli_index_handler,
        cli_download_handler,
    ),
    components(schemas(
        ExecRequest,
        ExecAccepted,
        ExecutionInfo,
        ExecutionOutput,
        ExecutionList,
        ExecutionSummary,
        CancelResult,
        ApiError,
        OutputQuery,
        CliAsset,
        CliIndex,
    ))
)]
pub struct ApiDoc;

// ── Handlers ─────────────────────────────────────────────────────────────

/// Submit JavaScript code for asynchronous execution.
///
/// Returns immediately with an `execution_id`. Use `GET /api/executions/{id}`
/// to poll status and `GET /api/executions/{id}/output` to read console output.
///
/// Accepts two request encodings, selected by `Content-Type`: an
/// `application/json` body (the schema below), or `multipart/form-data` to
/// upload the code as a file. For multipart, send the script source in a
/// `file` (or `code`) part; the optional `heap`, `session`,
/// `heap_memory_max_mb`, `execution_timeout_secs`, and `tags` (a JSON object)
/// parts mirror the JSON fields.
#[utoipa::path(
    post,
    path = "/api/exec",
    request_body = ExecRequest,
    responses(
        (status = 202, description = "Execution queued", body = ExecAccepted),
        (status = 400, description = "Malformed request body", body = ApiError),
        (status = 500, description = "Internal error", body = ApiError),
    ),
    tag = "executions"
)]
async fn exec_handler(
    State(engine): State<Engine>,
    request: Request,
) -> (StatusCode, Json<serde_json::Value>) {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let exec_req = if content_type.starts_with("multipart/form-data") {
        match parse_multipart_exec(request).await {
            Ok(req) => req,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": e })),
                )
            }
        }
    } else {
        // Default to a JSON body (back-compatible with existing clients).
        let bytes = match axum::body::to_bytes(request.into_body(), MAX_EXEC_JSON_BYTES).await {
            Ok(b) => b,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("failed to read request body: {}", e) })),
                )
            }
        };
        match serde_json::from_slice::<ExecRequest>(&bytes) {
            Ok(req) => req,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("invalid JSON body: {}", e) })),
                )
            }
        }
    };

    submit_exec(engine, exec_req).await
}

/// Build an [`ExecRequest`] from a `multipart/form-data` upload.
///
/// The script source comes from a `file` part (an uploaded file) or a `code`
/// part (a plain text field); the remaining parts map to the optional
/// [`ExecRequest`] fields. Unknown parts are ignored.
async fn parse_multipart_exec(request: Request) -> Result<ExecRequest, String> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|e| format!("invalid multipart request: {}", e))?;

    let mut code: Option<String> = None;
    let mut heap: Option<String> = None;
    let mut session: Option<String> = None;
    let mut heap_memory_max_mb: Option<usize> = None;
    let mut execution_timeout_secs: Option<u64> = None;
    let mut tags: Option<HashMap<String, String>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| format!("malformed multipart field: {}", e))?
    {
        let name = field.name().unwrap_or("").to_string();
        let text = field
            .text()
            .await
            .map_err(|e| format!("failed to read multipart part '{}': {}", name, e))?;
        match name.as_str() {
            // `file` (an uploaded file) and `code` (a text field) are aliases
            // for the script source; whichever appears last wins.
            "file" | "code" => code = Some(text),
            "heap" => heap = Some(text),
            "session" => session = Some(text),
            "heap_memory_max_mb" => {
                heap_memory_max_mb = Some(text.trim().parse::<usize>().map_err(|e| {
                    format!("invalid heap_memory_max_mb '{}': {}", text.trim(), e)
                })?);
            }
            "execution_timeout_secs" => {
                execution_timeout_secs = Some(text.trim().parse::<u64>().map_err(|e| {
                    format!("invalid execution_timeout_secs '{}': {}", text.trim(), e)
                })?);
            }
            "tags" => {
                tags = Some(
                    serde_json::from_str::<HashMap<String, String>>(&text)
                        .map_err(|e| format!("invalid tags JSON: {}", e))?,
                );
            }
            _ => { /* ignore unknown parts */ }
        }
    }

    let code = code
        .ok_or_else(|| "multipart request must include a 'file' or 'code' part".to_string())?;

    Ok(ExecRequest {
        code,
        heap,
        session,
        heap_memory_max_mb,
        execution_timeout_secs,
        tags,
    })
}

/// Queue an [`ExecRequest`] on the engine and map the result to an HTTP
/// response. Shared by the JSON and multipart code paths.
async fn submit_exec(
    engine: Engine,
    req: ExecRequest,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut r = engine.run_js(req.code);
    if let Some(h) = req.heap { r = r.heap(h); }
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

// ── Router builders ──────────────────────────────────────────────────────

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
        .with_state(engine)
}
