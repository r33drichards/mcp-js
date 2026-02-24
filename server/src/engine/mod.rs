pub mod heap_storage;
pub mod session_log;
pub mod wasm_host;

use std::sync::Once;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ffi::c_void;
use std::alloc::{Layout, alloc_zeroed, alloc, dealloc};
use v8::{self};
use sha2::{Sha256, Digest};

use swc_core::common::{
    comments::SingleThreadedComments,
    sync::Lrc,
    Globals, Mark, SourceMap, GLOBALS,
};
use swc_core::ecma::visit::swc_ecma_ast::Pass;
use swc_core::ecma::codegen::{text_writer::JsWriter, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::{fixer::fixer, hygiene::hygiene, resolver};
use swc_core::ecma::transforms::typescript::strip;

use tokio::sync::Semaphore;

use crate::engine::heap_storage::{HeapStorage, AnyHeapStorage};
use crate::engine::session_log::{SessionLog, SessionLogEntry};

pub const DEFAULT_HEAP_MEMORY_MAX_MB: usize = 8;
pub const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 30;
/// Minimum heap memory in MB. V8 needs at least this much for internal
/// bookkeeping — smaller values cause `FatalProcessOutOfMemory` → `abort()`
/// before the near-heap-limit callback can fire.
pub const MIN_HEAP_MEMORY_MB: usize = 4;

// ── V8 initialization ───────────────────────────────────────────────────

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

// ── Snapshot envelope ───────────────────────────────────────────────────

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

pub fn unwrap_snapshot(data: &[u8]) -> Result<Vec<u8>, String> {
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

// ── Bounded ArrayBuffer allocator ────────────────────────────────────────
//
// Typed arrays (Uint8Array, etc.) allocate backing stores through V8's
// ArrayBuffer::Allocator, which lives outside the managed JS heap. The
// default allocator uses malloc/calloc and has no size limit — when the
// system runs out of memory V8 calls FatalProcessOutOfMemory → abort().
//
// This custom allocator tracks total allocated bytes and returns null when
// the limit is exceeded. V8 treats a null return as an allocation failure
// and throws a JS-level RangeError instead of aborting the process.

struct BoundedAllocatorState {
    allocated: AtomicUsize,
    limit: usize,
}

const ARRAY_BUF_ALIGN: usize = 16; // match platform malloc alignment

unsafe extern "C" fn bounded_allocate(
    state: &BoundedAllocatorState,
    len: usize,
) -> *mut c_void {
    if len == 0 {
        return std::ptr::null_mut();
    }
    // Atomically reserve space; undo if over limit.
    let prev = state.allocated.fetch_add(len, Ordering::SeqCst);
    if prev.saturating_add(len) > state.limit {
        state.allocated.fetch_sub(len, Ordering::SeqCst);
        return std::ptr::null_mut();
    }
    let Ok(layout) = Layout::from_size_align(len, ARRAY_BUF_ALIGN) else {
        state.allocated.fetch_sub(len, Ordering::SeqCst);
        return std::ptr::null_mut();
    };
    let ptr = unsafe { alloc_zeroed(layout) };
    if ptr.is_null() {
        state.allocated.fetch_sub(len, Ordering::SeqCst);
        return std::ptr::null_mut();
    }
    ptr as *mut c_void
}

unsafe extern "C" fn bounded_allocate_uninitialized(
    state: &BoundedAllocatorState,
    len: usize,
) -> *mut c_void {
    if len == 0 {
        return std::ptr::null_mut();
    }
    let prev = state.allocated.fetch_add(len, Ordering::SeqCst);
    if prev.saturating_add(len) > state.limit {
        state.allocated.fetch_sub(len, Ordering::SeqCst);
        return std::ptr::null_mut();
    }
    let Ok(layout) = Layout::from_size_align(len, ARRAY_BUF_ALIGN) else {
        state.allocated.fetch_sub(len, Ordering::SeqCst);
        return std::ptr::null_mut();
    };
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        state.allocated.fetch_sub(len, Ordering::SeqCst);
        return std::ptr::null_mut();
    }
    ptr as *mut c_void
}

unsafe extern "C" fn bounded_free(
    state: &BoundedAllocatorState,
    data: *mut c_void,
    len: usize,
) {
    if data.is_null() || len == 0 {
        return;
    }
    let Ok(layout) = Layout::from_size_align(len, ARRAY_BUF_ALIGN) else {
        return;
    };
    unsafe { dealloc(data as *mut u8, layout) };
    state.allocated.fetch_sub(len, Ordering::SeqCst);
}

unsafe extern "C" fn bounded_drop(state: *const BoundedAllocatorState) {
    drop(unsafe { Box::from_raw(state as *mut BoundedAllocatorState) });
}

static BOUNDED_VTABLE: v8::RustAllocatorVtable<BoundedAllocatorState> =
    v8::RustAllocatorVtable {
        allocate: bounded_allocate,
        allocate_uninitialized: bounded_allocate_uninitialized,
        free: bounded_free,
        drop: bounded_drop,
    };

fn create_bounded_allocator(limit: usize) -> v8::UniqueRef<v8::Allocator> {
    let state = Box::new(BoundedAllocatorState {
        allocated: AtomicUsize::new(0),
        limit,
    });
    unsafe { v8::new_rust_allocator(Box::into_raw(state), &BOUNDED_VTABLE) }
}

// ── V8 fatal OOM handler ─────────────────────────────────────────────────
//
// When V8 encounters an allocation that exceeds its internal limits (e.g.
// `new Array(1e9)` exceeds FixedArray::kMaxLength), it calls
// `FatalProcessOutOfMemory` which invokes this handler. This is NOT a
// recoverable condition — V8 may hold internal locks and have global state
// in an inconsistent state. Approaches like setjmp/longjmp or panic
// corrupt V8's global state and cause SIGSEGV in subsequent V8 operations.
//
// We log a descriptive message and abort. In production, the process
// manager should restart the server. The MIN_HEAP_MEMORY_MB floor and
// near_heap_limit_callback handle the vast majority of OOM scenarios
// gracefully — this handler only fires for pathological allocations that
// exceed V8's internal structural limits.

unsafe extern "C" fn oom_error_handler(
    location: *const std::ffi::c_char,
    details: &v8::OomDetails,
) {
    let loc = if location.is_null() {
        "unknown"
    } else {
        unsafe { std::ffi::CStr::from_ptr(location) }
            .to_str()
            .unwrap_or("unknown")
    };
    eprintln!(
        "V8 fatal OOM at {}: is_heap_oom={} — aborting process. \
         Consider increasing heap_memory_max_mb or simplifying the script.",
        loc, details.is_heap_oom,
    );
    std::process::abort();
}

// ── V8 heap / timeout helpers ───────────────────────────────────────────

fn create_params_with_heap_limit(heap_memory_max_bytes: usize) -> v8::CreateParams {
    let min_bytes = MIN_HEAP_MEMORY_MB * 1024 * 1024;
    let clamped = heap_memory_max_bytes.max(min_bytes);
    v8::CreateParams::default()
        .heap_limits(0, clamped)
        .array_buffer_allocator(create_bounded_allocator(clamped))
}

struct HeapLimitCallbackData {
    isolate_ptr: *mut v8::Isolate,
    oom_flag: Arc<AtomicBool>,
}

unsafe impl Send for HeapLimitCallbackData {}
unsafe impl Sync for HeapLimitCallbackData {}

unsafe extern "C" fn near_heap_limit_callback(
    data: *mut std::ffi::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    let cb_data = unsafe { &*(data as *const HeapLimitCallbackData) };
    cb_data.oom_flag.store(true, Ordering::SeqCst);
    let isolate = unsafe { &mut *cb_data.isolate_ptr };
    isolate.terminate_execution();
    current_heap_limit * 2
}

fn install_heap_limit_callback(
    isolate: &mut v8::Isolate,
    oom_flag: Arc<AtomicBool>,
) -> *mut HeapLimitCallbackData {
    // Install the OOM error handler to convert fatal V8 OOM (which
    // normally calls abort()) into a Rust panic that catch_unwind catches.
    isolate.set_oom_error_handler(oom_error_handler);

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

fn classify_termination_error(
    oom_flag: &AtomicBool,
    timed_out: bool,
    original_error: String,
) -> String {
    if oom_flag.load(Ordering::SeqCst) {
        "Out of memory: V8 heap limit exceeded. Try increasing heap_memory_max_mb.".to_string()
    } else if timed_out {
        "Execution timed out: script exceeded the time limit. Try increasing execution_timeout_secs.".to_string()
    } else {
        original_error
    }
}

// ── TypeScript type stripping ────────────────────────────────────────────
//
// Uses SWC to strip TypeScript type annotations from the input code,
// producing plain JavaScript that V8 can execute. This is type *removal*
// only — no type checking is performed. Plain JavaScript passes through
// unchanged.

pub fn strip_typescript_types(code: &str) -> Result<String, String> {
    let cm: Lrc<SourceMap> = Default::default();

    let fm = cm.new_source_file(
        swc_core::common::FileName::Anon.into(),
        code.into(),
    );

    let comments = SingleThreadedComments::default();

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        Some(&comments),
    );

    let mut parser = Parser::new_from(lexer);

    let mut program = parser
        .parse_program()
        .map_err(|e| format!("TypeScript parse error: {:?}", e))?;

    // Report non-fatal parse errors but don't fail on them
    for e in parser.take_errors() {
        eprintln!("TypeScript parse warning: {:?}", e);
    }

    let globals = Globals::default();
    GLOBALS.set(&globals, || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();

        // Conduct identifier scope analysis
        resolver(unresolved_mark, top_level_mark, true).process(&mut program);

        // Remove typescript types
        strip(unresolved_mark, top_level_mark).process(&mut program);

        // Fix up any identifiers with the same name, but different contexts
        hygiene().process(&mut program);

        // Ensure that we have enough parenthesis
        fixer(Some(&comments)).process(&mut program);

        let mut buf = vec![];
        {
            let mut emitter = Emitter {
                cfg: swc_core::ecma::codegen::Config::default(),
                cm: cm.clone(),
                comments: Some(&comments),
                wr: JsWriter::new(cm.clone(), "\n", &mut buf, None),
            };

            emitter
                .emit_program(&program)
                .map_err(|e| format!("Failed to emit JavaScript: {:?}", e))?;
        }

        String::from_utf8(buf).map_err(|e| format!("Non-UTF8 output: {}", e))
    })
}

