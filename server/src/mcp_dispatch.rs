//! Transport-agnostic MCP tool dispatch.
//!
//! Both the primary rmcp 1.x handler (`mcp.rs`, Streamable HTTP + stdio + native
//! tasks) and the legacy SSE handler (`mcp_sse.rs`, backed by the vendored rmcp
//! 0.1.5 SSE server transport) route tool calls through here, so the actual
//! tool logic lives in exactly one place regardless of which rmcp version
//! frames the request/response.
//!
//! Each function takes the tool arguments as a `serde_json::Value` object and
//! returns the tool's result body as a `Value`; the per-transport handlers wrap
//! that into their respective `CallToolResult` types.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::engine::Engine;
use crate::engine::heap_tags::HeapTagEntry;

/// Dispatch a tool call by name. `args` is the arguments object (may be null).
pub async fn call_tool(
    engine: &Engine,
    session_id: Option<&str>,
    mcp_headers: Option<&Value>,
    name: &str,
    args: &Value,
) -> Value {
    match name {
        "run_js" => run_js(engine, session_id, mcp_headers, args).await,
        "get_execution" => get_execution(engine, args),
        "get_execution_output" => get_execution_output(engine, args),
        "cancel_execution" => cancel_execution(engine, args),
        "list_executions" => list_executions(engine),
        "list_sessions" => list_sessions(engine).await,
        "list_session_snapshots" => list_session_snapshots(engine, session_id, args).await,
        "get_heap_tags" => get_heap_tags(engine, args).await,
        "set_heap_tags" => set_heap_tags(engine, args).await,
        "delete_heap_tags" => delete_heap_tags(engine, args).await,
        "query_heaps_by_tags" => query_heaps_by_tags(engine, args).await,
        "fs_ls" => fs_ls(engine).await,
        "fs_pull" => fs_pull(engine, args).await,
        "fs_label" => fs_label(engine, args).await,
        "fs_log" => fs_log(engine, args).await,
        "fs_push" => fs_push(engine, args).await,
        "fs_reset" => fs_reset(engine, args).await,
        "fs_merge" => fs_merge(engine, args).await,
        other => json!({ "error": format!("unknown tool: {other}") }),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    str_arg(args, key).map(str::to_string)
}

fn map_arg(args: &Value, key: &str) -> Option<HashMap<String, String>> {
    let obj = args.get(key)?.as_object()?;
    let mut m = HashMap::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            m.insert(k.clone(), s.to_string());
        }
    }
    Some(m)
}

pub async fn run_js(
    engine: &Engine,
    session_id: Option<&str>,
    mcp_headers: Option<&Value>,
    args: &Value,
) -> Value {
    let mut req = engine.run_js(string_arg(args, "code").unwrap_or_default());
    req = req.maybe_file(string_arg(args, "file"));
    if let Some(h) = string_arg(args, "heap") {
        req = req.heap(h);
    }
    req = req.maybe_fs(string_arg(args, "fs"));
    if let Some(s) = session_id {
        req = req.session(s.to_string());
    }
    if let Some(mb) = args.get("heap_memory_max_mb").and_then(Value::as_u64) {
        req = req.heap_memory_max_mb(mb as usize);
    }
    if let Some(secs) = args.get("execution_timeout_secs").and_then(Value::as_u64) {
        req = req.execution_timeout_secs(secs);
    }
    if let Some(tags) = map_arg(args, "tags") {
        req = req.tags(tags);
    }
    req = req.maybe_mcp_headers(mcp_headers.cloned());
    let execution_id = match req.execute().await {
        Ok(id) => id,
        Err(e) => format!("error: {}", e),
    };
    json!({ "execution_id": execution_id })
}

