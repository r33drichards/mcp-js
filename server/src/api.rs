use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{OpenApi, ToSchema};

use crate::engine::Engine;

// ── CLI download helpers ──────────────────────────────────────────────

/// The version of this server binary, from Cargo.toml at compile time.
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// GitHub repo where releases are published.
const GITHUB_REPO: &str = "r33drichards/mcp-js";

/// Map a `{os}-{arch}` platform string to the release asset name.
fn cli_asset_name(platform: &str) -> Option<&'static str> {
    match platform {
        "linux-x86_64"  => Some("mcp-v8-cli-linux-x86_64"),
        "linux-aarch64" => Some("mcp-v8-cli-linux-arm64"),
        "macos-x86_64"  => Some("mcp-v8-cli-macos-x86_64"),
        "macos-aarch64" => Some("mcp-v8-cli-macos-arm64"),
        _ => None,
    }
}

fn cli_download_url(asset: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/v{}/{}",
        GITHUB_REPO, SERVER_VERSION, asset
    )
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
    /// Direct download URL for the CLI binary.
    pub url: String,
    /// Gzip-compressed download URL.
    pub url_gz: String,
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
#[utoipa::path(
    post,
    path = "/api/exec",
    request_body = ExecRequest,
    responses(
        (status = 202, description = "Execution queued", body = ExecAccepted),
        (status = 500, description = "Internal error", body = ApiError),
    ),
    tag = "executions"
)]
async fn exec_handler(
    State(engine): State<Engine>,
    Json(req): Json<ExecRequest>,
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

const PLATFORMS: &[&str] = &[
    "linux-x86_64",
    "linux-aarch64",
    "macos-x86_64",
    "macos-aarch64",
];

/// List available CLI binary downloads for the running server version.
///
/// Returns download URLs for all supported platforms. The CLI binary will
/// always match the server version it was downloaded from.
#[utoipa::path(
    get,
    path = "/api/cli",
    responses(
        (status = 200, description = "CLI download index", body = CliIndex),
    ),
    tag = "cli"
)]
async fn cli_index_handler() -> Json<serde_json::Value> {
    let assets: Vec<_> = PLATFORMS
        .iter()
        .filter_map(|p| {
            let asset = cli_asset_name(p)?;
            Some(serde_json::json!({
                "platform": p,
                "url":    cli_download_url(asset),
                "url_gz": cli_download_url(&format!("{}.gz", asset)),
            }))
        })
        .collect();

    Json(serde_json::json!({
        "version": SERVER_VERSION,
        "assets": assets,
    }))
}

/// Download the CLI binary for a specific platform.
///
/// Redirects (302) to the matching GitHub Release asset for the running server
/// version. Use `?gz=1` to download the gzip-compressed variant.
///
/// Supported platforms: `linux-x86_64`, `linux-aarch64`, `macos-x86_64`, `macos-aarch64`.
#[utoipa::path(
    get,
    path = "/api/cli/{platform}",
    params(
        ("platform" = String, Path, description = "Target platform (linux-x86_64 | linux-aarch64 | macos-x86_64 | macos-aarch64)"),
        ("gz" = Option<String>, Query, description = "Set to '1' to download the .gz compressed binary"),
    ),
    responses(
        (status = 302, description = "Redirect to the CLI binary on GitHub Releases"),
        (status = 404, description = "Unknown platform", body = ApiError),
    ),
    tag = "cli"
)]
async fn cli_download_handler(
    Path(platform): Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Result<Redirect, (StatusCode, Json<serde_json::Value>)> {
    let want_gz = query
        .as_deref()
        .unwrap_or("")
        .split('&')
        .any(|kv| kv == "gz=1" || kv == "gz=true");

    match cli_asset_name(&platform) {
        Some(asset) => {
            let url = if want_gz {
                cli_download_url(&format!("{}.gz", asset))
            } else {
                cli_download_url(asset)
            };
            Ok(Redirect::temporary(&url))
        }
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "Unknown platform '{}'. Valid platforms: {}",
                    platform,
                    PLATFORMS.join(", ")
                )
            })),
        )),
    }
}

// ── Router builders ──────────────────────────────────────────────────────

/// Build the plain Axum router (no OpenAPI metadata attached).
///
/// Used when running in stdio mode where no HTTP server is present, and
/// for merging into SSE / Streamable-HTTP transport servers.
pub fn api_router(engine: Engine) -> Router {
    Router::new()
        .route("/api/exec", post(exec_handler))
        .route("/api/executions", get(list_executions_handler))
        .route("/api/executions/{id}", get(get_execution_handler))
        .route("/api/executions/{id}/output", get(get_execution_output_handler))
        .route("/api/executions/{id}/cancel", post(cancel_execution_handler))
        .route("/api/cli", get(cli_index_handler))
        .route("/api/cli/{platform}", get(cli_download_handler))
        .with_state(engine)
}
