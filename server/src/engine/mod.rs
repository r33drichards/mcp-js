pub mod console;
pub mod execution;
pub mod fetch;
pub mod heap_storage;
pub mod heap_tags;
pub mod module_loader;
pub mod opa;
pub mod session_log;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ffi::c_void;
use std::alloc::{Layout, alloc_zeroed, alloc, dealloc};
use deno_core::v8;
use deno_core::{JsRuntime, JsRuntimeForSnapshot, ModuleSpecifier, RuntimeOptions};
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

use self::console::ConsoleLogState;
use self::execution::{ExecutionId, ExecutionRegistry, ExecutionInfo, ExecutionSummary, ConsoleOutputPage};

use crate::engine::heap_storage::{HeapStorage, AnyHeapStorage};
use crate::engine::heap_tags::{HeapTagStore, HeapTagEntry};
use crate::engine::session_log::{SessionLog, SessionLogEntry};
use wasmparser::Validator;

pub const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 30;
/// Default maximum native memory (bytes) a WASM module may declare when no
/// per-module limit is set. 16 MiB.
pub const DEFAULT_WASM_MAX_BYTES: usize = 16 * 1024 * 1024;
/// Minimum heap memory in MB. deno_core runs bootstrap JavaScript during
/// JsRuntime creation (before our near-heap-limit callback is installed).
/// The heap must be large enough for this bootstrap to complete — smaller
/// values cause `FatalProcessOutOfMemory` → `abort()`.
pub const MIN_HEAP_MEMORY_MB: usize = 8;

// ── V8 initialization ───────────────────────────────────────────────────

pub fn initialize_v8() {
    // deno_core initializes V8 automatically on first JsRuntime creation.
    // Kept for backward compatibility with callers (main.rs, tests, fuzz).
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

/// RAII guard that frees the HeapLimitCallbackData on drop, ensuring no
/// leak even when catch_unwind catches a panic from deno_core/V8.
struct HeapLimitGuard {
    ptr: *mut HeapLimitCallbackData,
}

impl Drop for HeapLimitGuard {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { let _ = Box::from_raw(self.ptr); }
        }
    }
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

/// Size of a single WASM memory page in bytes (64 KiB per the spec).
const WASM_PAGE_BYTES: u64 = 65_536;

/// Estimated bytes per WASM table element (funcref/externref pointer + V8 overhead).
const WASM_TABLE_ELEMENT_BYTES: u64 = 8;