/// Compile and instantiate WASM modules using V8's native API, binding their
/// exports as global variables. Delegates to `wasm_host::inject_wasm_modules_with_imports`
/// which handles both the zero-import path and the host-import path (with OPA policy).
pub fn inject_wasm_modules(
    scope: &mut v8::ContextScope<v8::HandleScope>,
    modules: &[WasmModule],
    host_config: &wasm_host::WasmHostConfig,
) -> Result<(), String> {
    wasm_host::inject_wasm_modules_with_imports(scope, modules, host_config)
}

pub fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Result<v8::Local<'s, v8::Value>, String> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).ok_or("Failed to create V8 string")?;
    let script = v8::Script::compile(scope, source, None).ok_or("Failed to compile script")?;
    let r = script.run(scope).ok_or("Failed to run script")?;
    Ok(scope.escape(r))
}

// ── Stateless / stateful V8 execution ───────────────────────────────────
//
// V8 always runs on the calling thread. An `IsolateHandle` is published
// for external cancellation (used by `run_js` for async timeout via
// `tokio::select!`). Tests and fuzz targets pass a no-op handle.

/// Stateless V8 execution — runs V8 on the calling thread. Publishes an
/// IsolateHandle for external cancellation (e.g. async timeout in `run_js`).
/// Returns (result, oom_flag).
pub fn execute_stateless(
    code: &str,
    heap_memory_max_bytes: usize,
    isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>>,
    wasm_modules: &[WasmModule],
    wasm_host_config: &wasm_host::WasmHostConfig,
) -> (Result<String, String>, bool) {
    let oom_flag = Arc::new(AtomicBool::new(false));

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = create_params_with_heap_limit(heap_memory_max_bytes);
        let mut isolate = v8::Isolate::new(params);

        // Publish handle immediately so caller can terminate us.
        *isolate_handle.lock().unwrap() = Some(isolate.thread_safe_handle());

        let cb_data_ptr = install_heap_limit_callback(&mut isolate, oom_flag.clone());

        let eval_result = {
            let scope = &mut v8::HandleScope::new(&mut isolate);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);

            // Inject WASM modules as globals via native V8 API.
            if let Err(e) = inject_wasm_modules(scope, wasm_modules, wasm_host_config) {
                return Err(e);
            }

            match eval(scope, code) {
                Ok(value) => match value.to_string(scope) {
                    Some(s) => Ok(s.to_rust_string_lossy(scope)),
                    None => Err("Failed to convert result to string".to_string()),
                },
                Err(e) => Err(e),
            }
        };

        *isolate_handle.lock().unwrap() = None;
        unsafe { let _ = Box::from_raw(cb_data_ptr); }

        eval_result
    }));

    let oom = oom_flag.load(Ordering::SeqCst);
    match result {
        Ok(Ok(output)) => (Ok(output), oom),
        Ok(Err(e)) => (Err(classify_termination_error(&oom_flag, false, e)), oom),
        Err(_panic) => {
            *isolate_handle.lock().unwrap() = None;
            (Err(classify_termination_error(
                &oom_flag, false, "V8 execution panicked unexpectedly".to_string(),
            )), oom)
        }
    }
}


