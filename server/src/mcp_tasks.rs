//! MCP **tasks** support (spec `2025-11-25`, `basic/utilities/tasks`) layered
//! over the rmcp Streamable HTTP `/mcp` route.
//!
//! The pinned rmcp release (`0.1.5`) predates the tasks utility: its
//! `ServerHandler` trait has no task hooks, its model has no `Task` types, and
//! its `ClientRequest` enum doesn't include `tasks/*` (so unknown methods are
//! rejected with `method_not_found` before any handler runs). Rather than fork
//! or do a breaking SDK upgrade, this module implements the protocol as a thin
//! JSON-RPC shim in front of rmcp's POST handler:
//!
//!   * `initialize` is forwarded to rmcp, then its result is augmented with the
//!     `tasks` server capability so task-enabled clients discover the feature.
//!   * A task-augmented `run_js` call (one carrying `params.task`) is launched
//!     as an asynchronous engine execution and a `CreateTaskResult` is returned
//!     immediately. The task is backed directly by the engine's execution
//!     registry — the same async machinery behind `get_execution` /
//!     `cancel_execution` — so no in-process round-trip through rmcp is needed.
//!   * `tasks/get`, `tasks/result`, `tasks/cancel`, and `tasks/list` are served
//!     from that execution state.
//!   * Everything else (normal tool calls, notifications, GET/DELETE, and
//!     task-augmented calls for tools other than `run_js`) is forwarded to
//!     rmcp unchanged, so existing behaviour is preserved exactly.
//!
//! Only the Streamable HTTP transport is wrapped — that is the transport
//! task-enabled clients use. stdio and the legacy SSE transport are unaffected.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::State,
    http::{header, request::Parts, HeaderMap, Method, StatusCode},
    response::Response,
    routing::any,
    Router,
};
use dashmap::DashMap;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::engine::Engine;

/// Default retention (ms) for a task when the client doesn't request a `ttl`.
const DEFAULT_TASK_TTL_MS: u64 = 5 * 60 * 1000; // 5 minutes
/// Suggested client poll interval (ms), surfaced via `pollInterval`.
const TASK_POLL_INTERVAL_MS: u64 = 500;
/// JSON-RPC "Invalid params" — returned for an unknown/malformed `taskId`.
const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC "Internal error".
const INTERNAL_ERROR: i64 = -32603;
/// Cap on buffered request/response bodies (64 MiB).
const MAX_BODY: usize = 64 * 1024 * 1024;
/// Poll interval while `tasks/result` blocks waiting for a terminal state.
const RESULT_POLL: Duration = Duration::from_millis(100);

// ── Task ⇄ execution mapping ────────────────────────────────────────────────

/// A task is a thin handle over an engine execution. The execution registry is
/// the source of truth for status/result; this record only holds task-level
/// metadata (id mapping, creation time, ttl).
struct TaskRecord {
    task_id: String,
    execution_id: String,
    created_at: String,
    ttl_ms: u64,
    created_instant: Instant,
}

/// Map an engine execution status string to an MCP task status. Engine
/// statuses are `running`/`completed`/`failed`/`cancelled`/`timed_out`.
fn map_engine_status(engine_status: &str) -> &'static str {
    match engine_status {
        "running" => "working",
        "completed" => "completed",
        "cancelled" => "cancelled",
        // A timeout is a failure of the underlying request.
        "failed" | "timed_out" => "failed",
        _ => "working",
    }
}

fn engine_status_is_terminal(engine_status: &str) -> bool {
    matches!(engine_status, "completed" | "failed" | "cancelled" | "timed_out")
}

// ── Shim state ──────────────────────────────────────────────────────────────

struct TaskShim {
    inner: Router,
    engine: Engine,
    tasks: Arc<DashMap<String, TaskRecord>>,
}

