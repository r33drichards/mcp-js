use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

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
    stdin: Option<String>,
}

#[derive(Serialize)]
struct ExecResponse {
    output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    heap: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stdout: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stderr: Vec<String>,
}

async fn exec_handler(
    State(engine): State<Engine>,
    Json(req): Json<ExecRequest>,
) -> (StatusCode, Json<ExecResponse>) {
    match engine
        .run_js(
            req.code,
            req.heap,
            req.session,
            req.heap_memory_max_mb,
            req.execution_timeout_secs,
            req.stdin,
        )
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(ExecResponse {
                output: result.output,
                heap: result.heap,
                stdout: result.stdout,
                stderr: result.stderr,
            }),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ExecResponse {
                output: format!("Error: {}", e),
                heap: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
            }),
        ),
    }
}

pub fn api_router(engine: Engine) -> Router {
    Router::new()
        .route("/api/exec", post(exec_handler))
        .with_state(engine)
}