/// Stateless run_js: submit, poll to completion, and return console output
/// directly (used by the stateless MCP service and the stateless SSE handler).
pub async fn run_js_blocking(engine: &Engine, mcp_headers: Option<&Value>, args: &Value) -> Value {
    let mut req = engine.run_js(string_arg(args, "code").unwrap_or_default());
    req = req.maybe_file(string_arg(args, "file"));
    if let Some(mb) = args.get("heap_memory_max_mb").and_then(Value::as_u64) {
        req = req.heap_memory_max_mb(mb as usize);
    }
    if let Some(secs) = args.get("execution_timeout_secs").and_then(Value::as_u64) {
        req = req.execution_timeout_secs(secs);
    }
    req = req.maybe_mcp_headers(mcp_headers.cloned());
    let exec_id = match req.execute().await {
        Ok(id) => id,
        Err(e) => return json!({ "error": e }),
    };

    let poll_interval = tokio::time::Duration::from_millis(50);
    let max_polls = 6000; // 5 minutes at 50ms intervals
    let mut status = String::new();
    let mut error_msg: Option<String> = None;
    for _ in 0..max_polls {
        tokio::time::sleep(poll_interval).await;
        match engine.get_execution(&exec_id) {
            Ok(info) => match info.status.as_str() {
                "completed" => { status = info.status; break; }
                "failed" | "timed_out" | "cancelled" => {
                    status = info.status;
                    error_msg = info.error;
                    break;
                }
                _ => continue,
            },
            Err(_) => continue,
        }
    }

    if status.is_empty() {
        return json!({ "error": "Execution did not complete within polling timeout" });
    }
    let output = engine
        .get_execution_output(&exec_id, None, Some(u64::MAX), None, None)
        .map(|page| page.data)
        .unwrap_or_default();
    match status.as_str() {
        "completed" => json!({ "output": output }),
        _ => json!({ "output": output, "error": error_msg }),
    }
}

fn get_execution(engine: &Engine, args: &Value) -> Value {
    let id = string_arg(args, "execution_id").unwrap_or_default();
    match engine.get_execution(&id) {
        Ok(info) => json!({
            "execution_id": info.id,
            "status": info.status,
            "result": info.result,
            "heap": info.heap,
            "fs": info.fs,
            "error": info.error,
            "started_at": info.started_at,
            "completed_at": info.completed_at,
        }),
        Err(e) => json!({ "error": e }),
    }
}

fn get_execution_output(engine: &Engine, args: &Value) -> Value {
    let id = string_arg(args, "execution_id").unwrap_or_default();
    let line_offset = args.get("line_offset").and_then(Value::as_u64);
    let line_limit = args.get("line_limit").and_then(Value::as_u64);
    let byte_offset = args.get("byte_offset").and_then(Value::as_u64);
    let byte_limit = args.get("byte_limit").and_then(Value::as_u64);
    let status = engine
        .get_execution(&id)
        .map(|info| info.status)
        .unwrap_or_else(|_| "unknown".to_string());
    match engine.get_execution_output(&id, line_offset, line_limit, byte_offset, byte_limit) {
        Ok(page) => json!({
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
        }),
        Err(e) => json!({ "error": e }),
    }
}