/// Wrap rmcp's MCP router so the `/mcp` path also speaks the tasks protocol.
/// Non-task traffic is transparently forwarded to `inner`.
///
/// Must be called from within a Tokio runtime (it spawns a retention sweeper).
pub fn wrap(mcp_path: &str, inner: Router, engine: Engine) -> Router {
    let tasks: Arc<DashMap<String, TaskRecord>> = Arc::new(DashMap::new());

    // Retention sweeper: drop task records once their ttl has elapsed.
    {
        let tasks = tasks.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                let now = Instant::now();
                tasks.retain(|_, rec| {
                    now.duration_since(rec.created_instant) < Duration::from_millis(rec.ttl_ms)
                });
            }
        });
    }

    let shim = Arc::new(TaskShim { inner, engine, tasks });
    Router::new().route(mcp_path, any(handle)).with_state(shim)
}

/// The `tasks` server capability advertised on `initialize`.
fn tasks_capability() -> Value {
    json!({
        "list": {},
        "cancel": {},
        "requests": { "tools": { "call": {} } }
    })
}

// ── Request dispatch ──────────────────────────────────────────────────────

async fn handle(State(shim): State<Arc<TaskShim>>, req: axum::extract::Request) -> Response {
    // Only POST carries JSON-RPC messages; forward other methods (GET stream,
    // DELETE session teardown) straight to rmcp.
    if req.method() != Method::POST {
        return forward(&shim.inner, req).await;
    }

    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return plain(StatusCode::BAD_REQUEST, "failed to read request body"),
    };

    // Anything we don't recognise as a single JSON-RPC object is forwarded
    // verbatim (batches, malformed bodies, etc.).
    let msg: Value = match serde_json::from_slice(&bytes) {
        Ok(v @ Value::Object(_)) => v,
        _ => return forward(&shim.inner, rebuild(parts, bytes.to_vec())).await,
    };
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");

    match method {
        "initialize" => handle_initialize(&shim.inner, rebuild(parts, bytes.to_vec())).await,
        "tasks/get" => respond_json(&get_task(&shim, &msg)),
        "tasks/list" => respond_json(&list_tasks(&shim, &msg)),
        "tasks/cancel" => respond_json(&cancel_task(&shim, &msg)),
        "tasks/result" => handle_tasks_result(&shim, &msg).await,
        // Only run_js benefits from task execution; other tools are fast and
        // synchronous, so a task-augmented call for them is forwarded as a
        // normal call (rmcp ignores the extra `task` field).
        "tools/call" if is_run_js_task(&msg) => create_run_js_task(&shim, &parts, &msg).await,
        _ => forward(&shim.inner, rebuild(parts, bytes.to_vec())).await,
    }
}

/// True when a `tools/call` for `run_js` opts into task execution via
/// `params.task` (an object, possibly empty — `TaskMetadata`).
fn is_run_js_task(msg: &Value) -> bool {
    let params = match msg.get("params") {
        Some(p) => p,
        None => return false,
    };
    let augmented = params.get("task").map(Value::is_object).unwrap_or(false);
    let is_run_js = params.get("name").and_then(Value::as_str) == Some("run_js");
    augmented && is_run_js
}

// ── initialize: inject the tasks capability ───────────────────────────────

async fn handle_initialize(inner: &Router, req: axum::extract::Request) -> Response {
    let resp = match inner.clone().oneshot(req).await {
        Ok(r) => r,
        Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "initialize forward failed"),
    };
    let (parts, body) = resp.into_parts();
    // rmcp returns the new session id on initialize; preserve it for the client.
    let session_id = parts.headers.get("mcp-session-id").cloned();

    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "initialize body read failed"),
    };

    let Some(mut rpc) = parse_rpc_from_body(&parts.headers, &bytes) else {
        // Couldn't parse (unexpected) — pass rmcp's response through unchanged.
        return Response::from_parts(parts, Body::from(bytes));
    };

    if let Some(result) = rpc.get_mut("result").and_then(Value::as_object_mut) {
        let caps = result.entry("capabilities").or_insert_with(|| json!({}));
        if let Some(caps) = caps.as_object_mut() {
            caps.insert("tasks".into(), tasks_capability());
        }
    }

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(sid) = session_id {
        builder = builder.header("mcp-session-id", sid);
    }
    builder
        .body(Body::from(serde_json::to_vec(&rpc).unwrap_or_default()))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "initialize build failed"))
}

// ── task-augmented run_js ──────────────────────────────────────────────────