/// Stateful V8 execution — runs V8 on the calling thread. Publishes an
/// IsolateHandle for external cancellation (e.g. async timeout in `run_js`).
/// Takes raw (already unwrapped) snapshot data. Returns (result, oom_flag).
pub fn execute_stateful(
    code: &str,
    raw_snapshot: Option<Vec<u8>>,
    heap_memory_max_bytes: usize,
    isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>>,
    wasm_modules: &[WasmModule],
    wasm_host_config: &wasm_host::WasmHostConfig,
) -> (Result<(String, Vec<u8>, String), String>, bool) {
    let oom_flag = Arc::new(AtomicBool::new(false));

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

        // Publish handle immediately so caller can terminate us.
        *isolate_handle.lock().unwrap() = Some(snapshot_creator.thread_safe_handle());

        let cb_data_ptr = install_heap_limit_callback(&mut snapshot_creator, oom_flag.clone());

        let output_result;
        {
            let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);

            // Inject WASM modules as globals via native V8 API.
            if let Err(e) = inject_wasm_modules(scope, wasm_modules, wasm_host_config) {
                scope.set_default_context(context);
                return Err(e);
            }

            output_result = match eval(scope, code) {
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

        *isolate_handle.lock().unwrap() = None;
        unsafe { let _ = Box::from_raw(cb_data_ptr); }

        let startup_data = snapshot_creator.create_blob(v8::FunctionCodeHandling::Clear)
            .ok_or("Failed to create V8 snapshot blob".to_string())?;
        let wrapped = wrap_snapshot(&startup_data);

        output_result.map(|output| (output, wrapped.data, wrapped.content_hash))
    }));

    let oom = oom_flag.load(Ordering::SeqCst);
    match result {
        Ok(Ok(triple)) => (Ok(triple), oom),
        Ok(Err(e)) => (Err(classify_termination_error(&oom_flag, false, e)), oom),
        Err(_panic) => {
            *isolate_handle.lock().unwrap() = None;
            (Err(classify_termination_error(
                &oom_flag, false, "V8 execution panicked unexpectedly".to_string(),
            )), oom)
        }
    }
}

