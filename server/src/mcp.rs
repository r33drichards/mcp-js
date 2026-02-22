use rmcp::{
    model::{ServerCapabilities, ServerInfo},

    Error as McpError, RoleServer, ServerHandler, model::*,
    service::RequestContext, tool,
};
use serde_json::json;


use std::sync::Once;
use std::sync::mpsc;
use std::time::Duration;
use v8::{self};

pub mod heap_storage;
use crate::mcp::heap_storage::{HeapStorage, AnyHeapStorage};




pub fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Result<v8::Local<'s, v8::Value>, String> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).ok_or("Failed to create V8 string")?;
    let script = v8::Script::compile(scope, source, None).ok_or("Failed to compile script")?;
    let r = script.run(scope).ok_or("Failed to run script")?;
    Ok(scope.escape(r))
}

pub const DEFAULT_HEAP_MEMORY_MAX_MB: usize = 8;
pub const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 30;

/// Snapshot envelope: magic header + FNV-1a checksum + minimum size.
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
/// Format: [MCPV8SNAP\0 (10 bytes)] [FNV-1a checksum (4 bytes)] [V8 snapshot payload]
///
/// Defense in depth against invalid data reaching V8:
///   1. Magic header — rejects obviously wrong data
///   2. FNV-1a checksum — rejects corrupted data
///   3. Minimum payload size — V8 snapshots are always 100KB+, so reject
///      anything smaller. This also prevents libfuzzer from synthesizing
///      valid envelopes: CMP instrumentation can crack both magic headers
///      (~1500 iterations) and checksums (~4000 iterations) by observing
///      comparison operands, but generating a 100KB+ payload that passes
///      all checks exceeds the fuzzer's time budget.
const SNAPSHOT_MAGIC: &[u8] = b"MCPV8SNAP\x00";
const SNAPSHOT_HEADER_LEN: usize = 10 + 4; // magic (10) + checksum (4)
const MIN_SNAPSHOT_PAYLOAD: usize = 100 * 1024; // 100KB — smallest valid V8 snapshot

/// FNV-1a hash — fast, deterministic, detects storage corruption.
fn fnv1a(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for &byte in data {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn wrap_snapshot(data: &[u8]) -> Vec<u8> {
    let mut wrapped = Vec::with_capacity(SNAPSHOT_HEADER_LEN + data.len());
    wrapped.extend_from_slice(SNAPSHOT_MAGIC);
    wrapped.extend_from_slice(&fnv1a(data).to_le_bytes());
    wrapped.extend_from_slice(data);
    wrapped
}

fn unwrap_snapshot(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < SNAPSHOT_HEADER_LEN {
        return Err("Snapshot data too small".to_string());
    }
    if &data[..SNAPSHOT_MAGIC.len()] != SNAPSHOT_MAGIC {
        return Err("Invalid snapshot: missing magic header".to_string());
    }
    let stored_checksum = u32::from_le_bytes(
        data[SNAPSHOT_MAGIC.len()..SNAPSHOT_HEADER_LEN]
            .try_into()
            .unwrap(),
    );
    let payload = &data[SNAPSHOT_HEADER_LEN..];
    if payload.len() < MIN_SNAPSHOT_PAYLOAD {
        return Err("Invalid snapshot: payload too small".to_string());
    }
    if fnv1a(payload) != stored_checksum {
        return Err("Invalid snapshot: checksum mismatch".to_string());
    }
    Ok(payload.to_vec())
}

fn create_params_with_heap_limit(heap_memory_max_bytes: usize) -> v8::CreateParams {
    v8::CreateParams::default().heap_limits(0, heap_memory_max_bytes)
}

/// Callback invoked when V8 heap usage approaches the configured limit.
/// Instead of letting V8 call FatalProcessOutOfMemory (which aborts the process),
/// we terminate JS execution so the error can be returned gracefully.
unsafe extern "C" fn near_heap_limit_callback(
    data: *mut std::ffi::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    let isolate = unsafe { &mut *(data as *mut v8::Isolate) };
    isolate.terminate_execution();
    // Return an increased limit to give V8 room to unwind gracefully
    // after termination is requested
    current_heap_limit * 2
}

fn install_heap_limit_callback(isolate: &mut v8::Isolate) {
    let isolate_ptr = isolate as *mut v8::Isolate as *mut std::ffi::c_void;
    isolate.add_near_heap_limit_callback(near_heap_limit_callback, isolate_ptr);
}

/// Install an execution timeout on the isolate.
/// Returns a guard that must be dropped (or signalled) after execution completes
/// to cancel the timer thread.
fn install_execution_timeout(isolate: &mut v8::Isolate, timeout_secs: u64) -> mpsc::Sender<()> {
    let handle = isolate.thread_safe_handle();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if rx.recv_timeout(Duration::from_secs(timeout_secs)).is_err() {
            handle.terminate_execution();
        }
    });
    tx
}

