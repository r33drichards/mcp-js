use rmcp::{
    model::{ServerCapabilities, ServerInfo},

    Error as McpError, RoleServer, ServerHandler, model::*,
    service::RequestContext, tool,
};
use serde_json::json;


use std::sync::Once;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use std::panic::{catch_unwind, AssertUnwindSafe};
use v8::{self};
use sha2::{Sha256, Digest};

pub mod heap_storage;
pub mod session_log;
use crate::mcp::heap_storage::{HeapStorage, AnyHeapStorage};
use crate::mcp::session_log::{SessionLog, SessionLogEntry};




pub fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Result<v8::Local<'s, v8::Value>, String> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).ok_or("Failed to create V8 string")?;
    let script = v8::Script::compile(scope, source, None).ok_or("Failed to compile script")?;
    let r = script.run(scope).ok_or("Failed to run script")?;
    Ok(scope.escape(r))
}

pub const DEFAULT_HEAP_MEMORY_MAX_MB: usize = 8;
pub const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 30;

/// Snapshot envelope: magic header + SHA-256 checksum + minimum size.
///
/// V8's Snapshot::Initialize calls abort() on invalid snapshot data, which
/// cannot be caught by Rust's panic machinery. To prevent this, we wrap
/// snapshots in an envelope that is validated before the data reaches V8.
///
/// The envelope is stored atomically with the snapshot data (rather than as
/// a separate storage key) so that the checksum and payload cannot go out of
/// sync — e.g., if the snapshot updates but a separately-stored checksum
/// doesn't, or vice versa.
///
/// Format: [MCPV8SNAP\0 (10 bytes)] [SHA-256 checksum (32 bytes)] [V8 snapshot payload]
///
/// Defense in depth against invalid data reaching V8:
///   1. Magic header — rejects obviously wrong data
///   2. SHA-256 checksum — rejects corrupted data
///   3. Minimum payload size — V8 snapshots are always 100KB+, so reject
///      anything smaller. This also prevents libfuzzer from synthesizing
///      valid envelopes.
const SNAPSHOT_MAGIC: &[u8] = b"MCPV8SNAP\x00";
const SNAPSHOT_HEADER_LEN: usize = 10 + 32; // magic (10) + SHA-256 checksum (32)
const MIN_SNAPSHOT_PAYLOAD: usize = 100 * 1024; // 100KB — smallest valid V8 snapshot