fn cancel_execution(engine: &Engine, args: &Value) -> Value {
    let id = string_arg(args, "execution_id").unwrap_or_default();
    match engine.cancel_execution(&id) {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

fn list_executions(engine: &Engine) -> Value {
    match engine.list_executions() {
        Ok(executions) => json!({ "executions": executions }),
        Err(e) => json!({ "error": e }),
    }
}

async fn list_sessions(engine: &Engine) -> Value {
    match engine.list_sessions().await {
        Ok(sessions) => json!({ "sessions": sessions }),
        Err(e) => json!({ "sessions": [format!("Error: {}", e)] }),
    }
}

async fn list_session_snapshots(engine: &Engine, session_id: Option<&str>, args: &Value) -> Value {
    let session = match session_id {
        Some(id) => id.to_string(),
        None => {
            return json!({
                "entries": [{"error": "no session ID available (send X-MCP-Session-Id header)"}]
            })
        }
    };
    let parsed_fields = string_arg(args, "fields").map(|f| {
        f.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
    });
    match engine.list_session_snapshots(session, parsed_fields).await {
        Ok(entries) => json!({ "entries": entries }),
        Err(e) => json!({ "entries": [{"error": e}] }),
    }
}

async fn get_heap_tags(engine: &Engine, args: &Value) -> Value {
    let heap = string_arg(args, "heap").unwrap_or_default();
    match engine.get_heap_tags(heap).await {
        Ok(tags) => json!({ "tags": tags }),
        Err(e) => json!({ "tags": { "error": e } }),
    }
}

async fn set_heap_tags(engine: &Engine, args: &Value) -> Value {
    let heap = string_arg(args, "heap").unwrap_or_default();
    let tags = map_arg(args, "tags").unwrap_or_default();
    match engine.set_heap_tags(heap, tags).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

async fn delete_heap_tags(engine: &Engine, args: &Value) -> Value {
    let heap = string_arg(args, "heap").unwrap_or_default();
    let parsed_keys = string_arg(args, "keys").map(|k| {
        k.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
    });
    match engine.delete_heap_tags(heap, parsed_keys).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

async fn query_heaps_by_tags(engine: &Engine, args: &Value) -> Value {
    let tags = map_arg(args, "tags").unwrap_or_default();
    match engine.query_heaps_by_tags(tags).await {
        Ok(results) => {
            let entries: Vec<Value> = results
                .into_iter()
                .map(|e: HeapTagEntry| json!({ "heap": e.heap, "tags": e.tags }))
                .collect();
            json!({ "results": entries })
        }
        Err(e) => json!({ "results": [{ "heap": "error", "tags": { "error": e } }] }),
    }
}

async fn fs_ls(engine: &Engine) -> Value {
    match engine.fs_list_labels().await {
        Ok(labels) => json!({ "labels": labels }),
        Err(e) => json!({ "error": e }),
    }
}

async fn fs_pull(engine: &Engine, args: &Value) -> Value {
    let label = string_arg(args, "label").unwrap_or_default();
    match engine.fs_resolve_label(&label).await {
        Ok(Some(ca_id)) => json!({ "label": label, "ca_id": ca_id }),
        Ok(None) => json!({ "error": format!("unknown label: {label}") }),
        Err(e) => json!({ "error": e }),
    }
}

async fn fs_label(engine: &Engine, args: &Value) -> Value {
    let name = string_arg(args, "name").unwrap_or_default();
    let ca_id = string_arg(args, "ca_id").unwrap_or_default();
    let message = string_arg(args, "message");
    match engine.fs_set_label(&name, &ca_id, message).await {
        Ok(()) => json!({ "label": name, "ca_id": ca_id }),
        Err(e) => json!({ "error": e }),
    }
}

async fn fs_log(engine: &Engine, args: &Value) -> Value {
    let label = string_arg(args, "label").unwrap_or_default();
    let limit = args.get("limit").and_then(Value::as_u64).map(|v| v as usize);
    match engine.fs_label_log(&label, limit).await {
        Ok(entries) => json!({ "label": label, "log": entries }),
        Err(e) => json!({ "error": e }),
    }
}

async fn fs_push(engine: &Engine, args: &Value) -> Value {
    let ca_id = string_arg(args, "ca_id").unwrap_or_default();
    let detach = args.get("detach").and_then(Value::as_bool).unwrap_or(false);
    if detach {
        return json!({ "status": "detached", "ca_id": ca_id });
    }
    let Some(label) = string_arg(args, "label") else {
        return json!({ "error": "fs_push requires a `label` unless detach=true" });
    };
    let expected = string_arg(args, "expected");
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
    let message = string_arg(args, "message");
    match engine.fs_push(&label, &ca_id, expected, force, message).await {
        Ok(outcome) => serde_json::to_value(&outcome).unwrap_or_else(|e| json!({ "error": e.to_string() })),
        Err(e) => json!({ "error": e }),
    }
}

async fn fs_reset(engine: &Engine, args: &Value) -> Value {
    let label = string_arg(args, "label").unwrap_or_default();
    let ca_id = string_arg(args, "ca_id").unwrap_or_default();
    let allow_unlogged = args.get("allow_unlogged").and_then(Value::as_bool).unwrap_or(false);
    let message = string_arg(args, "message");
    match engine.fs_reset(&label, &ca_id, allow_unlogged, message).await {
        Ok(()) => json!({ "label": label, "ca_id": ca_id }),
        Err(e) => json!({ "error": e }),
    }
}

async fn fs_merge(engine: &Engine, args: &Value) -> Value {
    let ours = string_arg(args, "ours").unwrap_or_default();
    let theirs = string_arg(args, "theirs").unwrap_or_default();
    let base = string_arg(args, "base");
    let prefer = match crate::engine::fs_merge::Prefer::parse(args.get("prefer").and_then(Value::as_str)) {
        Ok(p) => p,
        Err(e) => return json!({ "error": e }),
    };
    match engine.fs_merge(&ours, &theirs, base, prefer).await {
        Ok(result) => serde_json::to_value(&result).unwrap_or_else(|e| json!({ "error": e.to_string() })),
        Err(e) => json!({ "error": e }),
    }
}