async fn create_run_js_task(shim: &Arc<TaskShim>, parts: &Parts, msg: &Value) -> Response {
    let id = rpc_id(msg);
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let ttl_ms = params
        .get("task")
        .and_then(|t| t.get("ttl"))
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TASK_TTL_MS);

    // Mirror McpService::run_js argument handling against the engine builder.
    let code = args.get("code").and_then(Value::as_str).unwrap_or("").to_string();
    let mut req = shim.engine.run_js(code);
    req = req.maybe_file(args.get("file").and_then(Value::as_str).map(str::to_string));
    if let Some(heap) = args.get("heap").and_then(Value::as_str) {
        req = req.heap(heap);
    }
    req = req.maybe_fs(args.get("fs").and_then(Value::as_str).map(str::to_string));
    if let Some(mb) = args.get("heap_memory_max_mb").and_then(Value::as_u64) {
        req = req.heap_memory_max_mb(mb as usize);
    }
    if let Some(secs) = args.get("execution_timeout_secs").and_then(Value::as_u64) {
        req = req.execution_timeout_secs(secs);
    }
    if let Some(tags) = parse_tags(&args) {
        req = req.tags(tags);
    }
    if let Some(session) = mcp_session_id(&parts.headers) {
        req = req.session(session);
    }
    req = req.maybe_mcp_headers(mcp_headers(&parts.headers));

    let execution_id = match req.execute().await {
        Ok(id) => id,
        Err(e) => {
            return respond_json(&rpc_error(
                id,
                INTERNAL_ERROR,
                &format!("failed to start run_js task: {e}"),
            ))
        }
    };

    let record = TaskRecord {
        task_id: uuid::Uuid::new_v4().to_string(),
        execution_id,
        created_at: now_rfc3339(),
        ttl_ms,
        created_instant: Instant::now(),
    };
    let task_json = build_task_json(&record, &shim.engine);
    shim.tasks.insert(record.task_id.clone(), record);

    respond_json(&rpc_result(id, json!({ "task": task_json })))
}

// ── tasks/get, tasks/list, tasks/cancel ───────────────────────────────────

fn get_task(shim: &TaskShim, msg: &Value) -> Value {
    let id = rpc_id(msg);
    let Some(task_id) = task_id_param(msg) else {
        return rpc_error(id, INVALID_PARAMS, "missing or invalid params.taskId");
    };
    match shim.tasks.get(&task_id) {
        Some(rec) => rpc_result(id, build_task_json(&rec, &shim.engine)),
        None => rpc_error(id, INVALID_PARAMS, &format!("unknown taskId: {task_id}")),
    }
}

fn list_tasks(shim: &TaskShim, msg: &Value) -> Value {
    let id = rpc_id(msg);
    let tasks: Vec<Value> = shim
        .tasks
        .iter()
        .map(|entry| build_task_json(entry.value(), &shim.engine))
        .collect();
    rpc_result(id, json!({ "tasks": tasks }))
}

fn cancel_task(shim: &TaskShim, msg: &Value) -> Value {
    let id = rpc_id(msg);
    let Some(task_id) = task_id_param(msg) else {
        return rpc_error(id, INVALID_PARAMS, "missing or invalid params.taskId");
    };
    match shim.tasks.get(&task_id) {
        Some(rec) => {
            // Best-effort: terminate the underlying execution. Already-terminal
            // executions return an error here, which we ignore.
            let _ = shim.engine.cancel_execution(&rec.execution_id);
            rpc_result(id, build_task_json(&rec, &shim.engine))
        }
        None => rpc_error(id, INVALID_PARAMS, &format!("unknown taskId: {task_id}")),
    }
}

// ── tasks/result: block until terminal, then return the run_js outcome ─────