/// Validate that a WASM module's resource declarations fit within the
/// allocated heap budget. Checks both direct declarations (memory/table
/// sections) and imported memories/tables, since both cause V8 to allocate
/// native memory outside the JS heap during compilation.
fn validate_wasm_resources(name: &str, bytes: &[u8], max_memory_bytes: usize) -> Result<(), String> {
    use wasmparser::{Parser, Payload, TypeRef};

    let budget = max_memory_bytes as u64;
    let max_pages = budget / WASM_PAGE_BYTES;
    let max_table_elements = budget / WASM_TABLE_ELEMENT_BYTES;

    for payload in Parser::new(0).parse_all(bytes) {
        match payload {
            Ok(Payload::MemorySection(reader)) => {
                for mem in reader {
                    let mem = mem.map_err(|e| format!("Invalid WASM module '{}': {}", name, e))?;
                    if mem.initial > max_pages {
                        return Err(format!(
                            "WASM module '{}': memory too large ({} pages = {} MiB, budget allows {} pages = {} MiB)",
                            name, mem.initial, mem.initial * 64 / 1024,
                            max_pages, max_pages * 64 / 1024,
                        ));
                    }
                }
            }
            Ok(Payload::TableSection(reader)) => {
                for table in reader {
                    let table = table.map_err(|e| format!("Invalid WASM module '{}': {}", name, e))?;
                    if table.ty.initial > max_table_elements {
                        return Err(format!(
                            "WASM module '{}': table too large ({} elements, budget allows {})",
                            name, table.ty.initial, max_table_elements,
                        ));
                    }
                }
            }
            Ok(Payload::ImportSection(reader)) => {
                for import in reader {
                    let import = import.map_err(|e| format!("Invalid WASM module '{}': {}", name, e))?;
                    match import.ty {
                        TypeRef::Memory(mem) => {
                            if mem.initial > max_pages {
                                return Err(format!(
                                    "WASM module '{}': imported memory too large ({} pages = {} MiB, budget allows {} pages = {} MiB)",
                                    name, mem.initial, mem.initial * 64 / 1024,
                                    max_pages, max_pages * 64 / 1024,
                                ));
                            }
                        }
                        TypeRef::Table(table_ty) => {
                            if table_ty.initial > max_table_elements {
                                return Err(format!(
                                    "WASM module '{}': imported table too large ({} elements, budget allows {})",
                                    name, table_ty.initial, max_table_elements,
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => break, // Structural errors caught by Validator
            _ => {}
        }
    }
    Ok(())
}

/// Check if a WASM module has any imports (used to decide whether
/// auto-instantiation without an imports object is possible).
fn wasm_has_imports(bytes: &[u8]) -> bool {
    use wasmparser::{Parser, Payload};
    for payload in Parser::new(0).parse_all(bytes) {
        if let Ok(Payload::ImportSection(reader)) = payload {
            return reader.count() > 0;
        }
    }
    false
}

/// Compile and inject WASM modules using V8's native API.
///
/// For every module, the compiled `WebAssembly.Module` is exposed as a global
/// named `__wasm_<name>`. This allows JavaScript code to instantiate modules
/// that require an imports object (e.g. WASI modules like SQLite).
///
/// Modules with **no imports** are also auto-instantiated and their exports
/// bound as a global named `<name>` (backwards-compatible behaviour).
///
/// Uses `v8::WasmModuleObject::compile` to compile raw `.wasm` bytes directly
/// (no JS string serialization).
pub fn inject_wasm_modules(
    runtime: &mut JsRuntime,
    modules: &[WasmModule],
    wasm_default_max_bytes: usize,
) -> Result<(), String> {
    if modules.is_empty() {
        return Ok(());
    }

    deno_core::scope!(scope, runtime);
    let global = scope.get_current_context().global(scope);

    // Look up WebAssembly.Instance constructor once.
    let wa_key = v8::String::new(scope, "WebAssembly")
        .ok_or("Failed to create 'WebAssembly' string")?;
    let wa_obj = global
        .get(scope, wa_key.into())
        .ok_or("WebAssembly not found on global")?;
    let wa_obj: v8::Local<v8::Object> = wa_obj.try_into()
        .map_err(|_| "WebAssembly is not an object")?;

    let instance_key = v8::String::new(scope, "Instance")
        .ok_or("Failed to create 'Instance' string")?;
    let instance_ctor = wa_obj
        .get(scope, instance_key.into())
        .ok_or("WebAssembly.Instance not found")?;
    let instance_ctor: v8::Local<v8::Function> = instance_ctor.try_into()
        .map_err(|_| "WebAssembly.Instance is not a function")?;

    let exports_key = v8::String::new(scope, "exports")
        .ok_or("Failed to create 'exports' string")?;

    for m in modules {
        // Pre-validate WASM bytes with wasmparser before handing them to V8.
        // V8's WASM compiler allocates native (non-heap) memory that isn't bounded
        // by our JS heap limits, so malformed modules can OOM the process.
        // wasmparser is a lightweight, safe validator that rejects invalid modules
        // before V8 gets a chance to allocate unbounded memory.
        Validator::new().validate_all(&m.bytes)
            .map_err(|e| format!("Invalid WASM module '{}': {}", m.name, e))?;

        // Reject modules declaring resources that exceed the per-module budget.
        let limit = m.max_memory_bytes.unwrap_or(wasm_default_max_bytes);
        validate_wasm_resources(&m.name, &m.bytes, limit)?;

        // Compile WASM bytes directly via V8's native API — no JS string generation.
        let module_obj = v8::WasmModuleObject::compile(scope, &m.bytes)
            .ok_or_else(|| format!("Failed to compile WASM module '{}'", m.name))?;

        let has_imports = wasm_has_imports(&m.bytes);
        let module_val: v8::Local<v8::Value> = module_obj.into();

        // Always expose the compiled WebAssembly.Module as __wasm_<name>.
        // This lets JS code instantiate modules that need an imports object:
        //   var instance = new WebAssembly.Instance(__wasm_sqlite, { ... });
        let module_global_name = format!("__wasm_{}", m.name);
        let module_key = v8::String::new(scope, &module_global_name)
            .ok_or_else(|| format!("Failed to create module global name for '{}'", m.name))?;
        global.set(scope, module_key.into(), module_val);

        if has_imports {
            // Module requires imports — skip auto-instantiation.
            // The compiled WebAssembly.Module is available as __wasm_<name>
            // for manual instantiation in JavaScript.
            eprintln!(
                "WASM module '{}' has imports — available as '{}' for manual instantiation in JS",
                m.name, module_global_name
            );
        } else {
            // No imports needed — auto-instantiate and expose exports as <name>.
            let instance = instance_ctor
                .new_instance(scope, &[module_val])
                .ok_or_else(|| format!("Failed to instantiate WASM module '{}'", m.name))?;

            let exports = instance
                .get(scope, exports_key.into())
                .ok_or_else(|| format!("Failed to get exports from WASM module '{}'", m.name))?;

            let name_key = v8::String::new(scope, &m.name)
                .ok_or_else(|| format!("Failed to create global name for WASM module '{}'", m.name))?;
            global.set(scope, name_key.into(), exports);
        }
    }

    Ok(())
}

// ── Stateless / stateful execution via deno_core ─────────────────────────
//
// deno_core's JsRuntime wraps V8 Isolate + Context + event loop.
// An IsolateHandle is published for external cancellation (used by
// `run_js` for async timeout via `tokio::select!`).
// Tests and fuzz targets pass a no-op handle.

/// Helper: execute code on a JsRuntime and return the string result.
/// Expects WASM modules to have already been injected.
/// If the result is a Promise, runs the event loop to resolve it.
fn execute_and_stringify(runtime: &mut JsRuntime, code: &str) -> Result<String, String> {
    let global_value = runtime
        .execute_script("<eval>", code.to_string())
        .map_err(|e| format!("{}", e))?;

    // Check if the result is a Promise; if so, run the event loop to resolve it.
    let is_promise = {
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, &global_value);
        local.is_promise()
    };

    if is_promise {
        // Run the event loop to process microtasks and resolve the Promise.
        let handle = tokio::runtime::Handle::current();
        handle
            .block_on(runtime.run_event_loop(Default::default()))
            .map_err(|e| format!("{}", e))?;

        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, &global_value);
        let promise: v8::Local<v8::Promise> = local
            .try_into()
            .map_err(|_| "Failed to cast to Promise".to_string())?;
        match promise.state() {
            v8::PromiseState::Fulfilled => {
                let result = promise.result(scope);
                result
                    .to_string(scope)
                    .map(|s| s.to_rust_string_lossy(scope))
                    .ok_or_else(|| "Failed to convert Promise result to string".to_string())
            }
            v8::PromiseState::Rejected => {
                let result = promise.result(scope);
                let err_str = result
                    .to_string(scope)
                    .map(|s| s.to_rust_string_lossy(scope))
                    .unwrap_or_else(|| "Unknown Promise rejection".to_string());
                Err(err_str)
            }
            v8::PromiseState::Pending => {
                Err("Promise did not resolve after running event loop".to_string())
            }
        }
    } else {
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, global_value);
        local
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .ok_or_else(|| "Failed to convert result to string".to_string())
    }
}

/// Check whether the (already TypeScript-stripped) code uses ES module syntax
/// (`import` / `export` declarations). When it does, the code must be
/// evaluated as an ES module rather than a classic script.
pub fn has_module_syntax(code: &str) -> bool {
    for line in code.lines() {
        let t = line.trim();
        // Static import declaration: import ... from "..."
        // Side-effect import: import "..."
        // Named/default import: import { ... } from "..."
        if t.starts_with("import ") || t.starts_with("import{") || t.starts_with("import\"") || t.starts_with("import'") {
            // Exclude dynamic import() which is an expression, not a declaration.
            // After SWC emitting, a static import always looks like
            //   import ... from "...";
            // while dynamic import is a call expression: import("...")
            if !t.starts_with("import(") {
                return true;
            }
        }
        if t.starts_with("export ") || t.starts_with("export{") || t.starts_with("export*") || t.starts_with("export default") {
            return true;
        }
    }
    false
}

/// Execute code as an ES module, supporting `import` declarations for
/// `npm:`, `jsr:`, and URL specifiers.
fn execute_module(runtime: &mut JsRuntime, code: &str) -> Result<String, String> {
    let handle = tokio::runtime::Handle::current();
    let main_url = ModuleSpecifier::parse("file:///main.js")
        .map_err(|e| format!("internal specifier error: {}", e))?;

    let mod_id = handle
        .block_on(runtime.load_main_es_module_from_code(&main_url, code.to_string()))
        .map_err(|e| format!("{}", e))?;

    let eval_future = runtime.mod_evaluate(mod_id);

    handle
        .block_on(runtime.run_event_loop(Default::default()))
        .map_err(|e| format!("{}", e))?;

    handle.block_on(eval_future).map_err(|e| format!("{}", e))?;

    Ok("undefined".to_string())
}

/// Stateless execution — creates a fresh JsRuntime (no snapshot).
/// Publishes an IsolateHandle for external cancellation.
/// Returns (result, oom_flag).
pub fn execute_stateless(
    code: &str,
    heap_memory_max_bytes: usize,
    isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>>,
    wasm_modules: &[WasmModule],
    wasm_default_max_bytes: usize,
    fetch_config: Option<&fetch::FetchConfig>,
    console_tree: Option<sled::Tree>,
    module_loader_config: Option<&module_loader::ModuleLoaderConfig>,
) -> (Result<String, String>, bool) {
    let oom_flag = Arc::new(AtomicBool::new(false));

    let use_modules = has_module_syntax(code);

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = create_params_with_heap_limit(heap_memory_max_bytes);
        let mut extensions = Vec::new();
        if console_tree.is_some() {
            extensions.push(console::create_extension());
        }
        if fetch_config.is_some() {
            extensions.push(fetch::create_extension());
        }

        // When the code contains ES module syntax (import/export) we need a
        // module loader that can resolve npm:/jsr:/URL specifiers.
        let module_loader: Option<Rc<dyn deno_core::ModuleLoader>> = if use_modules {
            match module_loader_config {
                Some(config) => Some(Rc::new(module_loader::NetworkModuleLoader::with_config(config.clone()))),
                None => Some(Rc::new(module_loader::NetworkModuleLoader::new())),
            }
        } else {
            None
        };

        let mut runtime = JsRuntime::new(RuntimeOptions {
            create_params: Some(params),
            extensions,
            module_loader,
            ..Default::default()
        });

        // Put console log state in OpState.
        if let Some(tree) = console_tree {
            runtime.op_state().borrow_mut().put(ConsoleLogState::new(tree));
        }

        // Put fetch config in OpState if OPA is configured.
        if let Some(fc) = fetch_config {
            runtime.op_state().borrow_mut().put(fc.clone());
        }

        // Publish handle immediately so caller can terminate us.
        *isolate_handle.lock().unwrap() = Some(
            runtime.v8_isolate().thread_safe_handle()
        );

        let cb_data_ptr = install_heap_limit_callback(
            runtime.v8_isolate(), oom_flag.clone()
        );
        let _heap_guard = HeapLimitGuard { ptr: cb_data_ptr };

        // Inject WASM modules as globals via V8 native API.
        let eval_result = match inject_wasm_modules(&mut runtime, wasm_modules, wasm_default_max_bytes) {
            Err(e) => Err(e),
            Ok(()) => {
                // Inject console JS wrapper.
                if let Err(e) = console::inject_console(&mut runtime) {
                    return Err(e);
                }
                // Inject fetch() JS wrapper if OPA is configured.
                if fetch_config.is_some() {
                    if let Err(e) = fetch::inject_fetch(&mut runtime) {
                        return Err(e);
                    }
                }
                if use_modules {
                    execute_module(&mut runtime, code)
                } else {
                    execute_and_stringify(&mut runtime, code)
                }
            }
        };

        // Flush any remaining console output before runtime is dropped.
        console::flush_console(&mut runtime);

        *isolate_handle.lock().unwrap() = None;

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

/// Stateful execution — creates a JsRuntimeForSnapshot, executes code,
/// then takes a snapshot. Publishes an IsolateHandle for external cancellation.
/// Takes raw (already unwrapped) snapshot data. Returns (result, oom_flag).
pub fn execute_stateful(
    code: &str,
    raw_snapshot: Option<Vec<u8>>,
    heap_memory_max_bytes: usize,
    isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>>,
    wasm_modules: &[WasmModule],
    wasm_default_max_bytes: usize,
    fetch_config: Option<&fetch::FetchConfig>,
    console_tree: Option<sled::Tree>,
    module_loader_config: Option<&module_loader::ModuleLoaderConfig>,
) -> (Result<(String, Vec<u8>, String), String>, bool) {
    let oom_flag = Arc::new(AtomicBool::new(false));

    let use_modules = has_module_syntax(code);

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = create_params_with_heap_limit(heap_memory_max_bytes);

        // Box::leak to get &'static [u8] required by RuntimeOptions::startup_snapshot.
        // We reclaim the memory after the runtime is consumed by snapshot().
        let leaked_snapshot: Option<(*mut [u8], &'static [u8])> = raw_snapshot
            .filter(|d| !d.is_empty())
            .map(|data| {
                eprintln!("creating isolate from snapshot...");
                let ptr = Box::into_raw(data.into_boxed_slice());
                let static_ref: &'static [u8] = unsafe { &*ptr };
                (ptr, static_ref)
            });

        if leaked_snapshot.is_none() {
            eprintln!("snapshot not found, creating new isolate...");
        }

        let startup_snapshot = leaked_snapshot.as_ref().map(|(_, s)| *s);

        let mut extensions = Vec::new();
        if console_tree.is_some() {
            extensions.push(console::create_extension());
        }
        if fetch_config.is_some() {
            extensions.push(fetch::create_extension());
        }

        let module_loader: Option<Rc<dyn deno_core::ModuleLoader>> = if use_modules {
            match module_loader_config {
                Some(config) => Some(Rc::new(module_loader::NetworkModuleLoader::with_config(config.clone()))),
                None => Some(Rc::new(module_loader::NetworkModuleLoader::new())),
            }
        } else {
            None
        };

        let mut runtime = JsRuntimeForSnapshot::new(RuntimeOptions {
            create_params: Some(params),
            startup_snapshot,
            extensions,
            module_loader,
            ..Default::default()
        });

        // Put console log state in OpState.
        if let Some(tree) = console_tree {
            runtime.op_state().borrow_mut().put(ConsoleLogState::new(tree));
        }

        // Put fetch config in OpState if OPA is configured.
        if let Some(fc) = fetch_config {
            runtime.op_state().borrow_mut().put(fc.clone());
        }

        // Publish handle immediately so caller can terminate us.
        *isolate_handle.lock().unwrap() = Some(
            runtime.v8_isolate().thread_safe_handle()
        );

        let cb_data_ptr = install_heap_limit_callback(
            runtime.v8_isolate(), oom_flag.clone()
        );
        let _heap_guard = HeapLimitGuard { ptr: cb_data_ptr };

        // Inject WASM modules as globals via V8 native API.
        // Do NOT early-return here — snapshot() must be called below.
        let output_result = match inject_wasm_modules(&mut runtime, wasm_modules, wasm_default_max_bytes) {
            Err(e) => Err(e),
            Ok(()) => {
                // Inject console JS wrapper.
                if let Err(e) = console::inject_console_snapshot(&mut runtime) {
                    return Err(e);
                }
                // Inject fetch() JS wrapper if OPA is configured.
                if fetch_config.is_some() {
                    if let Err(e) = fetch::inject_fetch(&mut runtime) {
                        return Err(e);
                    }
                }
                if use_modules {
                    execute_module(&mut runtime, code)
                } else {
                    execute_and_stringify(&mut runtime, code)
                }
            }
        };

        // Flush any remaining console output before snapshot.
        console::flush_console_snapshot(&mut runtime);

        *isolate_handle.lock().unwrap() = None;

        // Consume runtime to create snapshot (replaces snapshot_creator.create_blob).
        let snapshot_data = runtime.snapshot();

        // Reclaim leaked snapshot input memory (safe: runtime is consumed).
        if let Some((ptr, _)) = leaked_snapshot {
            unsafe { let _ = Box::from_raw(ptr); }
        }

        match output_result {
            Ok(output) => {
                let wrapped = wrap_snapshot(&snapshot_data);
                Ok((output, wrapped.data, wrapped.content_hash))
            }
            Err(e) => Err(e),
        }
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
    /// Max native memory (bytes) this module may declare (linear memory + tables).
    /// Defaults to wasm_default_max_bytes when None.
    pub max_memory_bytes: Option<usize>,
}

#[derive(Clone)]
pub struct Engine {
    heap_storage: Option<AnyHeapStorage>,
    session_log: Option<SessionLog>,
    heap_tag_store: Option<HeapTagStore>,
    heap_memory_max_bytes: usize,
    execution_timeout_secs: u64,
    v8_semaphore: Arc<Semaphore>,
    /// V8's SnapshotCreator is not safe to run concurrently — multiple
    /// snapshot_creator instances on parallel threads cause SIGSEGV.
    /// This mutex serializes stateful V8 execution while stateless
    /// requests proceed in full parallelism.
    snapshot_mutex: Arc<tokio::sync::Mutex<()>>,
    /// Default max native memory (bytes) for WASM modules without a per-module limit.
    wasm_default_max_bytes: usize,
    /// WASM modules to inject as globals before every execution.
    wasm_modules: Arc<Vec<WasmModule>>,
    /// OPA-gated fetch configuration. When Some, `fetch()` is injected into the JS runtime.
    fetch_config: Option<Arc<fetch::FetchConfig>>,
    /// Execution registry for async execution tracking and console output.
    execution_registry: Option<Arc<ExecutionRegistry>>,
    /// Module loader configuration controlling external module access and OPA auditing.
    module_loader_config: Arc<module_loader::ModuleLoaderConfig>,
}

impl Engine {
    pub fn is_stateful(&self) -> bool {
        self.heap_storage.is_some()
    }

    pub fn new_stateless(heap_memory_max_bytes: usize, execution_timeout_secs: u64, max_concurrent: usize) -> Self {
        Self {
            heap_storage: None,
            session_log: None,
            heap_tag_store: None,
            heap_memory_max_bytes,
            execution_timeout_secs,
            v8_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            snapshot_mutex: Arc::new(tokio::sync::Mutex::new(())),
            wasm_default_max_bytes: DEFAULT_WASM_MAX_BYTES,
            wasm_modules: Arc::new(Vec::new()),
            fetch_config: None,
            execution_registry: None,
            module_loader_config: Arc::new(module_loader::ModuleLoaderConfig {
                allow_external: false,
                opa_client: None,
                opa_module_policy: None,
            }),
        }
    }

    pub fn new_stateful(
        heap_storage: AnyHeapStorage,
        session_log: Option<SessionLog>,
        heap_tag_store: Option<HeapTagStore>,
        heap_memory_max_bytes: usize,
        execution_timeout_secs: u64,
        max_concurrent: usize,
    ) -> Self {
        Self {
            heap_storage: Some(heap_storage),
            session_log,
            heap_tag_store,
            heap_memory_max_bytes,
            execution_timeout_secs,
            v8_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            snapshot_mutex: Arc::new(tokio::sync::Mutex::new(())),
            wasm_default_max_bytes: DEFAULT_WASM_MAX_BYTES,
            wasm_modules: Arc::new(Vec::new()),
            fetch_config: None,
            execution_registry: None,
            module_loader_config: Arc::new(module_loader::ModuleLoaderConfig {
                allow_external: false,
                opa_client: None,
                opa_module_policy: None,
            }),
        }
    }

    /// Set the default max native memory for WASM modules without a per-module limit.
    pub fn with_wasm_default_max_bytes(mut self, bytes: usize) -> Self {
        self.wasm_default_max_bytes = bytes;
        self
    }

    /// Set WASM modules to inject as globals before every execution.
    pub fn with_wasm_modules(mut self, modules: Vec<WasmModule>) -> Self {
        self.wasm_modules = Arc::new(modules);
        self
    }

    /// Enable OPA-gated fetch() in the JS runtime.
    pub fn with_fetch_config(mut self, config: fetch::FetchConfig) -> Self {
        self.fetch_config = Some(Arc::new(config));
        self
    }

    /// Set the execution registry for async execution tracking.
    pub fn with_execution_registry(mut self, registry: Arc<ExecutionRegistry>) -> Self {
        self.execution_registry = Some(registry);
        self
    }

    /// Configure module loader settings (external module access and OPA auditing).
    pub fn with_module_loader_config(mut self, config: module_loader::ModuleLoaderConfig) -> Self {
        self.module_loader_config = Arc::new(config);
        self
    }

    /// Submit code for async execution. Returns an execution ID immediately.
    /// V8 runs in a background task. Use `get_execution()` to poll status and
    /// `get_execution_output()` to read console output.
    pub async fn run_js(
        &self,
        code: String,
        heap: Option<String>,
        session: Option<String>,
        heap_memory_max_mb: Option<usize>,
        execution_timeout_secs: Option<u64>,
        tags: Option<HashMap<String, String>>,
    ) -> Result<ExecutionId, String> {
        let registry = self.execution_registry.as_ref()
            .ok_or_else(|| "Execution registry not configured".to_string())?;

        // Strip TypeScript types before V8 execution (no-op for plain JS)
        let code = strip_typescript_types(&code)?;

        let id = uuid::Uuid::new_v4().to_string();
        let console_tree = registry.register(&id)?;

        // For stateful mode, unwrap snapshot before spawning background task.
        let raw_snapshot = if let Some(storage) = &self.heap_storage {
            let snapshot = match &heap {
                Some(h) if !h.is_empty() => storage.get(h).await.ok(),
                _ => None,
            };
            match snapshot {
                Some(data) if !data.is_empty() => Some(unwrap_snapshot(&data)?),
                _ => None,
            }
        } else {
            None
        };

        let engine = self.clone();
        let id_bg = id.clone();

        tokio::spawn(async move {
            engine.execute_in_background(
                id_bg, code, heap, session, heap_memory_max_mb,
                execution_timeout_secs, tags, raw_snapshot, console_tree,
            ).await;
        });

        Ok(id)
    }

    /// Background execution task — runs V8 on the blocking pool with timeout.
    async fn execute_in_background(
        &self,
        id: ExecutionId,
        code: String,
        heap: Option<String>,
        session: Option<String>,
        heap_memory_max_mb: Option<usize>,
        execution_timeout_secs: Option<u64>,
        tags: Option<HashMap<String, String>>,
        raw_snapshot: Option<Vec<u8>>,
        console_tree: sled::Tree,
    ) {
        let registry = match &self.execution_registry {
            Some(r) => r.clone(),
            None => return,
        };

        let max_bytes = heap_memory_max_mb
            .map(|mb| mb.max(MIN_HEAP_MEMORY_MB) * 1024 * 1024)
            .unwrap_or(self.heap_memory_max_bytes.max(MIN_HEAP_MEMORY_MB * 1024 * 1024));
        let timeout = execution_timeout_secs.unwrap_or(self.execution_timeout_secs);
        let timeout_dur = Duration::from_secs(timeout);

        // Bound concurrent V8 executions to avoid OS thread exhaustion.
        let permit = match self.v8_semaphore.acquire().await {
            Ok(p) => p,
            Err(_) => {
                registry.fail(&id, "V8 semaphore closed".to_string());
                return;
            }
        };

        let isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>> = Arc::new(Mutex::new(None));

        match &self.heap_storage {
            None => {
                // Stateless mode
                let ih = isolate_handle.clone();
                let wasm = self.wasm_modules.clone();
                let wasm_default = self.wasm_default_max_bytes;
                let fc = self.fetch_config.clone();
                let ct = console_tree;
                let mlc = self.module_loader_config.clone();
                let mut join_handle = tokio::task::spawn_blocking(move || {
                    execute_stateless(&code, max_bytes, ih, &wasm, wasm_default, fc.as_deref(), Some(ct), Some(&mlc))
                });

                // Publish isolate handle for cancellation once it's available.
                let ih_clone = isolate_handle.clone();
                let reg_clone = registry.clone();
                let id_clone = id.clone();
                tokio::spawn(async move {
                    // Poll briefly for handle to become available.
                    for _ in 0..100 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        if let Some(h) = ih_clone.lock().unwrap().as_ref() {
                            reg_clone.set_isolate_handle(&id_clone, h.clone());
                            break;
                        }
                    }
                });

                let result = tokio::select! {
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
                        let _ = join_handle.await;
                        Err("Execution timed out: script exceeded the time limit.".to_string())
                    }
                };

                match result {
                    Ok(js_result) => registry.complete(&id, js_result.output, None),
                    Err(e) if e.contains("timed out") => registry.timed_out(&id),
                    Err(e) => registry.fail(&id, e),
                }
            }
            Some(storage) => {
                // Stateful mode
                let code_for_log = code.clone();
                let ih = isolate_handle.clone();
                let wasm = self.wasm_modules.clone();
                let wasm_default = self.wasm_default_max_bytes;
                let fc = self.fetch_config.clone();
                let ct = console_tree;
                let mlc = self.module_loader_config.clone();

                let snap_mutex = self.snapshot_mutex.clone();
                let mut join_handle = tokio::task::spawn_blocking(move || {
                    let _guard = snap_mutex.blocking_lock();
                    execute_stateful(&code, raw_snapshot, max_bytes, ih, &wasm, wasm_default, fc.as_deref(), Some(ct), Some(&mlc))
                });

                // Publish isolate handle for cancellation.
                let ih_clone = isolate_handle.clone();
                let reg_clone = registry.clone();
                let id_clone = id.clone();
                tokio::spawn(async move {
                    for _ in 0..100 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        if let Some(h) = ih_clone.lock().unwrap().as_ref() {
                            reg_clone.set_isolate_handle(&id_clone, h.clone());
                            break;
                        }
                    }
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
                        Err("Execution timed out: script exceeded the time limit.".to_string())
                    }
                };

                match v8_result {
                    Ok((output, startup_data, content_hash)) => {
                        if let Err(e) = storage.put(&content_hash, &startup_data).await {
                            registry.fail(&id, format!("Error saving heap: {}", e));
                            return;
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

                        if let (Some(t), Some(tag_store)) = (tags, &self.heap_tag_store) {
                            if let Err(e) = tag_store.set_tags(&content_hash, t).await {
                                tracing::warn!("Failed to store heap tags: {}", e);
                            }
                        }

                        registry.complete(&id, output, Some(content_hash));
                    }
                    Err(e) if e.contains("timed out") => registry.timed_out(&id),
                    Err(e) => registry.fail(&id, e),
                }
            }
        }

        drop(permit);
    }

    // ── Query / cancel methods ───────────────────────────────────────────

    /// Get execution status and result.
    pub fn get_execution(&self, id: &str) -> Result<ExecutionInfo, String> {
        let registry = self.execution_registry.as_ref()
            .ok_or_else(|| "Execution registry not configured".to_string())?;
        registry.get(id).ok_or_else(|| format!("Execution '{}' not found", id))
    }

    /// Get paginated console output for an execution.
    pub fn get_execution_output(
        &self,
        id: &str,
        line_offset: Option<u64>,
        line_limit: Option<u64>,
        byte_offset: Option<u64>,
        byte_limit: Option<u64>,
    ) -> Result<ConsoleOutputPage, String> {
        let registry = self.execution_registry.as_ref()
            .ok_or_else(|| "Execution registry not configured".to_string())?;
        registry.get_console_output(id, line_offset, line_limit, byte_offset, byte_limit)
    }

    /// Cancel a running execution.
    pub fn cancel_execution(&self, id: &str) -> Result<(), String> {
        let registry = self.execution_registry.as_ref()
            .ok_or_else(|| "Execution registry not configured".to_string())?;
        registry.cancel(id)
    }

    /// List all executions.
    pub fn list_executions(&self) -> Result<Vec<ExecutionSummary>, String> {
        let registry = self.execution_registry.as_ref()
            .ok_or_else(|| "Execution registry not configured".to_string())?;
        Ok(registry.list())
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

    pub async fn get_heap_tags(&self, heap: String) -> Result<HashMap<String, String>, String> {
        match &self.heap_tag_store {
            Some(store) => store.get_tags(&heap).await,
            None => Err("Heap tag store not configured".to_string()),
        }
    }

    pub async fn set_heap_tags(
        &self,
        heap: String,
        tags: HashMap<String, String>,
    ) -> Result<(), String> {
        match &self.heap_tag_store {
            Some(store) => store.set_tags(&heap, tags).await,
            None => Err("Heap tag store not configured".to_string()),
        }
    }

    pub async fn delete_heap_tags(
        &self,
        heap: String,
        keys: Option<Vec<String>>,
    ) -> Result<(), String> {
        match &self.heap_tag_store {
            Some(store) => store.delete_tags(&heap, keys).await,
            None => Err("Heap tag store not configured".to_string()),
        }
    }

    pub async fn query_heaps_by_tags(
        &self,
        filter: HashMap<String, String>,
    ) -> Result<Vec<HeapTagEntry>, String> {
        match &self.heap_tag_store {
            Some(store) => store.query_by_tags(filter).await,
            None => Err("Heap tag store not configured".to_string()),
        }
    }
}