/// Compute SHA-256 digest of the given data.
fn sha256_hash(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

struct WrappedSnapshot {
    data: Vec<u8>,
    content_hash: String,
}

fn wrap_snapshot(data: &[u8]) -> WrappedSnapshot {
    let hash = sha256_hash(data);
    let mut wrapped = Vec::with_capacity(SNAPSHOT_HEADER_LEN + data.len());
    wrapped.extend_from_slice(SNAPSHOT_MAGIC);
    wrapped.extend_from_slice(&hash);
    wrapped.extend_from_slice(data);
    let content_hash = hash.iter().map(|b| format!("{:02x}", b)).collect::<String>();
    WrappedSnapshot {
        data: wrapped,
        content_hash,
    }
}

fn unwrap_snapshot(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < SNAPSHOT_HEADER_LEN {
        return Err("Snapshot data too small".to_string());
    }
    if &data[..SNAPSHOT_MAGIC.len()] != SNAPSHOT_MAGIC {
        return Err("Invalid snapshot: missing magic header".to_string());
    }
    let stored_checksum: [u8; 32] = data[SNAPSHOT_MAGIC.len()..SNAPSHOT_HEADER_LEN]
        .try_into()
        .unwrap();
    let payload = &data[SNAPSHOT_HEADER_LEN..];
    if payload.len() < MIN_SNAPSHOT_PAYLOAD {
        return Err("Invalid snapshot: payload too small".to_string());
    }
    if sha256_hash(payload) != stored_checksum {
        return Err("Invalid snapshot: checksum mismatch".to_string());
    }
    Ok(payload.to_vec())
}

fn create_params_with_heap_limit(heap_memory_max_bytes: usize) -> v8::CreateParams {
    v8::CreateParams::default().heap_limits(0, heap_memory_max_bytes)
}

/// Data passed to the near-heap-limit callback so it can both terminate
/// execution and signal that OOM was the cause.
struct HeapLimitCallbackData {
    isolate_ptr: *mut v8::Isolate,
    oom_flag: Arc<AtomicBool>,
}

// Safety: The raw pointer is only dereferenced inside the callback which
// runs on the same thread that created the isolate.
unsafe impl Send for HeapLimitCallbackData {}
unsafe impl Sync for HeapLimitCallbackData {}

/// Callback invoked when V8 heap usage approaches the configured limit.
/// Instead of letting V8 call FatalProcessOutOfMemory (which aborts the process),
/// we terminate JS execution so the error can be returned gracefully.
unsafe extern "C" fn near_heap_limit_callback(
    data: *mut std::ffi::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    let cb_data = unsafe { &*(data as *const HeapLimitCallbackData) };
    cb_data.oom_flag.store(true, Ordering::SeqCst);
    let isolate = unsafe { &mut *cb_data.isolate_ptr };
    isolate.terminate_execution();
    // Return an increased limit to give V8 room to unwind gracefully
    // after termination is requested
    current_heap_limit * 2
}

/// Install the near-heap-limit callback on the isolate.
/// Returns a raw pointer to the callback data that must be freed after the
/// isolate is dropped (via `Box::from_raw`).
fn install_heap_limit_callback(
    isolate: &mut v8::Isolate,
    oom_flag: Arc<AtomicBool>,
) -> *mut HeapLimitCallbackData {
    let data = Box::new(HeapLimitCallbackData {
        isolate_ptr: isolate as *mut v8::Isolate,
        oom_flag,
    });
    let data_ptr = Box::into_raw(data);
    isolate.add_near_heap_limit_callback(
        near_heap_limit_callback,
        data_ptr as *mut std::ffi::c_void,
    );
    data_ptr
}

/// Install an execution timeout on the isolate.
/// Returns a guard that must be dropped (or signalled) after execution completes
/// to cancel the timer thread.
fn install_execution_timeout(
    isolate: &mut v8::Isolate,
    timeout_secs: u64,
    timeout_flag: Arc<AtomicBool>,
) -> mpsc::Sender<()> {
    let handle = isolate.thread_safe_handle();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if rx.recv_timeout(Duration::from_secs(timeout_secs)).is_err() {
            timeout_flag.store(true, Ordering::SeqCst);
            handle.terminate_execution();
        }
    });
    tx
}

/// Map a generic V8 execution error to a descriptive message based on
/// which termination flag was set.
fn classify_termination_error(
    oom_flag: &AtomicBool,
    timeout_flag: &AtomicBool,
    original_error: String,
) -> String {
    if oom_flag.load(Ordering::SeqCst) {
        "Out of memory: V8 heap limit exceeded. Try increasing heap_memory_max_mb.".to_string()
    } else if timeout_flag.load(Ordering::SeqCst) {
        "Execution timed out: script exceeded the time limit. Try increasing execution_timeout_secs.".to_string()
    } else {
        original_error
    }
}