async fn handle_tasks_result(shim: &TaskShim, msg: &Value) -> Response {
    let id = rpc_id(msg);
    let Some(task_id) = task_id_param(msg) else {
        return respond_json(&rpc_error(id, INVALID_PARAMS, "missing or invalid params.taskId"));
    };
    let Some(execution_id) = shim.tasks.get(&task_id).map(|r| r.execution_id.clone()) else {
        return respond_json(&rpc_error(id, INVALID_PARAMS, &format!("unknown taskId: {task_id}")));
    };

    // Block until the execution reaches a terminal state. The execution has its
    // own timeout, so this always resolves.
    loop {
        match shim.engine.get_execution(&execution_id) {
            Ok(info) if engine_status_is_terminal(&info.status) => {
                let payload = build_result_payload(shim, &execution_id, &info);
                return respond_json(&rpc_result(id, payload));
            }
            Ok(_) => {}
            Err(e) => {
                return respond_json(&rpc_error(
                    id,
                    INTERNAL_ERROR,
                    &format!("execution lookup failed: {e}"),
                ))
            }
        }
        tokio::time::sleep(RESULT_POLL).await;
    }
}

/// Build the `Task` object for a record, reading live status from the engine.
fn build_task_json(rec: &TaskRecord, engine: &Engine) -> Value {
    let info = engine.get_execution(&rec.execution_id).ok();
    let engine_status = info.as_ref().map(|i| i.status.as_str()).unwrap_or("failed");
    let status = map_engine_status(engine_status);
    let last_updated = info
        .as_ref()
        .and_then(|i| i.completed_at.clone())
        .unwrap_or_else(|| rec.created_at.clone());

    let mut m = serde_json::Map::new();
    m.insert("taskId".into(), json!(rec.task_id));
    m.insert("status".into(), json!(status));
    if let Some(err) = info.as_ref().and_then(|i| i.error.clone()) {
        if matches!(status, "failed" | "cancelled") {
            m.insert("statusMessage".into(), json!(err));
        }
    }
    m.insert("createdAt".into(), json!(rec.created_at));
    m.insert("lastUpdatedAt".into(), json!(last_updated));
    m.insert("ttl".into(), json!(rec.ttl_ms));
    m.insert("pollInterval".into(), json!(TASK_POLL_INTERVAL_MS));
    Value::Object(m)
}

/// Build the `tasks/result` payload: a `CallToolResult` describing the finished
/// run_js execution (its console output, status, and any result/heap/fs/error).
fn build_result_payload(
    shim: &TaskShim,
    execution_id: &str,
    info: &crate::engine::execution::ExecutionInfo,
) -> Value {
    let output = shim
        .engine
        .get_execution_output(execution_id, None, Some(u64::MAX), None, None)
        .map(|page| page.data)
        .unwrap_or_default();
    let is_error = matches!(info.status.as_str(), "failed" | "timed_out" | "cancelled");
    let inner = json!({
        "execution_id": execution_id,
        "status": info.status,
        "result": info.result,
        "output": output,
        "heap": info.heap,
        "fs": info.fs,
        "error": info.error,
    });
    json!({
        "content": [ { "type": "text", "text": inner.to_string() } ],
        "isError": is_error
    })
}

// ── forwarding & framing helpers ───────────────────────────────────────────

/// Forward an unmodified request to the inner rmcp router. This is exactly how
/// `axum::serve` would invoke the router, so forwarded requests behave as if
/// the shim were not present.
async fn forward(inner: &Router, req: axum::extract::Request) -> Response {
    match inner.clone().oneshot(req).await {
        Ok(resp) => resp,
        Err(_) => plain(StatusCode::INTERNAL_SERVER_ERROR, "forward failed"),
    }
}

fn rebuild(parts: Parts, body: Vec<u8>) -> axum::extract::Request {
    axum::http::Request::from_parts(parts, Body::from(body))
}

/// Extract a JSON-RPC message from a response body that may be framed as a
/// single JSON object or as an SSE stream (`data:` lines).
fn parse_rpc_from_body(headers: &HeaderMap, bytes: &[u8]) -> Option<Value> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type.contains("text/event-stream") {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut data = String::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        serde_json::from_str(&data).ok()
    } else {
        serde_json::from_slice(bytes).ok()
    }
}

/// Read `X-MCP-Session-Id` (used to scope stateful run_js sessions), mirroring
/// `McpService`'s header handling.
fn mcp_session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Collect `X-MCP-*` headers into a map (keys stripped of the `x-mcp-` prefix),
/// for policy evaluation — same shape `McpService` captures at initialize.
fn mcp_headers(headers: &HeaderMap) -> Option<Value> {
    let mut map = serde_json::Map::new();
    for (name, value) in headers.iter() {
        if let Some(key) = name.as_str().strip_prefix("x-mcp-") {
            if let Ok(v) = value.to_str() {
                map.insert(key.to_string(), Value::String(v.to_string()));
            }
        }
    }
    if map.is_empty() {
        None
    } else {
        Some(Value::Object(map))
    }
}