// ── Engine ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct JsResult {
    pub output: String,
    pub heap: Option<String>,
}

/// A pre-loaded WASM module: human-readable name + raw `.wasm` bytes.
#[derive(Clone, Debug)]
pub struct WasmModule {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct Engine {
    heap_storage: Option<AnyHeapStorage>,
    session_log: Option<SessionLog>,
    heap_memory_max_bytes: usize,
    execution_timeout_secs: u64,
    v8_semaphore: Arc<Semaphore>,
    /// V8's SnapshotCreator is not safe to run concurrently — multiple
    /// snapshot_creator instances on parallel threads cause SIGSEGV.
    /// This mutex serializes stateful V8 execution while stateless
    /// requests proceed in full parallelism.
    snapshot_mutex: Arc<tokio::sync::Mutex<()>>,
    /// WASM modules to inject as globals before every execution.
    wasm_modules: Arc<Vec<WasmModule>>,
    /// WASM host import configuration (OPA policy engine).
    wasm_host_config: Arc<wasm_host::WasmHostConfig>,
}

impl Engine {
    pub fn is_stateful(&self) -> bool {
        self.heap_storage.is_some()
    }

    pub fn new_stateless(heap_memory_max_bytes: usize, execution_timeout_secs: u64, max_concurrent: usize) -> Self {
        Self {
            heap_storage: None,
            session_log: None,
            heap_memory_max_bytes,
            execution_timeout_secs,
            v8_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            snapshot_mutex: Arc::new(tokio::sync::Mutex::new(())),
            wasm_modules: Arc::new(Vec::new()),
            wasm_host_config: Arc::new(wasm_host::WasmHostConfig::default()),
        }
    }

