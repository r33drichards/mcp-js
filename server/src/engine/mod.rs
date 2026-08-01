pub mod console;
pub mod execution;
pub mod fetch;
pub mod fetch_auth;
pub mod fs;
pub mod fs_chunker;
pub mod fs_content_merge;
pub mod fs_gc;
pub mod fs_labels;
pub mod fs_merge;
pub mod fs_mount;
pub mod fs_store;
pub mod fs_tree;
pub mod heap_storage;
pub mod heap_tags;
pub mod mcp_client;
pub mod module_loader;
pub mod opa;
pub mod run_js_file;
pub mod session_log;
pub mod subprocess;
pub mod timers;
pub mod wasm_stub;

pub use console::HardeningConfig;

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
use deno_core::{JsRuntime, JsRuntimeForSnapshot, ModuleSpecifier, RuntimeOptions as DenoRuntimeOptions};
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

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU8;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cluster::ClusterNode;
use crate::engine::fs_labels::LabelStore;
use crate::engine::fs_merge::Prefer;
use crate::engine::fs_store::FsStore;
use crate::engine::heap_storage::FileHeapStorage;
use crate::mcp::{ToolCatalog, built_in_tool_catalog};

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
    //
    // Note: V8 145 (bundled with deno_core 0.381) does not support
    // --no-harmony-sharedarraybuffer or --regexp-backtrace-limit flags.
    // SharedArrayBuffer is removed via JS in the hardening step instead.
    // ReDoS is mitigated by the per-execution timeout.
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

/// Parse a 64-char lowercase/uppercase hex string into a 32-byte CA id.
/// Returns `None` for anything that is not exactly a 32-byte hex blob (e.g. a
/// human-readable label name).
pub fn parse_ca_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn refop_str(op: fs_labels::RefOp) -> &'static str {
    match op {
        fs_labels::RefOp::Create => "create",
        fs_labels::RefOp::Push => "push",
        fs_labels::RefOp::Reset => "reset",
        fs_labels::RefOp::Force => "force",
    }
}

/// A label and its current head CA id (hex), for API/CLI responses.
#[derive(Debug, Clone, serde::Serialize, uniffi::Record)]
pub struct FsLabelView {
    pub name: String,
    pub ca_id: String,
}

/// One reflog entry, hex-rendered, for API/CLI responses.
#[derive(Debug, Clone, serde::Serialize, uniffi::Record)]
pub struct FsRefLogView {
    pub at: i64,
    pub from: Option<String>,
    pub to: String,
    pub op: String,
    /// Optional human note recorded with the move (omitted when absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Outcome of an [`Engine::fs_push`].
#[derive(Debug, Clone, serde::Serialize, uniffi::Enum)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum FsPushOutcome {
    /// The label now points at `ca_id`.
    Advanced { label: String, ca_id: String },
    /// The label moved since the caller pulled — re-pull and retry (or force).
    Rejected {
        label: String,
        current: Option<String>,
    },
}

/// Result of an [`Engine::fs_merge`].
#[derive(Debug, Clone, serde::Serialize, uniffi::Enum)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum FsMergeResult {
    /// A clean merge; the new snapshot has this CA id.
    Merged { ca_id: String },
    /// Unresolved conflicts; no snapshot was produced.
    Conflict { conflicts: Vec<FsMergeConflictView> },
}

/// One conflicting path. Each side is a content id (hex of the entry) when the
/// file is present on that side, or `null` when it is absent (delete). For text
/// files the response also carries diff3 conflict markers and unified diffs so
/// the caller can review and resolve at line level.
#[derive(Debug, Clone, serde::Serialize, uniffi::Record)]
pub struct FsMergeConflictView {
    pub path: String,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    /// Detected content type: `text`, `binary`, `sqlite`, or `modify/delete`.
    pub kind: String,
    /// diff3-marked text to edit and write back (text conflicts only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markers: Option<String>,
    /// Unified diff base -> ours (text conflicts only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_ours: Option<String>,
    /// Unified diff base -> theirs (text conflicts only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_theirs: Option<String>,
}

/// A stable content id for a manifest entry: blake3 of its canonical encoding.
/// Lets a caller tell whether two sides' versions of a path differ.
fn entry_content_id(e: &fs_store::Entry) -> String {
    let bytes = bincode::serialize(e).unwrap_or_default();
    ca_to_hex(blake3::hash(&bytes).as_bytes())
}