// Execute JS in a stateless isolate (no snapshot creation)
pub fn execute_stateless(code: String, heap_memory_max_bytes: usize, timeout_secs: u64) -> Result<String, String> {
    let oom_flag = Arc::new(AtomicBool::new(false));
    let timeout_flag = Arc::new(AtomicBool::new(false));

    let oom_clone = oom_flag.clone();
    let timeout_clone = timeout_flag.clone();

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = create_params_with_heap_limit(heap_memory_max_bytes);
        let mut isolate = v8::Isolate::new(params);

        let cb_data_ptr = install_heap_limit_callback(&mut isolate, oom_clone);
        let _timeout_guard = install_execution_timeout(&mut isolate, timeout_secs, timeout_clone);

        let eval_result = {
            let scope = &mut v8::HandleScope::new(&mut isolate);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);

            match eval(scope, &code) {
                Ok(value) => match value.to_string(scope) {
                    Some(s) => Ok(s.to_rust_string_lossy(scope)),
                    None => Err("Failed to convert result to string".to_string()),
                },
                Err(e) => Err(e),
            }
        };

        // Clean up callback data after scope is dropped but before isolate is dropped.
        // The callback won't fire again since we're past all V8 execution.
        drop(isolate);
        unsafe { let _ = Box::from_raw(cb_data_ptr); }

        eval_result
    }));

    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(classify_termination_error(&oom_flag, &timeout_flag, e)),
        Err(_panic) => Err(classify_termination_error(
            &oom_flag,
            &timeout_flag,
            "V8 execution panicked unexpectedly".to_string(),
        )),
    }
}

// Execute JS with snapshot support (preserves heap state)
pub fn execute_stateful(code: String, snapshot: Option<Vec<u8>>, heap_memory_max_bytes: usize, timeout_secs: u64) -> Result<(String, Vec<u8>, String), String> {
    // Validate and unwrap snapshot data before passing to V8.
    // V8's Snapshot::Initialize calls V8_Fatal (abort) on invalid data,
    // which cannot be caught, so we must validate first.
    let raw_snapshot = match snapshot {
        Some(data) if !data.is_empty() => Some(unwrap_snapshot(&data)?),
        _ => None,
    };

    let oom_flag = Arc::new(AtomicBool::new(false));
    let timeout_flag = Arc::new(AtomicBool::new(false));

    let oom_clone = oom_flag.clone();
    let timeout_clone = timeout_flag.clone();

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = Some(create_params_with_heap_limit(heap_memory_max_bytes));

        let mut snapshot_creator = match raw_snapshot {
            Some(raw) if !raw.is_empty() => {
                eprintln!("creating isolate from snapshot...");
                v8::Isolate::snapshot_creator_from_existing_snapshot(raw, None, params)
            }
            _ => {
                eprintln!("snapshot not found, creating new isolate...");
                v8::Isolate::snapshot_creator(None, params)
            }
        };

        let cb_data_ptr = install_heap_limit_callback(&mut snapshot_creator, oom_clone);
        let _timeout_guard = install_execution_timeout(&mut snapshot_creator, timeout_secs, timeout_clone);

        let output_result;
        {
            let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);
            output_result = match eval(scope, &code) {
                Ok(result) => {
                    result
                        .to_string(scope)
                        .map(|s| s.to_rust_string_lossy(scope))
                        .ok_or_else(|| "Failed to convert result to string".to_string())
                }
                Err(e) => Err(e),
            };
            scope.set_default_context(context);
        }

        // Clean up callback data — snapshot_creator is still alive but we're
        // past all JS execution, so the callback won't fire again.
        unsafe { let _ = Box::from_raw(cb_data_ptr); }

        let startup_data = snapshot_creator.create_blob(v8::FunctionCodeHandling::Clear)
            .ok_or("Failed to create V8 snapshot blob".to_string())?;
        let wrapped = wrap_snapshot(&startup_data);

        output_result.map(|output| (output, wrapped.data, wrapped.content_hash))
    }));

    match result {
        Ok(inner) => inner.map_err(|e| classify_termination_error(&oom_flag, &timeout_flag, e)),
        Err(_panic) => Err(classify_termination_error(
            &oom_flag,
            &timeout_flag,
            "V8 execution panicked unexpectedly".to_string(),
        )),
    }
}

static INIT: Once = Once::new();
static mut PLATFORM: Option<v8::SharedRef<v8::Platform>> = None;