fn parse_tags(args: &Value) -> Option<HashMap<String, String>> {
    let obj = args.get("tags")?.as_object()?;
    let mut tags = HashMap::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            tags.insert(k.clone(), s.to_string());
        }
    }
    if tags.is_empty() {
        None
    } else {
        Some(tags)
    }
}

fn task_id_param(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("taskId")?
        .as_str()
        .map(str::to_string)
}

fn rpc_id(msg: &Value) -> Value {
    msg.get("id").cloned().unwrap_or(Value::Null)
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn respond_json(value: &Value) -> Response {
    let body = serde_json::to_vec(value).unwrap_or_default();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("static response builder")
}

fn plain(status: StatusCode, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(message.to_string()))
        .expect("static response builder")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_status_maps_to_task_status() {
        assert_eq!(map_engine_status("running"), "working");
        assert_eq!(map_engine_status("completed"), "completed");
        assert_eq!(map_engine_status("failed"), "failed");
        assert_eq!(map_engine_status("timed_out"), "failed");
        assert_eq!(map_engine_status("cancelled"), "cancelled");
    }

    #[test]
    fn terminal_engine_statuses() {
        assert!(!engine_status_is_terminal("running"));
        assert!(engine_status_is_terminal("completed"));
        assert!(engine_status_is_terminal("failed"));
        assert!(engine_status_is_terminal("timed_out"));
        assert!(engine_status_is_terminal("cancelled"));
    }

    #[test]
    fn detects_run_js_task_augmentation() {
        let augmented = json!({
            "method": "tools/call",
            "params": { "name": "run_js", "arguments": {}, "task": {} }
        });
        assert!(is_run_js_task(&augmented));

        let with_ttl = json!({
            "method": "tools/call",
            "params": { "name": "run_js", "arguments": {}, "task": { "ttl": 1000 } }
        });
        assert!(is_run_js_task(&with_ttl));

        // No task field → not a task.
        let plain = json!({
            "method": "tools/call",
            "params": { "name": "run_js", "arguments": {} }
        });
        assert!(!is_run_js_task(&plain));

        // Augmenting a different tool is not intercepted.
        let other = json!({
            "method": "tools/call",
            "params": { "name": "get_execution", "arguments": {}, "task": {} }
        });
        assert!(!is_run_js_task(&other));

        // A null task is not an opt-in.
        let null_task = json!({
            "method": "tools/call",
            "params": { "name": "run_js", "task": Value::Null }
        });
        assert!(!is_run_js_task(&null_task));
    }

    #[test]
    fn parses_rpc_from_sse_and_json() {
        let mut sse_headers = HeaderMap::new();
        sse_headers.insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
        let sse = b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let parsed = parse_rpc_from_body(&sse_headers, sse).expect("sse parse");
        assert_eq!(parsed["result"]["ok"], true);

        let mut json_headers = HeaderMap::new();
        json_headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        let body = br#"{"jsonrpc":"2.0","id":2,"result":{"n":5}}"#;
        let parsed = parse_rpc_from_body(&json_headers, body).expect("json parse");
        assert_eq!(parsed["result"]["n"], 5);
    }

    #[test]
    fn collects_mcp_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-mcp-session-id", "sess-1".parse().unwrap());
        headers.insert("x-mcp-tenant", "acme".parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());

        assert_eq!(mcp_session_id(&headers).as_deref(), Some("sess-1"));
        let collected = mcp_headers(&headers).expect("some headers");
        assert_eq!(collected["session-id"], "sess-1");
        assert_eq!(collected["tenant"], "acme");
        assert!(collected.get("content-type").is_none());
    }

    #[test]
    fn tasks_capability_shape() {
        let cap = tasks_capability();
        assert!(cap["list"].is_object());
        assert!(cap["cancel"].is_object());
        assert!(cap["requests"]["tools"]["call"].is_object());
    }
}