/// Render a 32-byte CA id as lowercase hex.
pub fn ca_to_hex(id: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in id {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn strip_typescript_types(code: &str) -> Result<String, String> {
    let cm: Lrc<SourceMap> = Default::default();

    let fm = cm.new_source_file(
        swc_core::common::FileName::Anon.into(),
        code.to_string(),
    );

    let comments = SingleThreadedComments::default();

    let lexer = Lexer::new(
        // JSX/TSX is intentionally disabled: the pipeline only strips TypeScript
        // types and does not transform JSX, so accepting JSX would emit code that
        // V8 rejects at runtime. Disabling tsx also re-enables `<T>value` type
        // assertions, which are ambiguous with JSX when tsx is on.
        Syntax::Typescript(TsSyntax {
            tsx: false,
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

/// Global counter for unique module URLs, avoiding conflicts when restoring
/// from a V8 heap snapshot that already has a registered module.
static MODULE_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Execute code as an ES module. All code is always executed as a module,
/// which supports `import` declarations, `export`, and top-level `await`.
// Build the dedicated current-thread runtime each isolate runs on.
//
// V8/deno_core is single-threaded and deno_core's async op driver dispatches ops
// through `deno_unsync`, which requires a `RuntimeFlavor::CurrentThread` runtime
// (on a multi-thread runtime it panics in debug and deadlocks fs ops in
// release). We are called from `spawn_blocking` (a blocking task), so building
// and driving a fresh current-thread runtime here is allowed — no dedicated OS
// thread needed.
//
// Ops that need the server's multi-thread runtime don't run their I/O here: the
// S3 client (heap + fs blobs) captures that runtime's Handle at construction and
// bridges via `handle.spawn(...).await`, and cross-worker fs blobs are pre-staged
// into the local cache by `build_fs_mount` before the isolate runs.
fn isolate_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to create current-thread runtime: {}", e))
}

fn execute_module(
    rt: &tokio::runtime::Runtime,
    runtime: &mut JsRuntime,
    code: &str,
) -> Result<(), String> {
    let id = MODULE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let main_url = ModuleSpecifier::parse(&format!("file:///main_{}.js", id))
        .map_err(|e| format!("internal specifier error: {}", e))?;

    // CRITICAL: run the load, module evaluation, AND event loop inside ONE
    // `block_on`. `mod_evaluate` runs the module's top-level synchronous code
    // immediately (up to the first await), which submits async ops — so it must
    // execute under this current-thread runtime's context too, not between
    // `block_on` calls (where the ambient multi-thread runtime is current and
    // deno_unsync's op spawn would panic/deadlock).
    rt.block_on(async move {
        let mod_id = runtime
            .load_side_es_module_from_code(&main_url, code.to_string())
            .await
            .map_err(|e| format!("{}", e))?;

        let eval_future = runtime.mod_evaluate(mod_id);

        runtime
            .run_event_loop(Default::default())
            .await
            .map_err(|e| format!("{}", e))?;

        eval_future.await.map_err(|e| format!("{}", e))?;

        Ok(())
    })
}

/// Configuration bundle for `execute_stateless` / `execute_stateful`.
///
/// Only `heap_memory_max_bytes` is required; everything else defaults to
/// sensible values (`None`, `&[]`, fresh `Arc<Mutex<None>>`).
pub struct ExecutionConfig<'a> {
    pub heap_memory_max_bytes: usize,
    pub isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>>,
    pub wasm_modules: &'a [WasmModule],
    pub wasm_default_max_bytes: usize,
    pub fetch_config: Option<&'a fetch::FetchConfig>,
    pub fs_config: Option<&'a fs::FsConfig>,
    /// Optional overlay mount. When present, the fs ops operate on this virtual
    /// filesystem instead of the host. Independent of the heap snapshot handle.
    pub fs_mount: Option<fs::FsMountHandle>,
    pub mcp_headers: Option<serde_json::Value>,
    pub subprocess_config: Option<&'a subprocess::SubprocessConfig>,
    pub console_tree: Option<sled::Tree>,
    pub module_loader_config: Option<&'a module_loader::ModuleLoaderConfig>,
    pub mcp_config: Option<&'a mcp_client::McpConfig>,
    /// Per-mitigation sandbox hardening. Default is all-off (unhardened).
    pub hardening: console::HardeningConfig,
}

impl<'a> ExecutionConfig<'a> {
    pub fn new(heap_memory_max_bytes: usize) -> Self {
        Self {
            heap_memory_max_bytes,
            isolate_handle: Arc::new(Mutex::new(None)),
            wasm_modules: &[],
            wasm_default_max_bytes: heap_memory_max_bytes,
            fetch_config: None,
            fs_config: None,
            fs_mount: None,
            mcp_headers: None,
            subprocess_config: None,
            console_tree: None,
            module_loader_config: None,
            mcp_config: None,
            hardening: console::HardeningConfig::default(),
        }
    }

    /// Set the per-mitigation sandbox hardening configuration.
    pub fn hardening(mut self, hardening: console::HardeningConfig) -> Self {
        self.hardening = hardening;
        self
    }

    pub fn mcp_headers(mut self, mcp_headers: Option<serde_json::Value>) -> Self {
        self.mcp_headers = mcp_headers;
        self
    }

    pub fn maybe_subprocess_config(mut self, config: Option<&'a subprocess::SubprocessConfig>) -> Self {
        self.subprocess_config = config;
        self
    }

    pub fn isolate_handle(mut self, handle: Arc<Mutex<Option<v8::IsolateHandle>>>) -> Self {
        self.isolate_handle = handle;
        self
    }

    pub fn wasm_modules(mut self, modules: &'a [WasmModule]) -> Self {
        self.wasm_modules = modules;
        self
    }

    pub fn wasm_default_max_bytes(mut self, bytes: usize) -> Self {
        self.wasm_default_max_bytes = bytes;
        self
    }

    #[allow(dead_code)]
    pub fn fetch_config(mut self, config: &'a fetch::FetchConfig) -> Self {
        self.fetch_config = Some(config);
        self
    }

    pub fn console_tree(mut self, tree: sled::Tree) -> Self {
        self.console_tree = Some(tree);
        self
    }

    pub fn module_loader_config(mut self, config: &'a module_loader::ModuleLoaderConfig) -> Self {
        self.module_loader_config = Some(config);
        self
    }

    pub fn maybe_fetch_config(mut self, config: Option<&'a fetch::FetchConfig>) -> Self {
        self.fetch_config = config;
        self
    }

    pub fn maybe_fs_config(mut self, config: Option<&'a fs::FsConfig>) -> Self {
        self.fs_config = config;
        self
    }

    pub fn maybe_fs_mount(mut self, mount: Option<fs::FsMountHandle>) -> Self {
        self.fs_mount = mount;
        self
    }

    pub fn maybe_mcp_config(mut self, config: Option<&'a mcp_client::McpConfig>) -> Self {
        self.mcp_config = config;
        self
    }

}

/// Stateless execution — creates a fresh JsRuntime (no snapshot).
/// Publishes an IsolateHandle for external cancellation.
/// Returns (result, oom_flag).
pub fn execute_stateless(
    code: &str,
    config: ExecutionConfig<'_>,
) -> (Result<String, String>, bool) {
    let ExecutionConfig {
        heap_memory_max_bytes,
        isolate_handle,
        wasm_modules,
        wasm_default_max_bytes,
        fetch_config,
        fs_config,
        fs_mount,
        mcp_headers,
            subprocess_config,
        console_tree,
        module_loader_config,
        mcp_config,
        hardening,
    } = config;
    let oom_flag = Arc::new(AtomicBool::new(false));

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = create_params_with_heap_limit(heap_memory_max_bytes);
        let mut extensions = Vec::new();
        if console_tree.is_some() {
            extensions.push(console::create_extension());
        }
        if fetch_config.is_some() {
            extensions.push(fetch::create_extension());
        if subprocess_config.is_some() {
            extensions.push(subprocess::create_extension());
        }
        }
        if fs_config.is_some() {
            extensions.push(fs::create_extension());
        }
        if mcp_config.is_some() {
            extensions.push(mcp_client::create_extension());
        }
        extensions.push(timers::create_extension());

        // Always create a module loader — all code runs as ES modules.
        let module_loader: Rc<dyn deno_core::ModuleLoader> = match module_loader_config {
            Some(config) => Rc::new(module_loader::NetworkModuleLoader::with_config(config.clone())),
            None => Rc::new(module_loader::NetworkModuleLoader::new()),
        };

        let mut runtime = JsRuntime::new(DenoRuntimeOptions {
            create_params: Some(params),
            extensions,
            module_loader: Some(module_loader),
            ..Default::default()
        });

        // Current-thread runtime that drives this isolate's async ops (see
        // `execute_module` / `isolate_runtime`).
        let rt = isolate_runtime()?;

        // Put console log state in OpState.
        if let Some(tree) = console_tree {
            runtime.op_state().borrow_mut().put(ConsoleLogState::new(tree));
        }

        // Put fetch config in OpState if OPA is configured.
        if let Some(fc) = fetch_config {
            runtime.op_state().borrow_mut().put(fc.clone());
        }

        // Put fs config in OpState if filesystem policies are configured.
        if let Some(fsc) = fs_config {
            let fsc = fsc.clone().with_mcp_headers(mcp_headers.clone());
            runtime.op_state().borrow_mut().put(fsc);

            // Attach the session's overlay mount, if any, so fs ops operate on
            // the virtual filesystem (behind the same policy gate).
            if let Some(mount) = fs_mount.clone() {
                runtime.op_state().borrow_mut().put(mount);
            }

            // Put subprocess config in OpState if subprocess policies are configured.
            if let Some(sc) = subprocess_config {
                runtime.op_state().borrow_mut().put(sc.clone());
            }
        }

        // Put MCP config in OpState if MCP servers are configured.
        if let Some(mc) = mcp_config {
            runtime.op_state().borrow_mut().put(mc.clone());
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
                // Neutralize dangerous built-in ops (op_panic, print).
                if let Err(e) = console::neutralize_dangerous_ops(&mut runtime) {
                    return Err(e);
                }
                // Inject atob/btoa (always available).
                if let Err(e) = console::inject_base64(&mut runtime) {
                    return Err(e);
                }
                // Inject Blob/File/FormData (always available).
                if let Err(e) = console::inject_web_apis(&mut runtime) {
                    return Err(e);
                }
                // Inject fetch() JS wrapper if OPA is configured.
                if fetch_config.is_some() {
                    if let Err(e) = fetch::inject_fetch(&mut runtime) {
                        return Err(e);
                    }
                // Inject subprocess JS wrapper if subprocess policies are configured.
                if subprocess_config.is_some() {
                    if let Err(e) = subprocess::inject_subprocess(&mut runtime) {
                        return Err(e);
                    }
                }
                }
                // Inject fs JS wrapper if filesystem policies are configured.
                if fs_config.is_some() {
                    if let Err(e) = fs::inject_fs(&mut runtime) {
                        return Err(e);
                    }
                }
                // Inject mcp JS wrapper if MCP servers are configured.
                if mcp_config.is_some() {
                    if let Err(e) = mcp_client::inject_mcp(&mut runtime) {
                        return Err(e);
                    }
                }
                // Inject setTimeout/clearTimeout (always available).
                if let Err(e) = timers::inject_timers(&mut runtime) {
                    return Err(e);
                }
                // Harden sandbox: freeze ops, neutralize introspection, remove __bootstrap.
                // Must run after all inject_* calls and before user code.
                if let Err(e) = console::harden_runtime(&mut runtime, hardening) {
                    return Err(e);
                }
                execute_module(&rt, &mut runtime, code)
            }
        };

        // Flush any remaining console output before runtime is dropped.
        console::flush_console(&mut runtime);

        *isolate_handle.lock().unwrap() = None;

        eval_result
    }));

    let oom = oom_flag.load(Ordering::SeqCst);
    match result {
        Ok(Ok(())) => (Ok(String::new()), oom),
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
    config: ExecutionConfig<'_>,
) -> (Result<(String, Vec<u8>, String), String>, bool) {
    let ExecutionConfig {
        heap_memory_max_bytes,
        isolate_handle,
        wasm_modules,
        wasm_default_max_bytes,
        fetch_config,
        fs_config,
        fs_mount,
        mcp_headers,
            subprocess_config,
        console_tree,
        module_loader_config,
        mcp_config,
        hardening,
    } = config;
    let oom_flag = Arc::new(AtomicBool::new(false));

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
        if subprocess_config.is_some() {
            extensions.push(subprocess::create_extension());
        }
        }
        if fs_config.is_some() {
            extensions.push(fs::create_extension());
        }
        if mcp_config.is_some() {
            extensions.push(mcp_client::create_extension());
        }
        extensions.push(timers::create_extension());

        // Always create a module loader — all code runs as ES modules.
        let module_loader: Rc<dyn deno_core::ModuleLoader> = match module_loader_config {
            Some(config) => Rc::new(module_loader::NetworkModuleLoader::with_config(config.clone())),
            None => Rc::new(module_loader::NetworkModuleLoader::new()),
        };

        let mut runtime = JsRuntimeForSnapshot::new(DenoRuntimeOptions {
            create_params: Some(params),
            startup_snapshot,
            extensions,
            module_loader: Some(module_loader),
            ..Default::default()
        });

        // Current-thread runtime that drives this isolate's async ops (see
        // `execute_module` / `isolate_runtime`).
        let rt = isolate_runtime()?;

        // Put console log state in OpState.
        if let Some(tree) = console_tree {
            runtime.op_state().borrow_mut().put(ConsoleLogState::new(tree));
        }

        // Put fetch config in OpState if OPA is configured.
        if let Some(fc) = fetch_config {
            runtime.op_state().borrow_mut().put(fc.clone());
        }

        // Put fs config in OpState if filesystem policies are configured.
        if let Some(fsc) = fs_config {
            let fsc = fsc.clone().with_mcp_headers(mcp_headers.clone());
            runtime.op_state().borrow_mut().put(fsc);

            // Attach the session's overlay mount, if any, so fs ops operate on
            // the virtual filesystem (behind the same policy gate).
            if let Some(mount) = fs_mount.clone() {
                runtime.op_state().borrow_mut().put(mount);
            }

            // Put subprocess config in OpState if subprocess policies are configured.
            if let Some(sc) = subprocess_config {
                runtime.op_state().borrow_mut().put(sc.clone());
            }
        }

        // Put MCP config in OpState if MCP servers are configured.
        if let Some(mc) = mcp_config {
            runtime.op_state().borrow_mut().put(mc.clone());
        }

        // Publish handle immediately so caller can terminate us.
        *isolate_handle.lock().unwrap() = Some(
            runtime.v8_isolate().thread_safe_handle()
        );

        let cb_data_ptr = install_heap_limit_callback(
            runtime.v8_isolate(), oom_flag.clone()
        );
        let _heap_guard = HeapLimitGuard { ptr: cb_data_ptr };

        // When restoring from a snapshot, the JS-level setup (console wrappers,
        // sandbox hardening, WASM globals) is already baked in. Re-running these
        // scripts would fail because the sandbox is locked down (Deno.core is
        // non-configurable, Deno.core.ops is frozen). Only inject on fresh runtimes.
        let has_snapshot = leaked_snapshot.is_some();

        let output_result = if has_snapshot {
            execute_module(&rt, &mut runtime, code)
        } else {
            // Inject WASM modules as globals via V8 native API.
            // Do NOT early-return here — snapshot() must be called below.
            match inject_wasm_modules(&mut runtime, wasm_modules, wasm_default_max_bytes) {
                Err(e) => Err(e),
                Ok(()) => {
                    // Inject console JS wrapper.
                    if let Err(e) = console::inject_console_snapshot(&mut runtime) {
                        return Err(e);
                    }
                    // Neutralize dangerous built-in ops (op_panic, print).
                    if let Err(e) = console::neutralize_dangerous_ops(&mut runtime) {
                        return Err(e);
                    }
                    // Inject atob/btoa (always available).
                    if let Err(e) = console::inject_base64_snapshot(&mut runtime) {
                        return Err(e);
                    }
                    // Inject Blob/File/FormData (always available).
                    if let Err(e) = console::inject_web_apis_snapshot(&mut runtime) {
                        return Err(e);
                    }
                    // Inject fetch() JS wrapper if OPA is configured.
                    if fetch_config.is_some() {
                        if let Err(e) = fetch::inject_fetch(&mut runtime) {
                            return Err(e);
                        }
                // Inject subprocess JS wrapper if subprocess policies are configured.
                if subprocess_config.is_some() {
                    if let Err(e) = subprocess::inject_subprocess(&mut runtime) {
                        return Err(e);
                    }
                }
                    }
                    // Inject fs JS wrapper if filesystem policies are configured.
                    if fs_config.is_some() {
                        if let Err(e) = fs::inject_fs(&mut runtime) {
                            return Err(e);
                        }
                    }
                    // Inject mcp JS wrapper if MCP servers are configured.
                    if mcp_config.is_some() {
                        if let Err(e) = mcp_client::inject_mcp(&mut runtime) {
                            return Err(e);
                        }
                    }
                    // Inject setTimeout/clearTimeout (always available).
                    if let Err(e) = timers::inject_timers(&mut runtime) {
                        return Err(e);
                    }
                    // Harden sandbox: freeze ops, neutralize introspection, remove __bootstrap.
                    // Must run after all inject_* calls and before user code.
                    if let Err(e) = console::harden_runtime(&mut runtime, hardening) {
                        return Err(e);
                    }
                    execute_module(&rt, &mut runtime, code)
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
            Ok(()) => {
                let wrapped = wrap_snapshot(&snapshot_data);
                Ok((String::new(), wrapped.data, wrapped.content_hash))
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
    /// Optional operator-supplied description used for the module's MCP stub
    /// tool. When set, it is shown to downstream agents alongside the
    /// auto-generated usage hint. Defaults to None (auto-generated text only).
    pub description: Option<String>,
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
    /// Controls whether loaded WASM modules are advertised as stub tools on
    /// the MCP surface, and under what name prefix.
    wasm_stub_config: wasm_stub::WasmStubConfig,
    /// OPA-gated fetch configuration. When Some, `fetch()` is injected into the JS runtime.
    fetch_config: Option<Arc<fetch::FetchConfig>>,
    /// Policy-gated filesystem configuration. When Some, `fs` is injected into the JS runtime.
    fs_config: Option<Arc<fs::FsConfig>>,
    /// Execution registry for async execution tracking and console output.
    execution_registry: Option<Arc<ExecutionRegistry>>,
    /// Module loader configuration controlling external module access and OPA auditing.
    module_loader_config: Arc<module_loader::ModuleLoaderConfig>,
    /// MCP client manager for programmatic tool calling from JS.
    mcp_client_manager: Option<Arc<mcp_client::McpClientManager>>,
    /// OPA policy chain for MCP tool calls (`mcp.callTool()`).
    mcp_tools_policy_chain: Option<Arc<opa::PolicyChain>>,
    /// Policy-gated subprocess configuration. When Some, subprocess execution is injected into the JS runtime.
    subprocess_config: Option<Arc<subprocess::SubprocessConfig>>,
    /// Optional override for the MCP server `instructions` field (the "system
    /// prompt" returned during `initialize`). When `None`, the built-in default
    /// is used.
    instructions_override: Option<Arc<str>>,
    /// Optional override for the `run_js` tool description advertised in
    /// `tools/list`. When `None`, the compiled-in description is used.
    run_js_description_override: Option<Arc<str>>,
    /// Controls whether `run_js` may read its code from a file on the server's
    /// own filesystem (the `file` parameter). `None` disables it entirely (the
    /// default); `Some` either allows all paths or gates them behind a policy.
    run_js_file_policy: Option<run_js_file::RunJsFilePolicy>,
    /// Content-addressed object store for fs snapshots. Shares the heap blob
    /// backend. When set, `run_js` may mount a snapshot via the `fs` parameter.
    fs_store: Option<Arc<fs_store::FsStore>>,
    /// Mutable label → manifest pointer store with reflog.
    label_store: Option<Arc<fs_labels::LabelStore>>,
    /// Policy chain gating fs snapshot pointer moves (pull/push/reset/label).
    fs_snapshot_policy_chain: Option<Arc<opa::PolicyChain>>,
    /// Per-mitigation sandbox hardening. Default is all-off (unhardened); each
    /// mitigation is opt-in via the `--harden-*` CLI flags.
    hardening: console::HardeningConfig,
}

/// Builder for `Engine::run_js()`. Only `code` is required; everything else
/// defaults to `None`.
pub struct RunJsRequest<'a> {
    engine: &'a Engine,
    code: String,
    /// Optional path to a file on the server's filesystem whose contents are
    /// executed instead of `code`. Policy-gated (see `run_js_file_policy`).
    file: Option<String>,
    heap: Option<String>,
    /// Optional fs snapshot handle (label name or 64-hex CA id). Independent of
    /// `heap` — the two are never coupled.
    fs: Option<String>,
    session: Option<String>,
    heap_memory_max_mb: Option<usize>,
    execution_timeout_secs: Option<u64>,
    tags: Option<HashMap<String, String>>,
    mcp_headers: Option<serde_json::Value>,
}

impl<'a> RunJsRequest<'a> {
    /// Read the code from a file on the server's filesystem instead of an
    /// inline `code` string. Subject to the engine's `run_js_file` policy.
    pub fn file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Set the file path from an `Option`, leaving it unset when `None`.
    pub fn maybe_file(mut self, file: Option<String>) -> Self {
        self.file = file;
        self
    }

    pub fn heap(mut self, heap: impl Into<String>) -> Self {
        self.heap = Some(heap.into());
        self
    }

    /// Mount an fs snapshot (label name or 64-hex CA id) for this execution.
    pub fn fs(mut self, fs: impl Into<String>) -> Self {
        self.fs = Some(fs.into());
        self
    }

    /// Set the fs handle from an `Option`, leaving it unset when `None`.
    pub fn maybe_fs(mut self, fs: Option<String>) -> Self {
        self.fs = fs;
        self
    }

    pub fn session(mut self, session: impl Into<String>) -> Self {
        self.session = Some(session.into());
        self
    }

    pub fn maybe_session(mut self, session: Option<String>) -> Self {
        self.session = session;
        self
    }

    pub fn heap_memory_max_mb(mut self, mb: usize) -> Self {
        self.heap_memory_max_mb = Some(mb);
        self
    }

    pub fn execution_timeout_secs(mut self, secs: u64) -> Self {
        self.execution_timeout_secs = Some(secs);
        self
    }

    pub fn tags(mut self, tags: HashMap<String, String>) -> Self {
        self.tags = Some(tags);
        self
    }

    pub fn mcp_headers(mut self, headers: serde_json::Value) -> Self {
        self.mcp_headers = Some(headers);
        self
    }

    pub fn maybe_mcp_headers(mut self, headers: Option<serde_json::Value>) -> Self {
        self.mcp_headers = headers;
        self
    }


    pub async fn execute(self) -> Result<ExecutionId, String> {
        self.engine.run_js_inner(
            self.code,
            self.file,
            self.heap,
            self.fs,
            self.session,
            self.heap_memory_max_mb,
            self.execution_timeout_secs,
            self.tags,
            self.mcp_headers,
        ).await
    }
}

impl Engine {
    pub fn is_stateful(&self) -> bool {
        self.heap_storage.is_some()
    }

    /// Heap persistence (V8 heap snapshots) is configured. Alias of
    /// `is_stateful`, kept for readability now that heap and fs are independent.
    pub fn heap_enabled(&self) -> bool {
        self.heap_storage.is_some()
    }

    /// Filesystem persistence (content-addressed `/work` snapshots) is configured.
    pub fn fs_enabled(&self) -> bool {
        self.fs_store.is_some()
    }

    /// True when the engine carries any per-session state (heap and/or fs), and
    /// therefore needs the session-capable MCP surface and a session log.
    pub fn session_capable(&self) -> bool {
        self.heap_enabled() || self.fs_enabled()
    }

    /// Attach a session log to an existing engine. Required for per-session
    /// state resolution on either axis (heap or fs); the stateful constructor
    /// passes one inline, but a stateless (heap-off) engine with fs persistence
    /// needs one too.
    pub fn with_session_log(mut self, log: SessionLog) -> Self {
        self.session_log = Some(log);
        self
    }

    /// Attach a heap-tag store to an existing engine (heap persistence only).
    pub fn with_heap_tag_store(mut self, store: HeapTagStore) -> Self {
        self.heap_tag_store = Some(store);
        self
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
            wasm_stub_config: wasm_stub::WasmStubConfig::default(),
            fetch_config: None,
            fs_config: None,
            execution_registry: None,
            module_loader_config: Arc::new(module_loader::ModuleLoaderConfig {
                allow_external: false,
                policy_chain: None,
            }),
            mcp_client_manager: None,
            mcp_tools_policy_chain: None,
            subprocess_config: None,
            instructions_override: None,
            run_js_description_override: None,
            run_js_file_policy: None,
            fs_store: None,
            label_store: None,
            fs_snapshot_policy_chain: None,
            hardening: console::HardeningConfig::default(),
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
            wasm_stub_config: wasm_stub::WasmStubConfig::default(),
            fetch_config: None,
            fs_config: None,
            execution_registry: None,
            module_loader_config: Arc::new(module_loader::ModuleLoaderConfig {
                allow_external: false,
                policy_chain: None,
            }),
            mcp_client_manager: None,
            mcp_tools_policy_chain: None,
            subprocess_config: None,
            instructions_override: None,
            run_js_description_override: None,
            run_js_file_policy: None,
            fs_store: None,
            label_store: None,
            fs_snapshot_policy_chain: None,
            hardening: console::HardeningConfig::default(),
        }
    }

    /// Set the default max native memory for WASM modules without a per-module limit.
    pub fn with_wasm_default_max_bytes(mut self, bytes: usize) -> Self {
        self.wasm_default_max_bytes = bytes;
        self
    }

    /// Set the per-mitigation sandbox hardening configuration. Defaults to
    /// all-off; mitigations are opt-in via the `--harden-*` CLI flags.
    pub fn with_hardening(mut self, hardening: console::HardeningConfig) -> Self {
        self.hardening = hardening;
        self
    }

    /// Set WASM modules to inject as globals before every execution.
    pub fn with_wasm_modules(mut self, modules: Vec<WasmModule>) -> Self {
        self.wasm_modules = Arc::new(modules);
        self
    }

    /// Configure how loaded WASM modules are advertised as stub tools on the
    /// MCP surface.
    pub fn with_wasm_stub_config(mut self, config: wasm_stub::WasmStubConfig) -> Self {
        self.wasm_stub_config = config;
        self
    }

    /// Generate stub `Tool` definitions for every loaded WASM module. Used by
    /// the MCP server side to expose modules for discovery. Returns an empty
    /// vec when WASM stubbing is disabled or no modules are loaded.
    pub fn wasm_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        wasm_stub::stub_tools(&self.wasm_modules, &self.wasm_stub_config)
    }

    /// If `name` is a stub for a loaded WASM module, build the instructional
    /// `CallToolResult` telling the caller to use the module via `run_js`.
    /// Returns `None` if stubs are disabled or `name` matches no module.
    pub fn wasm_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        wasm_stub::stub_call_response(&self.wasm_modules, &self.wasm_stub_config, name, arguments)
    }

    /// Enable OPA-gated fetch() in the JS runtime.
    pub fn with_fetch_config(mut self, config: fetch::FetchConfig) -> Self {
        self.fetch_config = Some(Arc::new(config));
        self
    }

    /// Enable policy-gated filesystem access in the JS runtime.
    pub fn with_fs_config(mut self, config: fs::FsConfig) -> Self {
        self.fs_config = Some(Arc::new(config));
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

    /// Enable MCP client tool calling from the JS runtime.
    pub fn with_mcp_client_manager(mut self, manager: mcp_client::McpClientManager) -> Self {
        self.mcp_client_manager = Some(Arc::new(manager));
        self
    }

    /// Get the MCP client manager (if any). Used by the MCP server side to
    /// expose upstream tools as stubs.
    pub fn mcp_client_manager(&self) -> Option<Arc<mcp_client::McpClientManager>> {
        self.mcp_client_manager.clone()
    }

    /// Set OPA policy chain for MCP tool calls (`mcp.callTool()`).
    pub fn with_mcp_tools_policy_chain(mut self, chain: Arc<opa::PolicyChain>) -> Self {
        self.mcp_tools_policy_chain = Some(chain);
        self
    }

    /// Submit code for async execution. Returns an execution ID immediately.
    /// Enable policy-gated subprocess execution in the JS runtime.
    pub fn with_subprocess_config(mut self, config: subprocess::SubprocessConfig) -> Self {
        self.subprocess_config = Some(Arc::new(config));
        self
    }

    /// Override the MCP server `instructions` (the "system prompt" returned
    /// during `initialize`).
    pub fn with_instructions_override(mut self, text: String) -> Self {
        self.instructions_override = Some(Arc::from(text));
        self
    }

    /// Get the MCP server `instructions` override, if one was configured.
    pub fn instructions_override(&self) -> Option<Arc<str>> {
        self.instructions_override.clone()
    }

    /// Override the `run_js` tool description advertised in `tools/list`.
    pub fn with_run_js_description_override(mut self, text: String) -> Self {
        self.run_js_description_override = Some(Arc::from(text));
        self
    }

    /// Get the `run_js` tool description override, if one was configured.
    pub fn run_js_description_override(&self) -> Option<Arc<str>> {
        self.run_js_description_override.clone()
    }

    /// Enable `run_js` file-path reads, either allowing all paths or gating
    /// them behind a policy. When this is never called, the `file` parameter
    /// is rejected.
    pub fn with_run_js_file_policy(mut self, policy: run_js_file::RunJsFilePolicy) -> Self {
        self.run_js_file_policy = Some(policy);
        self
    }

    /// Configure the content-addressed fs snapshot store (the object store) and
    /// the label/reflog pointer store. Both are required for the `fs` mount
    /// parameter and the `fs_*` tools to function.
    pub fn with_fs_snapshots(
        mut self,
        store: Arc<fs_store::FsStore>,
        labels: Arc<fs_labels::LabelStore>,
    ) -> Self {
        self.fs_store = Some(store);
        self.label_store = Some(labels);
        self
    }

    /// Gate fs snapshot pointer moves (pull/push/reset/label) behind a policy.
    pub fn with_fs_snapshot_policy(mut self, chain: Arc<opa::PolicyChain>) -> Self {
        self.fs_snapshot_policy_chain = Some(chain);
        self
    }

    /// Evaluate the fs-snapshot policy for an operation, if a chain is set.
    /// Input: `{ "op": ..., "label": ..., "ca_id": ... }`.
    async fn check_fs_snapshot_policy(
        &self,
        op: &str,
        label: Option<&str>,
        ca_id: Option<&str>,
    ) -> Result<(), String> {
        let Some(chain) = &self.fs_snapshot_policy_chain else {
            return Ok(());
        };
        let input = serde_json::json!({ "op": op, "label": label, "ca_id": ca_id });
        let allowed = chain
            .evaluate(&input)
            .await
            .map_err(|e| format!("fs_snapshot policy error: {e}"))?;
        if !allowed {
            return Err(format!(
                "fs_snapshot {op} denied by policy (label={label:?}, ca_id={ca_id:?})"
            ));
        }
        Ok(())
    }

    /// The fs object store, if configured.
    pub fn fs_store(&self) -> Option<&Arc<fs_store::FsStore>> {
        self.fs_store.as_ref()
    }

    /// The fs label/reflog store, if configured.
    pub fn label_store(&self) -> Option<&Arc<fs_labels::LabelStore>> {
        self.label_store.as_ref()
    }

    /// Resolve a `run_js` `fs` handle to an attached overlay mount. The handle
    /// is a label name (mounted at its current head) or a 64-hex CA id (mounted
    /// detached/pinned). Returns `None` when no handle was supplied.
    async fn build_fs_mount(
        &self,
        fs: &Option<String>,
    ) -> Result<Option<fs::FsMountHandle>, String> {
        let Some(handle) = fs.as_ref().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        self.check_fs_snapshot_policy("pull", Some(handle), None).await?;
        let store = self
            .fs_store
            .as_ref()
            .ok_or_else(|| "fs snapshots are not configured on this server".to_string())?;

        // A 64-char hex string is treated as a detached CA id; anything else is
        // a label resolved to its current head.
        let mount = if let Some(id) = parse_ca_hex(handle) {
            let hash = blake3::Hash::from_bytes(id);
            fs_mount::SessionMount::pull((**store).clone(), hash)
                .await
                .map_err(|e| format!("fs mount: pull {handle}: {e}"))?
        } else {
            let labels = self
                .label_store
                .as_ref()
                .ok_or_else(|| "fs labels are not configured on this server".to_string())?;
            match labels.resolve(handle).await? {
                Some(id) => {
                    let hash = blake3::Hash::from_bytes(id);
                    fs_mount::SessionMount::pull((**store).clone(), hash)
                        .await
                        .map_err(|e| format!("fs mount: pull label {handle}: {e}"))?
                }
                // Unknown label → start from an empty overlay so the first push
                // can create it.
                None => fs_mount::SessionMount::empty((**store).clone()),
            }
        };

        // Pre-stage the mounted tree's blobs into the node-local cache now, on
        // this (main) runtime. The isolate runs on its own current-thread
        // runtime and its fs ops cannot await the blob backend's remote I/O, so
        // a lazy in-op fetch from S3 would deadlock; warming here makes those
        // reads pure local-cache hits.
        mount
            .warm()
            .await
            .map_err(|e| format!("fs mount: warm {handle}: {e}"))?;

        Ok(Some(fs::FsMountHandle::new(mount)))
    }

    /// Resolve a `run_js` `file` parameter to source code, applying the
    /// configured policy. Errors if file-path execution is disabled or denied.
    async fn resolve_run_js_file(
        &self,
        path: &str,
        mcp_headers: Option<&serde_json::Value>,
    ) -> Result<String, String> {
        match &self.run_js_file_policy {
            None => Err(
                "run_js file-path execution is disabled. Enable it with \
                 --allow-run-js-file or configure a `run_js_file` policy in \
                 --policies-json."
                    .to_string(),
            ),
            Some(policy) => policy.read(path, mcp_headers).await,
        }
    }

    /// Create a builder for submitting JavaScript code for execution.
    pub fn run_js(&self, code: impl Into<String>) -> RunJsRequest<'_> {
        RunJsRequest {
            engine: self,
            code: code.into(),
            file: None,
            heap: None,
            fs: None,
            session: None,
            heap_memory_max_mb: None,
            execution_timeout_secs: None,
            tags: None,
            mcp_headers: None,
        }
    }

    /// Internal: actually submit the run_js request.
    #[allow(clippy::too_many_arguments)]
    async fn run_js_inner(
        &self,
        code: String,
        file: Option<String>,
        heap: Option<String>,
        fs: Option<String>,
        session: Option<String>,
        heap_memory_max_mb: Option<usize>,
        execution_timeout_secs: Option<u64>,
        tags: Option<HashMap<String, String>>,
        mcp_headers: Option<serde_json::Value>,
    ) -> Result<ExecutionId, String> {
        let registry = self.execution_registry.as_ref()
            .ok_or_else(|| "Execution registry not configured".to_string())?;

        // Resolve a file-path source (policy-gated) when provided; otherwise
        // use the inline code. Supplying both is an error to avoid ambiguity.
        let code = match file {
            Some(path) => {
                if !code.trim().is_empty() {
                    return Err(
                        "run_js: provide either `code` or `file`, not both".to_string()
                    );
                }
                self.resolve_run_js_file(&path, mcp_headers.as_ref()).await?
            }
            None => code,
        };

        // Strip TypeScript types before V8 execution (no-op for plain JS)
        let code = strip_typescript_types(&code)?;

        let id = uuid::Uuid::new_v4().to_string();
        let console_tree = registry.register(&id)?;

        // Resolve which heap snapshot to restore. An explicit `heap` always
        // wins. Otherwise, when a `session` is given, fall back to that
        // session's most-recent output heap so `session` acts as a stable,
        // unchanging label for accumulated state (callers can persist just the
        // session name and never have to track the content-addressed heap key).
        let heap = match &heap {
            Some(h) if !h.is_empty() => heap,
            _ => match (session.as_ref(), self.session_log.as_ref()) {
                (Some(session_name), Some(log)) => match log.get_latest(session_name).await {
                    Ok(Some(entry)) => Some(entry.output_heap),
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!(
                            "Failed to resolve latest heap for session '{}': {}",
                            session_name,
                            e
                        );
                        None
                    }
                },
                _ => None,
            },
        };

        // Resolve which fs snapshot to mount, mirroring the heap logic above so
        // the content-addressed filesystem persists per `session` exactly like
        // the heap. An explicit `fs` (label or CA id) always wins. Otherwise,
        // when fs snapshots are configured and a `session` is given, mount that
        // session's most-recent output fs; on the first run there is none yet,
        // so fall back to the session name as the handle — `build_fs_mount`
        // treats an unknown label as an empty overlay, which is exactly the
        // desired starting state. The post-run output fs is recorded in the
        // session log, so the next run picks it up with no label management.
        let fs = match &fs {
            Some(f) if !f.is_empty() => fs,
            _ if self.fs_store.is_some() => {
                match (session.as_ref(), self.session_log.as_ref()) {
                    (Some(session_name), Some(log)) => {
                        match log.get_latest(session_name).await {
                            Ok(Some(entry)) if entry.output_fs.is_some() => entry.output_fs,
                            Ok(_) => Some(session_name.clone()),
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to resolve latest fs for session '{}': {}",
                                    session_name,
                                    e
                                );
                                Some(session_name.clone())
                            }
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        };

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
                id_bg, code, heap, fs, session, heap_memory_max_mb,
                execution_timeout_secs, tags, raw_snapshot, console_tree,
                mcp_headers,
            ).await;
        });

        Ok(id)
    }

    // ── fs snapshot label operations ─────────────────────────────────────
    // Thin orchestration over the LabelStore; the MCP tools, HTTP API, and CLI
    // all route through these so behavior stays identical across surfaces.

    fn labels_or_err(&self) -> Result<&Arc<fs_labels::LabelStore>, String> {
        self.label_store
            .as_ref()
            .ok_or_else(|| "fs labels are not configured on this server".to_string())
    }

    fn fs_store_or_err(&self) -> Result<&Arc<fs_store::FsStore>, String> {
        self.fs_store
            .as_ref()
            .ok_or_else(|| "fs snapshots are not configured on this server".to_string())
    }

    /// Three-way merge two snapshots into a new one. `base` (the common
    /// ancestor the two sides diverged from — typically the label head both
    /// were mounted from) is optional: with it, only paths both sides changed
    /// conflict; without it, the merge is 2-way. `prefer` auto-resolves
    /// conflicts to one side. A clean merge yields the new snapshot's CA id;
    /// otherwise the conflicting paths are reported. The merge produces a normal
    /// pure manifest and does NOT move any label (push it explicitly).
    pub async fn fs_merge(
        &self,
        ours: &str,
        theirs: &str,
        base: Option<String>,
        prefer: fs_merge::Prefer,
    ) -> Result<FsMergeResult, String> {
        self.check_fs_snapshot_policy("merge", None, None).await?;
        let store = self.fs_store_or_err()?;

        let load = |hex: &str| -> Result<[u8; 32], String> {
            parse_ca_hex(hex).ok_or_else(|| format!("invalid CA id: {hex}"))
        };
        let base_root = match &base {
            Some(b) => Some(load(b)?),
            None => None,
        };

        // Structural per-path 3-way merge over the trees: equal subtrees are
        // pruned by hash (never loaded), clean parts land in the merged tree,
        // divergent paths come back as conflicts.
        let structural =
            fs_merge::merge_trees(store, base_root, Some(load(ours)?), Some(load(theirs)?), prefer)
                .await
                .map_err(|e| format!("fs_merge: {e}"))?;
        let merged_root = structural.root;

        // Content-merge pass: give a type-aware merger a shot at each conflict
        // before reporting it. Clean text merges resolve silently and are patched
        // back into the merged tree; the rest are surfaced with diffs/markers.
        let mergers = fs_content_merge::default_mergers();
        let mut conflict_views = Vec::new();
        let mut resolved: Vec<(Vec<String>, Option<fs_store::Entry>)> = Vec::new();
        for c in structural.conflicts {
            let view = match (&c.ours, &c.theirs) {
                (Some(oe), Some(te)) => {
                    let ours_b = store
                        .read_file(oe)
                        .await
                        .map_err(|e| format!("fs_merge: read ours {}: {e}", c.path.display()))?;
                    let theirs_b = store
                        .read_file(te)
                        .await
                        .map_err(|e| format!("fs_merge: read theirs {}: {e}", c.path.display()))?;
                    let base_b = match &c.base {
                        Some(be) => Some(store.read_file(be).await.map_err(|e| {
                            format!("fs_merge: read base {}: {e}", c.path.display())
                        })?),
                        None => None,
                    };
                    match fs_content_merge::merge_content(
                        &mergers,
                        base_b.as_deref(),
                        &ours_b,
                        &theirs_b,
                    ) {
                        fs_content_merge::ContentMergeResult::Clean(bytes) => {
                            let entry = store
                                .put_file(&bytes)
                                .await
                                .map_err(|e| format!("fs_merge: store merged {}: {e}", c.path.display()))?;
                            resolved.push((fs_tree::components_of(&c.path), Some(entry)));
                            continue; // resolved — not a conflict
                        }
                        fs_content_merge::ContentMergeResult::Conflict(cc) => FsMergeConflictView {
                            path: c.path.to_string_lossy().to_string(),
                            base: c.base.as_ref().map(entry_content_id),
                            ours: c.ours.as_ref().map(entry_content_id),
                            theirs: c.theirs.as_ref().map(entry_content_id),
                            kind: cc.kind.as_str().to_string(),
                            markers: cc.markers,
                            diff_ours: cc.diff_ours,
                            diff_theirs: cc.diff_theirs,
                        },
                    }
                }
                // A modify/delete (or add on one side): no content to reconcile.
                _ => FsMergeConflictView {
                    path: c.path.to_string_lossy().to_string(),
                    base: c.base.as_ref().map(entry_content_id),
                    ours: c.ours.as_ref().map(entry_content_id),
                    theirs: c.theirs.as_ref().map(entry_content_id),
                    kind: "modify/delete".to_string(),
                    markers: None,
                    diff_ours: None,
                    diff_theirs: None,
                },
            };
            conflict_views.push(view);
        }

        if conflict_views.is_empty() {
            // Patch the content-merge resolutions onto the structurally-merged
            // tree (writing only the touched spine).
            let final_root = if resolved.is_empty() {
                merged_root
            } else {
                store
                    .build_root(Some(merged_root), resolved)
                    .await
                    .map_err(|e| format!("fs_merge: store result: {e}"))?
            };
            Ok(FsMergeResult::Merged {
                ca_id: ca_to_hex(&final_root),
            })
        } else {
            Ok(FsMergeResult::Conflict {
                conflicts: conflict_views,
            })
        }
    }

    /// List every label and its current head CA id (hex).
    pub async fn fs_list_labels(&self) -> Result<Vec<FsLabelView>, String> {
        let labels = self.labels_or_err()?;
        Ok(labels
            .list()
            .await?
            .into_iter()
            .map(|(name, id)| FsLabelView {
                name,
                ca_id: ca_to_hex(&id),
            })
            .collect())
    }

    /// Resolve a label to its current head CA id (hex), if it exists.
    pub async fn fs_resolve_label(&self, name: &str) -> Result<Option<String>, String> {
        let labels = self.labels_or_err()?;
        Ok(labels.resolve(name).await?.map(|id| ca_to_hex(&id)))
    }

    /// Create a label, or repoint an existing one, to a CA id. `message` is an
    /// optional human note recorded on the reflog entry.
    pub async fn fs_set_label(
        &self,
        name: &str,
        ca_hex: &str,
        message: Option<String>,
    ) -> Result<(), String> {
        self.check_fs_snapshot_policy("label", Some(name), Some(ca_hex)).await?;
        let labels = self.labels_or_err()?;
        let id = parse_ca_hex(ca_hex).ok_or_else(|| format!("invalid CA id: {ca_hex}"))?;
        match labels.resolve(name).await? {
            Some(_) => labels.force(name, id, message).await,
            None => labels.create(name, id, message).await,
        }
    }

    /// The reflog for a label (hex-rendered), oldest first. When `limit` is
    /// given, only the most recent `limit` entries are read and returned —
    /// bounding the scan over very long histories.
    pub async fn fs_label_log(
        &self,
        name: &str,
        limit: Option<usize>,
    ) -> Result<Vec<FsRefLogView>, String> {
        let labels = self.labels_or_err()?;
        let entries = match limit {
            Some(n) => labels.log_recent(name, n).await?,
            None => labels.log(name).await?,
        };
        Ok(entries
            .into_iter()
            .map(|e| FsRefLogView {
                at: e.at,
                from: e.from.as_ref().map(ca_to_hex),
                to: ca_to_hex(&e.to),
                op: refop_str(e.op).to_string(),
                message: e.message,
            })
            .collect())
    }

    /// Advance a label to a CA id. Default is reject-and-rebase: the move only
    /// succeeds if the label's current head equals `expected` (or the label does
    /// not yet exist and `expected` is `None`). `force` skips the check.
    pub async fn fs_push(
        &self,
        label: &str,
        ca_hex: &str,
        expected: Option<String>,
        force: bool,
        message: Option<String>,
    ) -> Result<FsPushOutcome, String> {
        self.check_fs_snapshot_policy("push", Some(label), Some(ca_hex)).await?;
        let labels = self.labels_or_err()?;
        let new = parse_ca_hex(ca_hex).ok_or_else(|| format!("invalid CA id: {ca_hex}"))?;

        if force {
            labels.force(label, new, message).await?;
            return Ok(FsPushOutcome::Advanced {
                label: label.to_string(),
                ca_id: ca_hex.to_string(),
            });
        }

        let expected = match expected {
            Some(h) => Some(parse_ca_hex(&h).ok_or_else(|| format!("invalid expected CA id: {h}"))?),
            None => None,
        };
        let current = labels.resolve(label).await?;
        let advanced = if current.is_none() && expected.is_none() {
            labels.create(label, new, message).await?;
            true
        } else {
            labels.cas(label, expected, new, message).await?
        };

        if advanced {
            Ok(FsPushOutcome::Advanced {
                label: label.to_string(),
                ca_id: ca_hex.to_string(),
            })
        } else {
            Ok(FsPushOutcome::Rejected {
                label: label.to_string(),
                current: current.as_ref().map(ca_to_hex),
            })
        }
    }

    /// Reset a label to an earlier CA id from its reflog (the rollback verb).
    /// Unless `allow_unlogged` is set, the target must appear in the label's
    /// reflog so resets stay within recorded history.
    pub async fn fs_reset(
        &self,
        label: &str,
        ca_hex: &str,
        allow_unlogged: bool,
        message: Option<String>,
    ) -> Result<(), String> {
        self.check_fs_snapshot_policy("reset", Some(label), Some(ca_hex)).await?;
        let labels = self.labels_or_err()?;
        let target = parse_ca_hex(ca_hex).ok_or_else(|| format!("invalid CA id: {ca_hex}"))?;
        if !allow_unlogged {
            let in_log = labels
                .log(label)
                .await?
                .iter()
                .any(|e| e.to == target || e.from == Some(target));
            if !in_log {
                return Err(format!(
                    "CA id {ca_hex} is not in the reflog for label '{label}'; \
                     pass allow_unlogged to reset anyway"
                ));
            }
        }
        labels.force(label, target, message).await
    }

    /// Flush a session's overlay mount into a new pure manifest and return its
    /// CA id (hex). This is the durable fs artifact recorded on completion; it
    /// does NOT advance any label (pushing a label is the explicit `fs_push`
    /// verb).
    ///
    /// `Ok(None)` means no mount was attached; `Ok(Some(ca))` is a flushed
    /// snapshot. A flush failure on an attached mount is returned as `Err` so
    /// the caller can fail the execution rather than silently reporting it
    /// complete with the filesystem changes lost.
    async fn push_mount(&self, fm: &Option<fs::FsMountHandle>) -> Result<Option<String>, String> {
        let Some(fm) = fm.as_ref() else {
            return Ok(None);
        };
        match fm.0.lock().await.push().await {
            Ok(h) => Ok(Some(ca_to_hex(h.as_bytes()))),
            Err(e) => Err(format!("fs snapshot flush failed: {e}")),
        }
    }

    /// Background execution task — runs V8 on the blocking pool with timeout.
    #[allow(clippy::too_many_arguments)]
    async fn execute_in_background(
        &self,
        id: ExecutionId,
        code: String,
        heap: Option<String>,
        fs: Option<String>,
        session: Option<String>,
        heap_memory_max_mb: Option<usize>,
        execution_timeout_secs: Option<u64>,
        tags: Option<HashMap<String, String>>,
        raw_snapshot: Option<Vec<u8>>,
        console_tree: sled::Tree,
        mcp_headers: Option<serde_json::Value>,
    ) {
        let registry = match &self.execution_registry {
            Some(r) => r.clone(),
            None => return,
        };

        // Resolve and attach the fs overlay mount (independent of the heap).
        let fs_mount = match self.build_fs_mount(&fs).await {
            Ok(m) => m,
            Err(e) => {
                registry.fail(&id, e);
                return;
            }
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
                let hardening = self.hardening;
                let fc = self.fetch_config.clone();
                let fsc = self.fs_config.clone();
                let mh = mcp_headers.clone();
                let sc = self.subprocess_config.clone();
                let ct = console_tree;
                let mlc = self.module_loader_config.clone();
                let mc = self.mcp_client_manager.as_ref().map(|m| mcp_client::McpConfig { client_manager: (**m).clone(), policy_chain: self.mcp_tools_policy_chain.clone() });
                let fm = fs_mount.clone();
                // Cloned for the post-run session-log entry, since `code` is
                // moved into the spawn_blocking closure below.
                let code_for_log = code.clone();
                let mut join_handle = tokio::task::spawn_blocking(move || {
                    execute_stateless(&code, ExecutionConfig::new(max_bytes)
                        .isolate_handle(ih)
                        .maybe_fs_mount(fm)
                        .wasm_modules(&wasm)
                        .wasm_default_max_bytes(wasm_default)
                        .hardening(hardening)
                        .maybe_fetch_config(fc.as_deref())
                        .maybe_fs_config(fsc.as_deref())
                        .mcp_headers(mh)
                        .maybe_subprocess_config(sc.as_deref())
                        .console_tree(ct)
                        .module_loader_config(&mlc)
                        .maybe_mcp_config(mc.as_ref()))
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
                    Ok(js_result) => {
                        // Flush and publish the fs snapshot id *before* marking
                        // the run complete, so a client that stops polling on the
                        // first terminal status cannot miss it. A flush failure
                        // fails the run rather than reporting lost fs changes.
                        match self.push_mount(&fs_mount).await {
                            Ok(output_fs) => {
                                registry.set_fs(&id, output_fs.clone());
                                // Record the resulting fs snapshot per session so a
                                // later run in the same session resumes it. This is
                                // what gives fs-only (heap-off) engines per-session
                                // filesystem persistence; no heap fields are set.
                                if let (Some(session_name), Some(log)) =
                                    (&session, &self.session_log)
                                {
                                    let entry = SessionLogEntry {
                                        input_heap: None,
                                        output_heap: String::new(),
                                        output_fs: output_fs.clone(),
                                        code: code_for_log,
                                        timestamp: chrono::Utc::now().to_rfc3339(),
                                    };
                                    if let Err(e) = log.append(session_name, entry).await {
                                        tracing::warn!("Failed to log session entry: {}", e);
                                    }
                                }
                                registry.complete(&id, js_result.output, None);
                            }
                            Err(e) => registry.fail(&id, e),
                        }
                    }
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
                let hardening = self.hardening;
                let fc = self.fetch_config.clone();
                let fsc = self.fs_config.clone();
                let mh = mcp_headers.clone();
                let sc = self.subprocess_config.clone();
                let ct = console_tree;
                let mlc = self.module_loader_config.clone();
                let mc = self.mcp_client_manager.as_ref().map(|m| mcp_client::McpConfig { client_manager: (**m).clone(), policy_chain: self.mcp_tools_policy_chain.clone() });

                let snap_mutex = self.snapshot_mutex.clone();
                let fm = fs_mount.clone();
                let mut join_handle = tokio::task::spawn_blocking(move || {
                    let _guard = snap_mutex.blocking_lock();
                    execute_stateful(&code, raw_snapshot, ExecutionConfig::new(max_bytes)
                        .isolate_handle(ih)
                        .maybe_fs_mount(fm)
                        .wasm_modules(&wasm)
                        .wasm_default_max_bytes(wasm_default)
                        .hardening(hardening)
                        .maybe_fetch_config(fc.as_deref())
                        .maybe_fs_config(fsc.as_deref())
                        .mcp_headers(mh)
                        .maybe_subprocess_config(sc.as_deref())
                        .console_tree(ct)
                        .module_loader_config(&mlc)
                        .maybe_mcp_config(mc.as_ref()))
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

                        // Record the resulting fs snapshot CA id independently of
                        // the heap. Does not advance any label. A flush failure
                        // on an attached mount fails the run rather than reporting
                        // it complete with the filesystem changes lost.
                        let output_fs = match self.push_mount(&fs_mount).await {
                            Ok(v) => v,
                            Err(e) => {
                                registry.fail(&id, e);
                                return;
                            }
                        };
                        registry.set_fs(&id, output_fs.clone());

                        if let (Some(session_name), Some(log)) = (&session, &self.session_log) {
                            let entry = SessionLogEntry {
                                input_heap: heap.clone(),
                                output_heap: content_hash.clone(),
                                output_fs: output_fs.clone(),
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

    /// Stop background work owned by the engine.
    pub async fn shutdown(&self) -> (u64, u64) {
        let cancelled_executions = self.execution_registry.as_ref()
            .map(|registry| registry.cancel_all())
            .unwrap_or(0);
        let closed_mcp_connections = match &self.mcp_client_manager {
            Some(manager) => manager.shutdown().await,
            None => 0,
        };
        (cancelled_executions, closed_mcp_connections)
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

// ═══════════════════════════════════════════════════════════════════════════
// Canonical runtime surface — the single uniffi-exported object shared by
// embedded callers (FFI bindings) and every Rust server transport.
// ═══════════════════════════════════════════════════════════════════════════

const DEFAULT_HEAP_MEMORY_MB: u64 = 64;
pub const DEFAULT_WASM_STUB_PREFIX: &str = crate::engine::wasm_stub::DEFAULT_WASM_STUB_PREFIX;
pub const DEFAULT_MCP_STUB_PREFIX: &str = crate::engine::mcp_client::DEFAULT_STUB_PREFIX;
const DEFAULT_MAX_CONCURRENT_EXECUTIONS: u32 = 4;

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeMode {
    Stateless,
    LocalStateful,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeLifecycleState {
    Running,
    ShuttingDown,
    Shutdown,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeShutdownResult {
    pub cancelled_executions: u64,
    pub closed_mcp_connections: u64,
    pub cluster_shutdown: bool,
    pub already_shutdown: bool,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct RuntimeHardeningConfig {
    pub freeze_ops: bool,
    pub neutralize_proxy_details: bool,
    pub neutralize_introspection: bool,
    pub remove_bootstrap: bool,
    pub remove_shared_memory: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeWasmModuleConfig {
    pub name: String,
    pub bytes: Vec<u8>,
    pub max_memory_bytes: Option<u64>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeWasmStubConfig {
    pub prefix: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeFeatureConfig {
    pub wasm_default_max_bytes: u64,
    pub hardening: RuntimeHardeningConfig,
    pub wasm_modules: Vec<RuntimeWasmModuleConfig>,
    pub wasm_stubs: RuntimeWasmStubConfig,
    pub instructions_override: Option<String>,
    pub run_js_description_override: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum RuntimePolicyEvalMode {
    #[default]
    All,
    Any,
}

#[derive(Clone, Debug, Deserialize, uniffi::Record)]
pub struct RuntimePolicySource {
    pub url: String,
    pub policy_path: Option<String>,
    pub rule: Option<String>,
}

#[derive(Clone, Debug, Deserialize, uniffi::Record)]
pub struct RuntimeOperationPolicies {
    #[serde(default)]
    pub mode: RuntimePolicyEvalMode,
    pub policies: Vec<RuntimePolicySource>,
}

#[derive(Clone, Debug, Default, Deserialize, uniffi::Record)]
pub struct RuntimePolicyConfig {
    pub fetch: Option<RuntimeOperationPolicies>,
    pub modules: Option<RuntimeOperationPolicies>,
    pub filesystem: Option<RuntimeOperationPolicies>,
    pub fs_snapshot: Option<RuntimeOperationPolicies>,
    pub mcp_tools: Option<RuntimeOperationPolicies>,
    pub subprocess: Option<RuntimeOperationPolicies>,
    pub run_js_file: Option<RuntimeOperationPolicies>,
}

#[derive(Clone, uniffi::Record)]
pub struct RuntimeFetchOAuthConfig {
    pub header_name: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: Option<String>,
    pub refresh_buffer_secs: u64,
}

impl std::fmt::Debug for RuntimeFetchOAuthConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeFetchOAuthConfig")
            .field("header_name", &self.header_name)
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scope", &self.scope)
            .field("refresh_buffer_secs", &self.refresh_buffer_secs)
            .finish()
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeFetchHeaderRule {
    pub host: String,
    pub methods: Vec<String>,
    pub static_headers: Option<HashMap<String, String>>,
    pub oauth: Option<RuntimeFetchOAuthConfig>,
}

impl RuntimeFetchHeaderRule {
    pub fn validate(&self) -> Result<(), RuntimeError> {
        crate::bootstrap::validate_fetch_header_rule(self)
    }

    pub fn normalized(self) -> Result<Self, RuntimeError> {
        crate::bootstrap::normalize_fetch_header_rule(self)
    }

    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    pub fn static_headers(&self) -> Option<&HashMap<String, String>> {
        self.static_headers.as_ref()
    }

    pub fn dynamic_auth(&self) -> Option<&RuntimeFetchOAuthConfig> {
        self.oauth.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Default, uniffi::Enum)]
pub enum RuntimeRunJsFileAccess {
    #[default]
    Disabled,
    AllowAll,
    Policy,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct RuntimeCapabilityConfig {
    pub fetch_header_rules: Vec<RuntimeFetchHeaderRule>,
    pub filesystem_passthrough: bool,
    pub allow_external_modules: bool,
    pub run_js_file_access: RuntimeRunJsFileAccess,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeMcpTransportKind {
    Stdio,
    Sse,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeMcpServerConfig {
    pub name: String,
    pub transport: RuntimeMcpTransportKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: Option<String>,
}

impl<'de> Deserialize<'de> for RuntimeMcpServerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "transport", rename_all = "lowercase")]
        enum Transport {
            Stdio {
                command: String,
                #[serde(default)]
                args: Vec<String>,
                #[serde(default)]
                env: HashMap<String, String>,
            },
            Sse {
                url: String,
            },
        }

        #[derive(Deserialize)]
        struct Config {
            name: String,
            #[serde(flatten)]
            transport: Transport,
        }

        let config = Config::deserialize(deserializer)?;
        Ok(match config.transport {
            Transport::Stdio { command, args, env } => Self {
                name: config.name,
                transport: RuntimeMcpTransportKind::Stdio,
                command: Some(command),
                args,
                env,
                url: None,
            },
            Transport::Sse { url } => Self {
                name: config.name,
                transport: RuntimeMcpTransportKind::Sse,
                command: None,
                args: Vec::new(),
                env: HashMap::new(),
                url: Some(url),
            },
        })
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeMcpStubConfig {
    pub prefix: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeUpstreamMcpConfig {
    pub servers: Vec<RuntimeMcpServerConfig>,
    pub stubs: RuntimeMcpStubConfig,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeStorageKind {
    None,
    Directory,
    S3,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeConfig {
    pub heap_store: RuntimeStorageKind,
    pub heap_dir: Option<String>,
    pub filesystem_store: RuntimeStorageKind,
    pub filesystem_dir: Option<String>,
    pub filesystem_labels_db: Option<String>,
    pub s3_bucket: Option<String>,
    pub cache_dir: Option<String>,
    pub session_db_path: String,
    pub execution_db_path: Option<String>,
    pub heap_memory_max_mb: u64,
    pub execution_timeout_secs: u64,
    pub max_concurrent_executions: u32,
    pub session_id: Option<String>,
    pub session_fork_from: Option<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeOptions {
    pub mode: RuntimeMode,
    pub data_dir: Option<String>,
    pub heap_memory_max_mb: u64,
    pub execution_timeout_secs: u64,
    pub max_concurrent_executions: u32,
    pub filesystem_enabled: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Stateless,
            data_dir: None,
            heap_memory_max_mb: DEFAULT_HEAP_MEMORY_MB,
            execution_timeout_secs: DEFAULT_EXECUTION_TIMEOUT_SECS,
            max_concurrent_executions: DEFAULT_MAX_CONCURRENT_EXECUTIONS,
            filesystem_enabled: false,
        }
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema_json: String,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct McpRequestHeaders {
    pub values: HashMap<String, String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ToolCallRequest {
    pub name: String,
    pub arguments_json: String,
    pub session_id: Option<String>,
    pub mcp_headers: Option<McpRequestHeaders>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct RuntimeCapabilities {
    pub heap: bool,
    pub filesystem: bool,
    pub sessions: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ExecutionRequest {
    pub code: String,
    pub file: Option<String>,
    pub heap: Option<String>,
    pub fs: Option<String>,
    pub session: Option<String>,
    pub heap_memory_max_mb: Option<u64>,
    pub execution_timeout_secs: Option<u64>,
    pub tags: Option<HashMap<String, String>>,
    pub mcp_headers: Option<McpRequestHeaders>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum RuntimeError {
    #[error("invalid configuration: {message}")]
    InvalidConfig { message: String },
    #[error("failed to initialize the embedded runtime: {message}")]
    Initialization { message: String },
    #[error("invalid JSON for {field}: {message}")]
    InvalidJson { field: String, message: String },
    #[error("tool call failed: {message}")]
    ToolCall { message: String },
    #[error("operation failed: {message}")]
    Operation { message: String },
}

#[derive(uniffi::Object)]
pub struct McpJsRuntime {
    tokio_runtime: Option<tokio::runtime::Runtime>,
    engine: Engine,
    cluster_node: Option<Arc<ClusterNode>>,
    lifecycle: AtomicU8,
    shutdown_lock: tokio::sync::Mutex<()>,
    _ephemeral_data_dir: Option<tempfile::TempDir>,
}

#[uniffi::export]
pub fn default_runtime_options() -> RuntimeOptions {
    RuntimeOptions::default()
}

#[uniffi::export]
pub fn default_feature_config() -> RuntimeFeatureConfig {
    RuntimeFeatureConfig {
        wasm_default_max_bytes: crate::engine::DEFAULT_WASM_MAX_BYTES as u64,
        hardening: RuntimeHardeningConfig::default(),
        wasm_modules: Vec::new(),
        wasm_stubs: RuntimeWasmStubConfig {
            prefix: DEFAULT_WASM_STUB_PREFIX.to_string(),
            enabled: true,
        },
        instructions_override: None,
        run_js_description_override: None,
    }
}

#[uniffi::export]
pub fn default_policy_config() -> RuntimePolicyConfig {
    RuntimePolicyConfig::default()
}

#[uniffi::export]
pub fn default_fetch_oauth_refresh_buffer_secs() -> u64 {
    crate::engine::fetch::default_refresh_buffer_secs()
}

#[uniffi::export]
pub fn default_capability_config() -> RuntimeCapabilityConfig {
    RuntimeCapabilityConfig::default()
}

#[uniffi::export]
pub fn default_upstream_mcp_config() -> RuntimeUpstreamMcpConfig {
    RuntimeUpstreamMcpConfig {
        servers: Vec::new(),
        stubs: RuntimeMcpStubConfig {
            prefix: DEFAULT_MCP_STUB_PREFIX.to_string(),
            enabled: true,
        },
    }
}

#[uniffi::export]
pub fn default_runtime_config(data_dir: String) -> RuntimeConfig {
    RuntimeConfig {
        heap_store: RuntimeStorageKind::None,
        heap_dir: None,
        filesystem_store: RuntimeStorageKind::None,
        filesystem_dir: None,
        filesystem_labels_db: None,
        s3_bucket: None,
        cache_dir: None,
        session_db_path: data_dir,
        execution_db_path: None,
        heap_memory_max_mb: DEFAULT_HEAP_MEMORY_MB,
        execution_timeout_secs: DEFAULT_EXECUTION_TIMEOUT_SECS,
        max_concurrent_executions: DEFAULT_MAX_CONCURRENT_EXECUTIONS,
        session_id: None,
        session_fork_from: None,
    }
}

#[uniffi::export]
pub fn create_runtime(config: RuntimeConfig) -> Result<Arc<McpJsRuntime>, RuntimeError> {
    create_runtime_with_features(config, default_feature_config())
}

#[uniffi::export]
pub fn create_runtime_with_features(
    config: RuntimeConfig,
    features: RuntimeFeatureConfig,
) -> Result<Arc<McpJsRuntime>, RuntimeError> {
    create_runtime_with_configuration(
        config,
        features,
        default_policy_config(),
        default_capability_config(),
    )
}

#[uniffi::export]
pub fn create_runtime_with_configuration(
    config: RuntimeConfig,
    features: RuntimeFeatureConfig,
    policies: RuntimePolicyConfig,
    capabilities: RuntimeCapabilityConfig,
) -> Result<Arc<McpJsRuntime>, RuntimeError> {
    create_runtime_with_upstreams(
        config,
        features,
        policies,
        capabilities,
        default_upstream_mcp_config(),
    )
}

#[uniffi::export]
pub fn create_runtime_with_upstreams(
    config: RuntimeConfig,
    features: RuntimeFeatureConfig,
    policies: RuntimePolicyConfig,
    capabilities: RuntimeCapabilityConfig,
    upstreams: RuntimeUpstreamMcpConfig,
) -> Result<Arc<McpJsRuntime>, RuntimeError> {
    validate_runtime_config(&config)?;
    initialize_v8();
    let worker_threads = usize::try_from(config.max_concurrent_executions).map_err(|_| {
        RuntimeError::InvalidConfig {
            message: "max_concurrent_executions is too large for this platform".to_string(),
        }
    })?;
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .build()
        .map_err(|error| RuntimeError::Initialization {
            message: error.to_string(),
        })?;
    let bootstrap_config = runtime_bootstrap_config(config)?;
    let bootstrap = tokio_runtime.block_on(async {
        let bootstrap = crate::bootstrap::build_storage_engine(bootstrap_config, None)
            .await
            .map_err(|error| RuntimeError::Initialization {
                message: error.to_string(),
            })?
            .with_feature_config(features)?
            .with_policy_config(policies, capabilities)?;
        bootstrap.with_upstream_mcp_config(upstreams).await
    })?;
    Ok(bootstrap.build_with_runtime(tokio_runtime))
}

#[uniffi::export]
impl McpJsRuntime {
    #[uniffi::constructor]
    pub fn new(config: RuntimeOptions) -> Result<Arc<Self>, RuntimeError> {
        validate_options(&config)?;
        initialize_v8();

        let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(config.max_concurrent_executions as usize)
            .build()
            .map_err(|error| RuntimeError::Initialization {
                message: error.to_string(),
            })?;

        let (engine, ephemeral_data_dir) = build_engine_from_options(&config)?;
        Ok(Self::wrap(
            engine,
            Some(tokio_runtime),
            ephemeral_data_dir,
            None,
        ))
    }

    pub fn mode(&self) -> RuntimeMode {
        if self.engine.session_capable() {
            RuntimeMode::LocalStateful
        } else {
            RuntimeMode::Stateless
        }
    }

    pub fn lifecycle_state(&self) -> RuntimeLifecycleState {
        self.current_lifecycle_state()
    }

    pub async fn shutdown(&self) -> RuntimeShutdownResult {
        let _guard = self.shutdown_lock.lock().await;
        if self.current_lifecycle_state() == RuntimeLifecycleState::Shutdown {
            return RuntimeShutdownResult {
                cancelled_executions: 0,
                closed_mcp_connections: 0,
                cluster_shutdown: false,
                already_shutdown: true,
            };
        }

        self.lifecycle
            .store(RuntimeLifecycleState::ShuttingDown as u8, Ordering::Release);
        let (cancelled_executions, closed_mcp_connections) = self.engine.shutdown().await;
        let cluster_shutdown = self.cluster_node.as_ref().is_some_and(|node| {
            node.shutdown();
            true
        });
        self.lifecycle
            .store(RuntimeLifecycleState::Shutdown as u8, Ordering::Release);

        RuntimeShutdownResult {
            cancelled_executions,
            closed_mcp_connections,
            cluster_shutdown,
            already_shutdown: false,
        }
    }

    pub fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            heap: self.engine.heap_enabled(),
            filesystem: self.engine.fs_enabled(),
            sessions: self.engine.session_capable(),
        }
    }

    pub async fn submit_execution(
        &self,
        request: ExecutionRequest,
    ) -> Result<String, RuntimeError> {
        let _lifecycle_guard = self.shutdown_lock.lock().await;
        self.ensure_running()?;
        if request.code.is_empty() && request.file.is_none() {
            return Err(RuntimeError::InvalidConfig {
                message: "execution requires code or a file path".to_string(),
            });
        }
        if !request.code.is_empty() && request.file.is_some() {
            return Err(RuntimeError::InvalidConfig {
                message: "execution cannot specify both code and a file path".to_string(),
            });
        }
        let heap_memory_max_mb = request
            .heap_memory_max_mb
            .map(usize::try_from)
            .transpose()
            .map_err(|_| RuntimeError::InvalidConfig {
                message: "heap_memory_max_mb is too large for this platform".to_string(),
            })?;
        let mcp_headers = request.mcp_headers.map(mcp_headers_value);

        let mut execution = self.engine.run_js(request.code)
            .maybe_file(request.file)
            .maybe_fs(request.fs)
            .maybe_session(request.session)
            .maybe_mcp_headers(mcp_headers);
        if let Some(heap) = request.heap {
            execution = execution.heap(heap);
        }
        if let Some(heap_memory_max_mb) = heap_memory_max_mb {
            execution = execution.heap_memory_max_mb(heap_memory_max_mb);
        }
        if let Some(execution_timeout_secs) = request.execution_timeout_secs {
            execution = execution.execution_timeout_secs(execution_timeout_secs);
        }
        if let Some(tags) = request.tags {
            execution = execution.tags(tags);
        }
        execution.execute().await.map_err(operation_message)
    }

    pub fn get_execution(&self, execution_id: String) -> Result<ExecutionInfo, RuntimeError> {
        self.engine
            .get_execution(&execution_id)
            .map_err(operation_message)
    }

    pub fn get_execution_output(
        &self,
        execution_id: String,
        line_offset: Option<u64>,
        line_limit: Option<u64>,
        byte_offset: Option<u64>,
        byte_limit: Option<u64>,
    ) -> Result<ConsoleOutputPage, RuntimeError> {
        self.engine
            .get_execution_output(
                &execution_id,
                line_offset,
                line_limit,
                byte_offset,
                byte_limit,
            )
            .map_err(operation_message)
    }

    pub fn cancel_execution(&self, execution_id: String) -> Result<(), RuntimeError> {
        self.engine
            .cancel_execution(&execution_id)
            .map_err(operation_message)
    }

    pub fn list_executions(&self) -> Result<Vec<ExecutionSummary>, RuntimeError> {
        self.engine.list_executions().map_err(operation_message)
    }

    pub async fn list_sessions(&self) -> Result<Vec<String>, RuntimeError> {
        self.engine
            .list_sessions()
            .await
            .map_err(operation_message)
    }

    pub async fn list_session_snapshots(
        &self,
        session: String,
        fields: Option<Vec<String>>,
    ) -> Result<Vec<String>, RuntimeError> {
        self.engine
            .list_session_snapshots(session, fields)
            .await
            .map_err(operation_message)?
            .into_iter()
            .map(|snapshot| {
                serde_json::to_string(&snapshot).map_err(|error| RuntimeError::Operation {
                    message: format!("failed to serialize session snapshot: {error}"),
                })
            })
            .collect()
    }

    pub async fn get_heap_tags(
        &self,
        heap: String,
    ) -> Result<HashMap<String, String>, RuntimeError> {
        self.engine
            .get_heap_tags(heap)
            .await
            .map_err(operation_message)
    }

    pub async fn set_heap_tags(
        &self,
        heap: String,
        tags: HashMap<String, String>,
    ) -> Result<(), RuntimeError> {
        self.engine
            .set_heap_tags(heap, tags)
            .await
            .map_err(operation_message)
    }

    pub async fn delete_heap_tags(
        &self,
        heap: String,
        keys: Option<Vec<String>>,
    ) -> Result<(), RuntimeError> {
        self.engine
            .delete_heap_tags(heap, keys)
            .await
            .map_err(operation_message)
    }

    pub async fn query_heaps_by_tags(
        &self,
        tags: HashMap<String, String>,
    ) -> Result<Vec<HeapTagEntry>, RuntimeError> {
        self.engine
            .query_heaps_by_tags(tags)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_list_labels(&self) -> Result<Vec<FsLabelView>, RuntimeError> {
        self.engine.fs_list_labels().await.map_err(operation_message)
    }

    pub async fn fs_resolve_label(&self, name: String) -> Result<Option<String>, RuntimeError> {
        self.engine
            .fs_resolve_label(&name)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_set_label(
        &self,
        name: String,
        ca_id: String,
        message: Option<String>,
    ) -> Result<(), RuntimeError> {
        self.engine
            .fs_set_label(&name, &ca_id, message)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_label_log(
        &self,
        name: String,
        limit: Option<u64>,
    ) -> Result<Vec<FsRefLogView>, RuntimeError> {
        let limit =
            limit
                .map(usize::try_from)
                .transpose()
                .map_err(|_| RuntimeError::Operation {
                    message: "filesystem log limit is too large for this platform".to_string(),
                })?;
        self.engine
            .fs_label_log(&name, limit)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_push(
        &self,
        label: String,
        ca_id: String,
        expected: Option<String>,
        force: bool,
        message: Option<String>,
    ) -> Result<FsPushOutcome, RuntimeError> {
        self.engine
            .fs_push(&label, &ca_id, expected, force, message)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_reset(
        &self,
        label: String,
        ca_id: String,
        allow_unlogged: bool,
        message: Option<String>,
    ) -> Result<(), RuntimeError> {
        self.engine
            .fs_reset(&label, &ca_id, allow_unlogged, message)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_merge(
        &self,
        ours: String,
        theirs: String,
        base: Option<String>,
        prefer: Prefer,
    ) -> Result<FsMergeResult, RuntimeError> {
        self.engine
            .fs_merge(&ours, &theirs, base, prefer)
            .await
            .map_err(operation_message)
    }

    pub fn list_tools(&self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        self.mcp_tools()
            .into_iter()
            .map(|tool| {
                let input_schema_json =
                    serde_json::to_string(tool.input_schema.as_ref()).map_err(|error| {
                        RuntimeError::Initialization {
                            message: format!(
                                "failed to serialize schema for '{}': {error}",
                                tool.name
                            ),
                        }
                    })?;
                Ok(ToolDefinition {
                    name: tool.name.to_string(),
                    description: tool.description.map(|description| description.to_string()),
                    input_schema_json,
                })
            })
            .collect()
    }

    pub fn call_tool(
        &self,
        name: String,
        arguments_json: String,
        session_id: Option<String>,
        mcp_headers: Option<McpRequestHeaders>,
    ) -> Result<String, RuntimeError> {
        let tokio_runtime =
            self.tokio_runtime
                .as_ref()
                .ok_or_else(|| RuntimeError::Initialization {
                    message: "synchronous tool calls require a library-created runtime".to_string(),
                })?;
        tokio_runtime.block_on(self.invoke_tool(ToolCallRequest {
            name,
            arguments_json,
            session_id,
            mcp_headers,
        }))
    }

    pub async fn invoke_tool(
        &self,
        request: ToolCallRequest,
    ) -> Result<String, RuntimeError> {
        let _lifecycle_guard = self.shutdown_lock.lock().await;
        self.ensure_running()?;
        let arguments = parse_json_object("arguments_json", &request.arguments_json)?;
        let mcp_headers = request.mcp_headers.map(mcp_headers_value);
        let result = self
            .dispatch_tool(
                request.session_id.as_deref(),
                mcp_headers.as_ref(),
                &request.name,
                &arguments,
            )
            .await;

        serde_json::to_string(&result).map_err(|error| RuntimeError::ToolCall {
            message: format!("failed to serialize result: {error}"),
        })
    }
}

impl McpJsRuntime {
    /// Wrap a fully configured runtime for Rust transports without creating a
    /// second Tokio executor or crossing the FFI boundary.
    fn wrap(
        engine: Engine,
        tokio_runtime: Option<tokio::runtime::Runtime>,
        ephemeral_data_dir: Option<tempfile::TempDir>,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            tokio_runtime,
            engine,
            cluster_node,
            lifecycle: AtomicU8::new(RuntimeLifecycleState::Running as u8),
            shutdown_lock: tokio::sync::Mutex::new(()),
            _ephemeral_data_dir: ephemeral_data_dir,
        })
    }

    fn current_lifecycle_state(&self) -> RuntimeLifecycleState {
        match self.lifecycle.load(Ordering::Acquire) {
            value if value == RuntimeLifecycleState::Running as u8 => {
                RuntimeLifecycleState::Running
            }
            value if value == RuntimeLifecycleState::ShuttingDown as u8 => {
                RuntimeLifecycleState::ShuttingDown
            }
            _ => RuntimeLifecycleState::Shutdown,
        }
    }

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        match self.current_lifecycle_state() {
            RuntimeLifecycleState::Running => Ok(()),
            state => Err(RuntimeError::Operation {
                message: format!("runtime is {state:?}"),
            }),
        }
    }

    pub fn builder() -> McpJsRuntimeBuilder {
        McpJsRuntimeBuilder::default()
    }

    pub fn from_engine(engine: Engine) -> Arc<Self> {
        Self::from_engine_with_cluster(engine, None)
    }

    pub(crate) fn from_engine_with_cluster(
        engine: Engine,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Self::wrap(engine, None, None, cluster_node)
    }

    pub(crate) fn from_engine_with_tokio_runtime(
        engine: Engine,
        tokio_runtime: tokio::runtime::Runtime,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Self::wrap(engine, Some(tokio_runtime), None, cluster_node)
    }

    pub fn heap_enabled(&self) -> bool {
        self.engine.heap_enabled()
    }

    pub fn fs_enabled(&self) -> bool {
        self.engine.fs_enabled()
    }

    pub fn session_capable(&self) -> bool {
        self.engine.session_capable()
    }

    pub fn tool_catalog(&self) -> ToolCatalog {
        built_in_tool_catalog(self.heap_enabled(), self.fs_enabled())
    }

    pub fn instructions_override(&self) -> Option<Arc<str>> {
        self.engine.instructions_override()
    }

    pub fn run_js_description_override(&self) -> Option<Arc<str>> {
        self.engine.run_js_description_override()
    }

    pub fn wasm_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        self.engine.wasm_stub_tools()
    }

    pub fn core_mcp_tools(&self) -> Vec<rmcp::model::Tool> {
        crate::mcp::mode_tool_list(self)
    }

    pub fn mcp_tools(&self) -> Vec<rmcp::model::Tool> {
        let mut tools = self.core_mcp_tools();
        tools.extend(self.upstream_mcp_stub_tools());
        tools.extend(self.wasm_stub_tools());
        tools
    }

    pub fn upstream_mcp_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        self.engine
            .mcp_client_manager()
            .map(|client| client.stub_tools())
            .unwrap_or_default()
    }

    /// Dispatch a tool call against the full MCP tool catalog.
    pub async fn dispatch_tool(
        &self,
        session_id: Option<&str>,
        mcp_headers: Option<&Value>,
        name: &str,
        arguments: &Value,
    ) -> Value {
        if self.session_capable() {
            crate::mcp_dispatch::call_tool(&self.engine, session_id, mcp_headers, name, arguments)
                .await
        } else if name == "run_js" {
            crate::mcp_dispatch::run_js_blocking(&self.engine, mcp_headers, arguments).await
        } else {
            json!({ "error": format!("unknown stateless tool: {name}") })
        }
    }

    pub fn upstream_mcp_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        self.engine
            .mcp_client_manager()
            .and_then(|client| client.stub_call_response(name, arguments))
    }

    pub fn wasm_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        self.engine.wasm_stub_call_response(name, arguments)
    }

    pub fn run_js(&self, code: impl Into<String>) -> RunJsRequest<'_> {
        self.engine.run_js(code)
    }
}

fn operation_message(message: String) -> RuntimeError {
    RuntimeError::Operation { message }
}

fn validate_runtime_config(config: &RuntimeConfig) -> Result<(), RuntimeError> {
    if config.session_db_path.is_empty() {
        return Err(RuntimeError::InvalidConfig {
            message: "session_db_path must not be empty".to_string(),
        });
    }
    if config.heap_memory_max_mb < crate::engine::MIN_HEAP_MEMORY_MB as u64 {
        return Err(RuntimeError::InvalidConfig {
            message: format!(
                "heap_memory_max_mb must be at least {}",
                crate::engine::MIN_HEAP_MEMORY_MB
            ),
        });
    }
    if config.execution_timeout_secs == 0 || config.max_concurrent_executions == 0 {
        return Err(RuntimeError::InvalidConfig {
            message: "execution timeout and concurrency must be greater than zero".to_string(),
        });
    }
    let uses_s3 = matches!(config.heap_store, RuntimeStorageKind::S3)
        || matches!(config.filesystem_store, RuntimeStorageKind::S3);
    if uses_s3 && config.s3_bucket.is_none() {
        return Err(RuntimeError::InvalidConfig {
            message: "S3 storage requires s3_bucket".to_string(),
        });
    }
    Ok(())
}

fn runtime_bootstrap_config(
    config: RuntimeConfig,
) -> Result<crate::bootstrap::StorageBootstrapConfig, RuntimeError> {
    let heap_memory_max_mb =
        usize::try_from(config.heap_memory_max_mb).map_err(|_| RuntimeError::InvalidConfig {
            message: "heap_memory_max_mb is too large for this platform".to_string(),
        })?;
    Ok(crate::bootstrap::StorageBootstrapConfig {
        heap_store: storage_kind(config.heap_store),
        heap_dir: config.heap_dir,
        fs_store: storage_kind(config.filesystem_store),
        fs_dir: config.filesystem_dir,
        fs_labels_db: config.filesystem_labels_db,
        s3_bucket: config.s3_bucket,
        cache_dir: config.cache_dir,
        session_db_path: config.session_db_path,
        http_port: None,
        execution_db_path: config.execution_db_path,
        heap_memory_max_bytes: heap_memory_max_mb.checked_mul(1024 * 1024).ok_or_else(|| {
            RuntimeError::InvalidConfig {
                message: "heap_memory_max_mb is too large for this platform".to_string(),
            }
        })?,
        execution_timeout_secs: config.execution_timeout_secs,
        max_concurrent_executions: config.max_concurrent_executions as usize,
        session_id: config.session_id,
        session_fork_from: config.session_fork_from,
    })
}

fn storage_kind(kind: RuntimeStorageKind) -> crate::cli::StoreKind {
    match kind {
        RuntimeStorageKind::None => crate::cli::StoreKind::None,
        RuntimeStorageKind::Directory => crate::cli::StoreKind::Dir,
        RuntimeStorageKind::S3 => crate::cli::StoreKind::S3,
    }
}

fn validate_options(config: &RuntimeOptions) -> Result<(), RuntimeError> {
    if config.heap_memory_max_mb < crate::engine::MIN_HEAP_MEMORY_MB as u64 {
        return Err(RuntimeError::InvalidConfig {
            message: format!(
                "heap_memory_max_mb must be at least {}",
                crate::engine::MIN_HEAP_MEMORY_MB
            ),
        });
    }
    if config.execution_timeout_secs == 0 {
        return Err(RuntimeError::InvalidConfig {
            message: "execution_timeout_secs must be greater than zero".to_string(),
        });
    }
    if config.max_concurrent_executions == 0 {
        return Err(RuntimeError::InvalidConfig {
            message: "max_concurrent_executions must be greater than zero".to_string(),
        });
    }
    if matches!(config.mode, RuntimeMode::LocalStateful) && config.data_dir.is_none() {
        return Err(RuntimeError::InvalidConfig {
            message: "data_dir is required in local_stateful mode".to_string(),
        });
    }
    Ok(())
}

fn build_engine_from_options(
    config: &RuntimeOptions,
) -> Result<(Engine, Option<tempfile::TempDir>), RuntimeError> {
    let heap_memory_max_mb =
        usize::try_from(config.heap_memory_max_mb).map_err(|_| RuntimeError::InvalidConfig {
            message: "heap_memory_max_mb is too large for this platform".to_string(),
        })?;
    let ephemeral_data_dir =
        if matches!(config.mode, RuntimeMode::Stateless) && config.data_dir.is_none() {
            Some(
                tempfile::tempdir().map_err(|error| RuntimeError::Initialization {
                    message: format!("failed to create temporary data directory: {error}"),
                })?,
            )
        } else {
            None
        };
    let data_dir = config
        .data_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            ephemeral_data_dir
                .as_ref()
                .expect("temporary directory created")
                .path()
                .to_path_buf()
        });

    let builder = McpJsRuntime::builder()
        .heap_memory_max_mb(heap_memory_max_mb)
        .execution_timeout_secs(config.execution_timeout_secs)
        .max_concurrent_executions(config.max_concurrent_executions as usize)
        .filesystem_enabled(config.filesystem_enabled);
    let builder = match config.mode {
        RuntimeMode::Stateless => builder.stateless(data_dir),
        RuntimeMode::LocalStateful => builder.local_stateful(data_dir),
    };
    let engine = builder.build_engine().map_err(init_message)?;
    Ok((engine, ephemeral_data_dir))
}

fn mcp_headers_value(headers: McpRequestHeaders) -> Value {
    Value::Object(
        headers
            .values
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect(),
    )
}

fn parse_json_object(field: &str, json: &str) -> Result<Value, RuntimeError> {
    let value: Value = serde_json::from_str(json).map_err(|error| RuntimeError::InvalidJson {
        field: field.to_string(),
        message: error.to_string(),
    })?;
    if !value.is_object() {
        return Err(RuntimeError::InvalidJson {
            field: field.to_string(),
            message: "expected a JSON object".to_string(),
        });
    }
    Ok(value)
}

fn init_message(message: String) -> RuntimeError {
    RuntimeError::Initialization { message }
}

#[derive(Clone, Debug)]
enum RuntimeStorage {
    Stateless { data_dir: PathBuf },
    LocalStateful { data_dir: PathBuf },
}

#[derive(Clone, Debug)]
pub struct McpJsRuntimeBuilder {
    storage: Option<RuntimeStorage>,
    heap_memory_max_mb: usize,
    execution_timeout_secs: u64,
    max_concurrent_executions: usize,
    filesystem_enabled: bool,
}

impl Default for McpJsRuntimeBuilder {
    fn default() -> Self {
        Self {
            storage: None,
            heap_memory_max_mb: DEFAULT_HEAP_MEMORY_MB as usize,
            execution_timeout_secs: crate::engine::DEFAULT_EXECUTION_TIMEOUT_SECS,
            max_concurrent_executions: DEFAULT_MAX_CONCURRENT_EXECUTIONS as usize,
            filesystem_enabled: false,
        }
    }
}

impl McpJsRuntimeBuilder {
    pub fn stateless(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.storage = Some(RuntimeStorage::Stateless {
            data_dir: data_dir.into(),
        });
        self
    }

    pub fn local_stateful(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.storage = Some(RuntimeStorage::LocalStateful {
            data_dir: data_dir.into(),
        });
        self
    }

    pub fn heap_memory_max_mb(mut self, heap_memory_max_mb: usize) -> Self {
        self.heap_memory_max_mb = heap_memory_max_mb;
        self
    }

    pub fn execution_timeout_secs(mut self, execution_timeout_secs: u64) -> Self {
        self.execution_timeout_secs = execution_timeout_secs;
        self
    }

    pub fn max_concurrent_executions(mut self, max_concurrent_executions: usize) -> Self {
        self.max_concurrent_executions = max_concurrent_executions;
        self
    }

    pub fn filesystem_enabled(mut self, filesystem_enabled: bool) -> Self {
        self.filesystem_enabled = filesystem_enabled;
        self
    }

    pub fn build(self) -> Result<std::sync::Arc<McpJsRuntime>, String> {
        self.build_engine().map(McpJsRuntime::from_engine)
    }

    pub fn build_engine(self) -> Result<Engine, String> {
        self.validate()?;
        let heap_memory_max_bytes = self
            .heap_memory_max_mb
            .checked_mul(1024 * 1024)
            .ok_or_else(|| "heap_memory_max_mb is too large for this platform".to_string())?;
        let storage = self
            .storage
            .ok_or_else(|| "runtime storage mode is required".to_string())?;

        match storage {
            RuntimeStorage::Stateless { data_dir } => {
                create_data_dir(&data_dir)?;
                let registry = execution_registry(&data_dir)?;
                let engine = Engine::new_stateless(
                    heap_memory_max_bytes,
                    self.execution_timeout_secs,
                    self.max_concurrent_executions,
                )
                .with_execution_registry(Arc::new(registry));
                configure_filesystem(engine, &data_dir, self.filesystem_enabled, true)
            }
            RuntimeStorage::LocalStateful { data_dir } => {
                create_data_dir(&data_dir)?;
                let session_log = SessionLog::new(path_string(&data_dir.join("sessions"))?)?;
                let heap_tags = HeapTagStore::new(path_string(&data_dir.join("heap-tags"))?)?;
                let registry = execution_registry(&data_dir)?;
                let engine = Engine::new_stateful(
                    AnyHeapStorage::File(FileHeapStorage::new(data_dir.join("heaps"))),
                    Some(session_log),
                    Some(heap_tags),
                    heap_memory_max_bytes,
                    self.execution_timeout_secs,
                    self.max_concurrent_executions,
                )
                .with_execution_registry(Arc::new(registry));
                configure_filesystem(engine, &data_dir, self.filesystem_enabled, false)
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.heap_memory_max_mb < MIN_HEAP_MEMORY_MB {
            return Err(format!(
                "heap_memory_max_mb must be at least {MIN_HEAP_MEMORY_MB}"
            ));
        }
        if self.execution_timeout_secs == 0 {
            return Err("execution_timeout_secs must be greater than zero".to_string());
        }
        if self.max_concurrent_executions == 0 {
            return Err("max_concurrent_executions must be greater than zero".to_string());
        }
        Ok(())
    }
}

fn configure_filesystem(
    mut engine: Engine,
    data_dir: &Path,
    filesystem_enabled: bool,
    needs_session_log: bool,
) -> Result<Engine, String> {
    if !filesystem_enabled {
        return Ok(engine);
    }
    if needs_session_log {
        engine =
            engine.with_session_log(SessionLog::new(path_string(&data_dir.join("sessions"))?)?);
    }
    let backend = Arc::new(FileHeapStorage::new(data_dir.join("fs-blobs")));
    let store = Arc::new(FsStore::new(backend));
    let labels = Arc::new(LabelStore::new(path_string(&data_dir.join("fs-labels"))?)?);
    Ok(engine.with_fs_snapshots(store, labels))
}

fn create_data_dir(data_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| format!("failed to create '{}': {error}", data_dir.display()))
}

fn execution_registry(data_dir: &Path) -> Result<ExecutionRegistry, String> {
    ExecutionRegistry::new(path_string(&data_dir.join("executions"))?)
}

fn path_string(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn v8_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn typed_mcp_headers_convert_to_policy_json() {
        let headers = McpRequestHeaders {
            values: HashMap::from([
                ("session-id".to_string(), "session-123".to_string()),
                ("tenant".to_string(), "acme".to_string()),
            ]),
        };

        assert_eq!(
            mcp_headers_value(headers),
            serde_json::json!({
                "session-id": "session-123",
                "tenant": "acme",
            })
        );
    }

    #[test]
    fn rejects_non_object_arguments() {
        let error = parse_json_object("arguments_json", "[]").unwrap_err();
        assert!(error.to_string().contains("expected a JSON object"));
    }

    #[test]
    fn local_stateful_requires_data_dir() {
        let config = RuntimeOptions {
            mode: RuntimeMode::LocalStateful,
            data_dir: None,
            ..RuntimeOptions::default()
        };
        assert!(validate_options(&config).is_err());
    }

    #[test]
    fn runtime_config_builds_directory_storage_with_owned_executor() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let mut config = default_runtime_config(data_dir.path().to_string_lossy().into_owned());
        config.heap_store = RuntimeStorageKind::Directory;
        config.heap_dir = Some(data_dir.path().join("heaps").to_string_lossy().into_owned());
        config.filesystem_store = RuntimeStorageKind::Directory;
        config.filesystem_dir = Some(
            data_dir
                .path()
                .join("fs-blobs")
                .to_string_lossy()
                .into_owned(),
        );
        let library = create_runtime(config).unwrap();

        let capabilities = library.capabilities();
        assert!(capabilities.heap);
        assert!(capabilities.filesystem);
        assert!(capabilities.sessions);
        let tools = library.list_tools().unwrap();
        assert!(tools.iter().any(|tool| tool.name == "get_heap_tags"));
        assert!(tools.iter().any(|tool| tool.name == "fs_ls"));
    }

    #[test]
    fn runtime_config_requires_bucket_for_s3() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut config = default_runtime_config(data_dir.path().to_string_lossy().into_owned());
        config.heap_store = RuntimeStorageKind::S3;
        assert!(create_runtime(config).is_err());
    }

    #[test]
    fn upstream_mcp_config_deserializes_existing_json_shape() {
        let stdio: RuntimeMcpServerConfig = serde_json::from_str(
            r#"{"name":"weather","transport":"stdio","command":"python","args":["server.py"],"env":{"TOKEN":"x"}}"#,
        )
        .unwrap();
        assert!(matches!(stdio.transport, RuntimeMcpTransportKind::Stdio));
        assert_eq!(stdio.command.as_deref(), Some("python"));
        assert_eq!(stdio.args, ["server.py"]);
        assert_eq!(stdio.env.get("TOKEN").map(String::as_str), Some("x"));

        let sse: RuntimeMcpServerConfig = serde_json::from_str(
            r#"{"name":"remote","transport":"sse","url":"http://127.0.0.1/sse"}"#,
        )
        .unwrap();
        assert!(matches!(sse.transport, RuntimeMcpTransportKind::Sse));
        assert_eq!(sse.url.as_deref(), Some("http://127.0.0.1/sse"));
    }

    #[test]
    fn upstream_mcp_config_rejects_duplicate_names_before_connecting() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let server = RuntimeMcpServerConfig {
            name: "duplicate".to_string(),
            transport: RuntimeMcpTransportKind::Stdio,
            command: Some("true".to_string()),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
        };
        let upstreams = RuntimeUpstreamMcpConfig {
            servers: vec![server.clone(), server],
            stubs: default_upstream_mcp_config().stubs,
        };
        let result = create_runtime_with_upstreams(
            default_runtime_config(data_dir.path().to_string_lossy().into_owned()),
            default_feature_config(),
            default_policy_config(),
            default_capability_config(),
            upstreams,
        );
        let error = match result {
            Ok(_) => panic!("duplicate upstream names should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("duplicate MCP server name"));
    }

    #[test]
    fn configured_capabilities_allow_run_js_file() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let script = data_dir.path().join("script.js");
        std::fs::write(&script, "console.log(6 * 7)").unwrap();
        let capabilities = RuntimeCapabilityConfig {
            run_js_file_access: RuntimeRunJsFileAccess::AllowAll,
            ..default_capability_config()
        };
        let library = create_runtime_with_configuration(
            default_runtime_config(data_dir.path().to_string_lossy().into_owned()),
            default_feature_config(),
            default_policy_config(),
            capabilities,
        )
        .unwrap();

        let result = library
            .call_tool(
                "run_js".to_string(),
                serde_json::json!({ "file": script.to_string_lossy() }).to_string(),
                None,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"], "42");
    }

    #[test]
    fn configured_policies_reject_invalid_sources() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let policies = RuntimePolicyConfig {
            fetch: Some(RuntimeOperationPolicies {
                mode: RuntimePolicyEvalMode::All,
                policies: vec![RuntimePolicySource {
                    url: "ftp://invalid".to_string(),
                    policy_path: None,
                    rule: None,
                }],
            }),
            ..default_policy_config()
        };
        let result = create_runtime_with_configuration(
            default_runtime_config(data_dir.path().to_string_lossy().into_owned()),
            default_feature_config(),
            policies,
            default_capability_config(),
        );
        let error = match result {
            Ok(_) => panic!("invalid policy source should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("Unsupported policy URL scheme"));
    }

    #[test]
    fn configured_features_reject_wasm_with_heap_persistence() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let mut runtime_config =
            default_runtime_config(data_dir.path().to_string_lossy().into_owned());
        runtime_config.heap_store = RuntimeStorageKind::Directory;
        runtime_config.heap_dir =
            Some(data_dir.path().join("heaps").to_string_lossy().into_owned());
        let mut features = default_feature_config();
        features.wasm_modules.push(RuntimeWasmModuleConfig {
            name: "math".to_string(),
            bytes: b"\0asm".to_vec(),
            max_memory_bytes: None,
            description: None,
        });

        let error = match create_runtime_with_features(runtime_config, features) {
            Ok(_) => panic!("heap persistence with WASM should be rejected"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("incompatible with heap persistence")
        );
    }

    #[test]
    fn configured_features_apply_through_exported_factory() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let mut features = default_feature_config();
        features.instructions_override = Some("Custom instructions".to_string());
        features.run_js_description_override = Some("Custom run_js".to_string());
        features.wasm_modules.push(RuntimeWasmModuleConfig {
            name: "math".to_string(),
            bytes: b"\0asm".to_vec(),
            max_memory_bytes: Some(1024 * 1024),
            description: Some("Math helpers".to_string()),
        });
        features.wasm_stubs.prefix = "ffi__".to_string();

        let library = create_runtime_with_features(
            default_runtime_config(data_dir.path().to_string_lossy().into_owned()),
            features,
        )
        .unwrap();
        assert_eq!(
            library.instructions_override().as_deref(),
            Some("Custom instructions")
        );
        assert_eq!(
            library.run_js_description_override().as_deref(),
            Some("Custom run_js")
        );
        assert!(
            library
                .list_tools()
                .unwrap()
                .iter()
                .any(|tool| tool.name == "ffi__wasm__math")
        );
    }

    #[test]
    fn configured_filesystem_uses_typed_label_api() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let library = McpJsRuntime::new(RuntimeOptions {
            mode: RuntimeMode::Stateless,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            filesystem_enabled: true,
            ..RuntimeOptions::default()
        })
        .unwrap();
        let runtime = library.tokio_runtime.as_ref().unwrap();
        let first = "0".repeat(64);
        let second = "1".repeat(64);

        assert!(library.capabilities().filesystem);
        runtime
            .block_on(library.fs_set_label(
                "main".to_string(),
                first.clone(),
                Some("create".to_string()),
            ))
            .unwrap();
        assert_eq!(
            runtime
                .block_on(library.fs_resolve_label("main".to_string()))
                .unwrap(),
            Some(first.clone())
        );
        assert_eq!(runtime.block_on(library.fs_list_labels()).unwrap().len(), 1);

        let pushed = runtime
            .block_on(library.fs_push(
                "main".to_string(),
                second.clone(),
                Some(first.clone()),
                false,
                Some("advance".to_string()),
            ))
            .unwrap();
        match pushed {
            FsPushOutcome::Advanced { label, ca_id } => {
                assert_eq!(label, "main");
                assert_eq!(ca_id, second);
            }
            other => panic!("expected an advanced push, got {other:?}"),
        }
        assert_eq!(
            runtime
                .block_on(library.fs_label_log("main".to_string(), None))
                .unwrap()
                .len(),
            2
        );

        runtime
            .block_on(library.fs_reset(
                "main".to_string(),
                first.clone(),
                false,
                Some("rollback".to_string()),
            ))
            .unwrap();
        assert_eq!(
            runtime
                .block_on(library.fs_resolve_label("main".to_string()))
                .unwrap(),
            Some(first)
        );
    }

    #[test]
    fn local_stateful_sessions_and_heap_tags_use_typed_api() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let library = McpJsRuntime::new(RuntimeOptions {
            mode: RuntimeMode::LocalStateful,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            ..RuntimeOptions::default()
        })
        .unwrap();
        let runtime = library.tokio_runtime.as_ref().unwrap();

        assert!(
            runtime
                .block_on(library.list_sessions())
                .unwrap()
                .is_empty()
        );

        let tags = HashMap::from([
            ("environment".to_string(), "test".to_string()),
            ("owner".to_string(), "uniffi".to_string()),
        ]);
        runtime
            .block_on(library.set_heap_tags("heap-1".to_string(), tags.clone()))
            .unwrap();
        assert_eq!(
            runtime
                .block_on(library.get_heap_tags("heap-1".to_string()))
                .unwrap(),
            tags
        );

        let matches =
            runtime
                .block_on(library.query_heaps_by_tags(HashMap::from([(
                    "owner".to_string(),
                    "uniffi".to_string(),
                )])))
                .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].heap, "heap-1");

        runtime
            .block_on(library.delete_heap_tags("heap-1".to_string(), None))
            .unwrap();
        assert!(
            runtime
                .block_on(library.get_heap_tags("heap-1".to_string()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn stateless_run_js_executes_through_sync_and_async_library_apis() {
        let _guard = v8_test_guard();
        let library = McpJsRuntime::new(RuntimeOptions::default()).unwrap();
        let result = library
            .call_tool(
                "run_js".to_string(),
                r#"{"code":"console.log(1 + 1)"}"#.to_string(),
                None,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"], "2");

        let runtime = library.tokio_runtime.as_ref().unwrap();
        let result = runtime
            .block_on(library.invoke_tool(ToolCallRequest {
                name: "run_js".to_string(),
                arguments_json: r#"{"code":"console.log(2 + 2)"}"#.to_string(),
                session_id: None,
                mcp_headers: None,
            }))
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"], "4");
    }

    #[test]
    fn lifecycle_shutdown_is_idempotent_and_rejects_new_work() {
        let _guard = v8_test_guard();
        let library = McpJsRuntime::new(RuntimeOptions::default()).unwrap();
        assert_eq!(library.lifecycle_state(), RuntimeLifecycleState::Running);

        let runtime = library.tokio_runtime.as_ref().unwrap();
        let first = runtime.block_on(library.shutdown());
        assert!(!first.already_shutdown);
        assert_eq!(library.lifecycle_state(), RuntimeLifecycleState::Shutdown);

        let second = runtime.block_on(library.shutdown());
        assert!(second.already_shutdown);
        let error = runtime
            .block_on(library.invoke_tool(ToolCallRequest {
                name: "run_js".to_string(),
                arguments_json: r#"{"code":"console.log('late')"}"#.to_string(),
                session_id: None,
                mcp_headers: None,
            }))
            .unwrap_err();
        assert!(error.to_string().contains("runtime is Shutdown"));
    }

    #[test]
    fn local_stateful_tools_submit_poll_and_read_output() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let library = McpJsRuntime::new(RuntimeOptions {
            mode: RuntimeMode::LocalStateful,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            ..RuntimeOptions::default()
        })
        .unwrap();

        let runtime = library.tokio_runtime.as_ref().unwrap();
        let execution_id = runtime
            .block_on(library.submit_execution(ExecutionRequest {
                code: "console.log(40 + 2)".to_string(),
                file: None,
                heap: None,
                fs: None,
                session: Some("ffi-test".to_string()),
                heap_memory_max_mb: None,
                execution_timeout_secs: None,
                tags: None,
                mcp_headers: None,
            }))
            .unwrap();

        let mut completed = false;
        for _ in 0..200 {
            let status = library.get_execution(execution_id.clone()).unwrap();
            if status.status == "completed" {
                completed = true;
                break;
            }
            if matches!(status.status.as_str(), "failed" | "timed_out" | "cancelled") {
                panic!("stateful execution failed: {}", status.status);
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(completed, "stateful execution did not complete");

        let output = library
            .get_execution_output(execution_id, None, None, None, None)
            .unwrap();
        assert_eq!(output.data, "42");
    }
}

#[cfg(test)]
mod builder_tests {
    use super::*;

    #[test]
    fn builder_requires_storage_mode() {
        assert!(McpJsRuntime::builder().build().is_err());
    }

    #[test]
    fn stateless_builder_configures_execution_registry() {
        let data_dir = tempfile::tempdir().unwrap();
        let runtime = McpJsRuntime::builder()
            .stateless(data_dir.path())
            .build()
            .unwrap();

        assert!(!runtime.session_capable());
        assert!(runtime.list_executions().is_ok());
    }

    #[test]
    fn stateless_builder_can_enable_filesystem_snapshots() {
        let data_dir = tempfile::tempdir().unwrap();
        let runtime = McpJsRuntime::builder()
            .stateless(data_dir.path())
            .filesystem_enabled(true)
            .build()
            .unwrap();

        assert!(!runtime.heap_enabled());
        assert!(runtime.fs_enabled());
        assert!(runtime.session_capable());
    }

    #[test]
    fn local_stateful_builder_enables_heap_tools() {
        let data_dir = tempfile::tempdir().unwrap();
        let runtime = McpJsRuntime::builder()
            .local_stateful(data_dir.path())
            .build()
            .unwrap();

        assert!(runtime.heap_enabled());
        assert!(
            runtime
                .tool_catalog()
                .tools
                .iter()
                .any(|tool| tool.name == "get_heap_tags")
        );
    }
}