pub fn initialize_v8() {
    INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform.clone());
        v8::V8::initialize();
        unsafe {
            PLATFORM = Some(platform);
        }
    });
}



// Stateful service with heap persistence
#[derive(Clone)]
pub struct StatefulService {
    heap_storage: AnyHeapStorage,
    session_log: Option<SessionLog>,
    heap_memory_max_bytes: usize,
    execution_timeout_secs: u64,
}

// Stateless service without heap persistence
#[derive(Clone)]
pub struct StatelessService {
    heap_memory_max_bytes: usize,
    execution_timeout_secs: u64,
}

// response to run_js (stateful - with heap)
#[derive(Debug, Clone)]
pub struct RunJsStatefulResponse {
    pub output: String,
    pub heap: String,
}

impl IntoContents for RunJsStatefulResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({
            "output": self.output,
            "heap": self.heap,
        })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert run_js response to content: {}", e))],
        }
    }
}

// response to run_js (stateless - no heap)
#[derive(Debug, Clone)]
pub struct RunJsStatelessResponse {
    pub output: String,
}

impl IntoContents for RunJsStatelessResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({
            "output": self.output,
        })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert run_js response to content: {}", e))],
        }
    }
}

// Response for list_sessions
#[derive(Debug, Clone)]
pub struct ListSessionsResponse {
    pub sessions: Vec<String>,
}

impl IntoContents for ListSessionsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({
            "sessions": self.sessions,
        })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert list_sessions response: {}", e))],
        }
    }
}

// Response for list_session_snapshots
#[derive(Debug, Clone)]
pub struct ListSessionSnapshotsResponse {
    pub entries: Vec<serde_json::Value>,
}

impl IntoContents for ListSessionSnapshotsResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({
            "entries": self.entries,
        })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert list_session_snapshots response: {}", e))],
        }
    }
}