    pub fn new_stateful(
        heap_storage: AnyHeapStorage,
        session_log: Option<SessionLog>,
        heap_memory_max_bytes: usize,
        execution_timeout_secs: u64,
        max_concurrent: usize,
    ) -> Self {
        Self {
            heap_storage: Some(heap_storage),
            session_log,
            heap_memory_max_bytes,
            execution_timeout_secs,
            v8_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            snapshot_mutex: Arc::new(tokio::sync::Mutex::new(())),
            wasm_modules: Arc::new(Vec::new()),
            wasm_host_config: Arc::new(wasm_host::WasmHostConfig::default()),
        }
    }

    /// Set WASM modules to inject as globals before every execution.
    pub fn with_wasm_modules(mut self, modules: Vec<WasmModule>) -> Self {
        self.wasm_modules = Arc::new(modules);
        self
    }

    /// Set WASM host import configuration (OPA policy engine).
    pub fn with_wasm_host_config(mut self, config: wasm_host::WasmHostConfig) -> Self {
        self.wasm_host_config = Arc::new(config);
        self
    }

    /// Core execution — used by both MCP and API.
    ///
    /// Acquires a semaphore permit to bound concurrent V8 executions,
    /// runs V8 on tokio's blocking pool (single thread per request),
    /// and enforces wall-clock timeout at the async layer via `tokio::select!`.
    pub async fn run_js(
        &self,
        code: String,
        heap: Option<String>,
        session: Option<String>,
        heap_memory_max_mb: Option<usize>,
        execution_timeout_secs: Option<u64>,
    ) -> Result<JsResult, String> {
        // Strip TypeScript types before V8 execution (no-op for plain JS)
        let code = strip_typescript_types(&code)?;

        let max_bytes = heap_memory_max_mb
            .map(|mb| mb.max(MIN_HEAP_MEMORY_MB) * 1024 * 1024)
            .unwrap_or(self.heap_memory_max_bytes.max(MIN_HEAP_MEMORY_MB * 1024 * 1024));
        let timeout = execution_timeout_secs.unwrap_or(self.execution_timeout_secs);
        let timeout_dur = Duration::from_secs(timeout);

        // Bound concurrent V8 executions to avoid OS thread exhaustion.
        let _permit = self.v8_semaphore.acquire().await
            .map_err(|_| "V8 semaphore closed".to_string())?;

        let isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>> = Arc::new(Mutex::new(None));

        match &self.heap_storage {
            None => {
                // Stateless mode
                let ih = isolate_handle.clone();
                let wasm = self.wasm_modules.clone();
                let host_cfg = self.wasm_host_config.clone();
                let mut join_handle = tokio::task::spawn_blocking(move || {
                    execute_stateless(&code, max_bytes, ih, &wasm, &host_cfg)
                });

                tokio::select! {
                    biased;
                    res = &mut join_handle => {
                        match res {
                            Ok((Ok(output), _oom)) => Ok(JsResult { output, heap: None }),
                            Ok((Err(e), _oom)) => Err(e),
                            Err(e) => Err(format!("Task join error: {}", e)),
                        }
                    }
                    _ = tokio::time::sleep(timeout_dur) => {
                        if let Some(h) = isolate_handle.lock().unwrap().as_ref() {
                            h.terminate_execution();
                        }
                        // Wait for the blocking task to finish cleanup (holds permit until done).
                        let _ = join_handle.await;
                        Err("Execution timed out: script exceeded the time limit. Try increasing execution_timeout_secs.".to_string())
                    }
                }
            }
            Some(storage) => {
                // Stateful mode — unwrap snapshot before entering blocking task.
                let snapshot = match &heap {
                    Some(h) if !h.is_empty() => storage.get(h).await.ok(),
                    _ => None,
                };
                let raw_snapshot = match snapshot {
                    Some(data) if !data.is_empty() => Some(unwrap_snapshot(&data)?),
                    _ => None,
                };

                let code_for_log = code.clone();
                let ih = isolate_handle.clone();
                let wasm = self.wasm_modules.clone();
                let host_cfg = self.wasm_host_config.clone();

                // V8 SnapshotCreator segfaults under concurrent use — serialize
                // stateful V8 work while keeping the async timeout wrapper.
                let snap_mutex = self.snapshot_mutex.clone();
                let mut join_handle = tokio::task::spawn_blocking(move || {
                    let _guard = snap_mutex.blocking_lock();
                    execute_stateful(&code, raw_snapshot, max_bytes, ih, &wasm, &host_cfg)
                });

                let v8_result = tokio::select! {
                    biased;
                    res = &mut join_handle => {
                        match res {
                            Ok((result, _oom)) => result,
                            Err(e) => Err(format!("Task join error: {}", e)),
                        }
                    }
                    _ = tokio::time::sleep(timeout_dur) => {
                        if let Some(h) = isolate_handle.lock().unwrap().as_ref() {
                            h.terminate_execution();
                        }
                        let _ = join_handle.await;
                        Err("Execution timed out: script exceeded the time limit. Try increasing execution_timeout_secs.".to_string())
                    }
                };

                match v8_result {
                    Ok((output, startup_data, content_hash)) => {
                        if let Err(e) = storage.put(&content_hash, &startup_data).await {
                            return Err(format!("Error saving heap: {}", e));
                        }

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

                        Ok(JsResult { output, heap: Some(content_hash) })
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }

    pub async fn list_sessions(&self) -> Result<Vec<String>, String> {
        match &self.session_log {
            Some(log) => log.list_sessions().await,
            None => Err("Session log not configured".to_string()),
        }
    }

    pub async fn list_session_snapshots(
        &self,
        session: String,
        fields: Option<Vec<String>>,
    ) -> Result<Vec<serde_json::Value>, String> {
        match &self.session_log {
            Some(log) => log.list_entries(&session, fields).await,
            None => Err("Session log not configured".to_string()),
        }
    }
}
