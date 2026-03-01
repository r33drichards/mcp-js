use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::collections::HashMap;

use crate::engine::Engine;

#[derive(Deserialize)]
struct ExecRequest {
    code: String,
    #[serde(default)]
    heap: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    heap_memory_max_mb: Option<usize>,
    #[serde(default)]
    execution_timeout_secs: Option<u64>,
    #[serde(default)]
    tags: Option<HashMap<String, String>>,
}


async fn exec_handler(
    State(engine): State<Engine>,
    Json(req): Json<ExecRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match engine
        .run_js(
            req.code,
            req.heap,
            req.session,
            req.heap_memory_max_mb,
            req.execution_timeout_secs,
            req.tags,
        )
        .await
    {
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

#[derive(Deserialize)]
struct OutputQuery {
    #[serde(default)]
    line_offset: Option<u64>,
    #[serde(default)]
    line_limit: Option<u64>,
    #[serde(default)]
    byte_offset: Option<u64>,
    #[serde(default)]
    byte_limit: Option<u64>,
}

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

pub fn api_router(engine: Engine) -> Router {
    Router::new()
        .route("/api/exec", post(exec_handler))
        .route("/api/executions", get(list_executions_handler))
        .route("/api/executions/{id}", get(get_execution_handler))
        .route("/api/executions/{id}/output", get(get_execution_output_handler))
        .route("/api/executions/{id}/cancel", post(cancel_execution_handler))
        .with_state(engine)
}