// Execute JS in a stateless isolate (no snapshot creation)
pub fn execute_stateless(code: String, heap_memory_max_bytes: usize, timeout_secs: u64) -> Result<String, String> {
    let params = create_params_with_heap_limit(heap_memory_max_bytes);
    let isolate = &mut v8::Isolate::new(params);
    install_heap_limit_callback(isolate);
    let _timeout_guard = install_execution_timeout(isolate, timeout_secs);
    let scope = &mut v8::HandleScope::new(isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let result = eval(scope, &code)?;
    match result.to_string(scope) {
        Some(s) => Ok(s.to_rust_string_lossy(scope)),
        None => Err("Failed to convert result to string".to_string()),
    }
}

// Execute JS with snapshot support (preserves heap state)
pub fn execute_stateful(code: String, snapshot: Option<Vec<u8>>, heap_memory_max_bytes: usize, timeout_secs: u64) -> Result<(String, Vec<u8>), String> {
    let params = Some(create_params_with_heap_limit(heap_memory_max_bytes));

    // Validate and unwrap snapshot data before passing to V8.
    // V8's Snapshot::Initialize calls V8_Fatal (abort) on invalid data,
    // which cannot be caught, so we must validate first.
    let raw_snapshot = match snapshot {
        Some(data) if !data.is_empty() => Some(unwrap_snapshot(&data)?),
        _ => None,
    };

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
    install_heap_limit_callback(&mut snapshot_creator);
    let _timeout_guard = install_execution_timeout(&mut snapshot_creator, timeout_secs);

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

    let startup_data = snapshot_creator.create_blob(v8::FunctionCodeHandling::Clear)
        .ok_or("Failed to create V8 snapshot blob".to_string())?;
    let startup_data_vec = wrap_snapshot(&startup_data);

    output_result.map(|output| (output, startup_data_vec))
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



#[allow(dead_code)]
pub trait DataService: Send + Sync + 'static {
    fn get_data(&self) -> String;
    fn set_data(&mut self, data: String);
}

// Stateful service with heap persistence
#[derive(Clone)]
pub struct StatefulService {
    heap_storage: AnyHeapStorage,
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
    pub fn new(heap_storage: AnyHeapStorage, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Self {
        Self { heap_storage, heap_memory_max_bytes, execution_timeout_secs }
    }

    /// Execute JavaScript code with heap persistence. The heap parameter identifies the execution context.
    #[tool(description = include_str!("run_js_tool_description.md"))]
    pub async fn run_js(
        &self,
        #[tool(param)] code: String,
        #[tool(param)] heap: String,
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
        let snapshot = self.heap_storage.get(&heap).await.ok();
        let v8_result = tokio::task::spawn_blocking(move || execute_stateful(code, snapshot, max_bytes, timeout)).await;

        match v8_result {
            Ok(Ok((output, startup_data))) => {
                if let Err(e) = self.heap_storage.put(&heap, &startup_data).await {
                    return RunJsStatefulResponse {
                        output: format!("Error saving heap: {}", e),
                        heap,
                    };
                }
                RunJsStatefulResponse { output, heap }
            }
            Ok(Err(e)) => RunJsStatefulResponse {
                output: format!("V8 error: {}", e),
                heap,
            },
            Err(e) => RunJsStatefulResponse {
                output: format!("Task join error: {}", e),
                heap,
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