// Stateless service implementation
#[tool(tool_box)]
impl StatelessService {
    pub fn new(heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Self {
        Self { heap_memory_max_bytes, execution_timeout_secs }
    }

    /// Execute JavaScript code in a fresh, stateless V8 isolate. Each execution starts with a clean environment.
    #[tool(description = include_str!("run_js_tool_stateless.md"))]
    pub async fn run_js(
        &self,
        #[tool(param)] code: String,
        #[tool(param)]
        #[serde(default)]
        heap_memory_max_mb: Option<usize>,
        #[tool(param)]
        #[serde(default)]
        execution_timeout_secs: Option<u64>,
    ) -> RunJsStatelessResponse {
        let max_bytes = heap_memory_max_mb
            .map(|mb| mb * 1024 * 1024)
            .unwrap_or(self.heap_memory_max_bytes);
        let timeout = execution_timeout_secs.unwrap_or(self.execution_timeout_secs);
        let v8_result = tokio::task::spawn_blocking(move || execute_stateless(code, max_bytes, timeout)).await;

        match v8_result {
            Ok(Ok(output)) => RunJsStatelessResponse { output },
            Ok(Err(e)) => RunJsStatelessResponse {
                output: format!("V8 error: {}", e),
            },
            Err(e) => RunJsStatelessResponse {
                output: format!("Task join error: {}", e),
            },
        }
    }
}

// Stateful service implementation
#[tool(tool_box)]
impl StatefulService {
    pub fn new(heap_storage: AnyHeapStorage, session_log: Option<SessionLog>, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Self {
        Self { heap_storage, session_log, heap_memory_max_bytes, execution_timeout_secs }
    }

    /// Execute JavaScript code with heap persistence. The heap parameter is the content hash from a previous execution.
    #[tool(description = include_str!("run_js_tool_description.md"))]
    pub async fn run_js(
        &self,
        #[tool(param)] code: String,
        #[tool(param)]
        #[serde(default)]
        heap: Option<String>,
        #[tool(param)]
        #[serde(default)]
        session: Option<String>,
        #[tool(param)]
        #[serde(default)]
        heap_memory_max_mb: Option<usize>,
        #[tool(param)]
        #[serde(default)]
        execution_timeout_secs: Option<u64>,
    ) -> RunJsStatefulResponse {
        let max_bytes = heap_memory_max_mb
            .map(|mb| mb * 1024 * 1024)
            .unwrap_or(self.heap_memory_max_bytes);
        let timeout = execution_timeout_secs.unwrap_or(self.execution_timeout_secs);
        let snapshot = match &heap {
            Some(h) if !h.is_empty() => self.heap_storage.get(h).await.ok(),
            _ => None,
        };
        let code_for_log = code.clone();
        let v8_result = tokio::task::spawn_blocking(move || execute_stateful(code, snapshot, max_bytes, timeout)).await;

        match v8_result {
            Ok(Ok((output, startup_data, content_hash))) => {
                if let Err(e) = self.heap_storage.put(&content_hash, &startup_data).await {
                    return RunJsStatefulResponse {
                        output: format!("Error saving heap: {}", e),
                        heap: content_hash,
                    };
                }

                // Log to session if session name is provided and session log is available
                if let (Some(session_name), Some(log)) = (&session, &self.session_log) {
                    let entry = SessionLogEntry {
                        input_heap: heap.clone(),
                        output_heap: content_hash.clone(),
                        code: code_for_log,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    };
                    if let Err(e) = log.append(session_name, entry).await {
                        tracing::warn!("Failed to log session entry: {}", e);
                    }
                }

                RunJsStatefulResponse { output, heap: content_hash }
            }
            Ok(Err(e)) => RunJsStatefulResponse {
                output: format!("V8 error: {}", e),
                heap: heap.unwrap_or_default(),
            },
            Err(e) => RunJsStatefulResponse {
                output: format!("Task join error: {}", e),
                heap: heap.unwrap_or_default(),
            },
        }
    }

    /// List all named sessions in the session log.
    #[tool(description = "List all named sessions. Returns an array of session names that have been used with the session parameter in run_js.")]
    pub async fn list_sessions(&self) -> ListSessionsResponse {
        match &self.session_log {
            Some(log) => match log.list_sessions().await {
                Ok(sessions) => ListSessionsResponse { sessions },
                Err(e) => ListSessionsResponse {
                    sessions: vec![format!("Error: {}", e)],
                },
            },
            None => ListSessionsResponse {
                sessions: vec!["Session log not configured".to_string()],
            },
        }
    }

    /// List all log entries for a named session.
    #[tool(description = "List all log entries for a named session. Each entry contains the input heap hash, output heap hash, code executed, and timestamp. Use the fields parameter to select specific fields (comma-separated: index,input_heap,output_heap,code,timestamp).")]
    pub async fn list_session_snapshots(
        &self,
        #[tool(param)] session: String,
        #[tool(param)]
        #[serde(default)]
        fields: Option<String>,
    ) -> ListSessionSnapshotsResponse {
        match &self.session_log {
            Some(log) => {
                let parsed_fields = fields.map(|f| {
                    f.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
                });
                match log.list_entries(&session, parsed_fields).await {
                    Ok(entries) => ListSessionSnapshotsResponse { entries },
                    Err(e) => ListSessionSnapshotsResponse {
                        entries: vec![serde_json::json!({"error": e})],
                    },
                }
            }
            None => ListSessionSnapshotsResponse {
                entries: vec![serde_json::json!({"error": "Session log not configured"})],
            },
        }
    }
}

#[tool(tool_box)]
impl ServerHandler for StatelessService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("JavaScript execution service (stateless mode - no heap persistence)".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
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
        }
        Ok(self.get_info())
    }
}

#[tool(tool_box)]
impl ServerHandler for StatefulService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("JavaScript execution service (stateful mode - with heap persistence)".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
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
        }
        Ok(self.get_info())
    }
}