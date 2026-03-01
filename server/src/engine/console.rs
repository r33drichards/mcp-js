//! Console output capture for the JavaScript runtime.
//!
//! Intercepts `console.log`, `console.info`, `console.warn`, and `console.error`
//! calls and streams the output as a byte stream into a sled tree. Writes are
//! buffered in-memory and flushed to sled in fixed-size pages (WAL-style) for
//! efficient batching.
//!
//! Uses V8's cppgc (Oilpan garbage collector) to manage the `ConsoleWriter`
//! lifetime — the JS wrapper holds the native writer object, and V8's GC
//! reclaims it when it goes out of scope.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use deno_core::{JsRuntime, OpState, op2};
use deno_core::GarbageCollected;

// ── Configuration ────────────────────────────────────────────────────────

/// Page size for WAL-style writes to sled. When the in-memory buffer reaches
/// this size, a full page is flushed to sled. Remaining bytes are flushed on
/// execution end.
const PAGE_SIZE: usize = 4096;

// ── cppgc-managed ConsoleWriter ─────────────────────────────────────────

/// Per-execution console output writer, tracked by V8's Oilpan garbage
/// collector. Buffers console output bytes and flushes them to sled in
/// fixed-size pages. Interior mutability via RefCell since cppgc hands
/// out `&self` references.
pub struct ConsoleWriter {
    tree: sled::Tree,
    seq: AtomicU64,
    buffer: RefCell<Vec<u8>>,
}

// Safety: ConsoleWriter is only accessed from the single V8 thread that
// owns the isolate. RefCell provides runtime borrow checking within that
// thread. AtomicU64 is inherently thread-safe. sled::Tree is Send+Sync.
unsafe impl Send for ConsoleWriter {}
unsafe impl Sync for ConsoleWriter {}

unsafe impl GarbageCollected for ConsoleWriter {
    fn trace(&self, _visitor: &mut deno_core::v8::cppgc::Visitor) {
        // No pointers to other GC objects — nothing to trace.
    }

    fn get_name(&self) -> &'static std::ffi::CStr {
        c"ConsoleWriter"
    }
}

impl ConsoleWriter {
    pub fn new(tree: sled::Tree) -> Self {
        Self {
            tree,
            seq: AtomicU64::new(0),
            buffer: RefCell::new(Vec::with_capacity(PAGE_SIZE)),
        }
    }

    /// Append bytes to the buffer, flushing full pages to sled.
    pub fn write(&self, data: &[u8]) {
        let mut buf = self.buffer.borrow_mut();
        buf.extend_from_slice(data);
        while buf.len() >= PAGE_SIZE {
            let page: Vec<u8> = buf.drain(..PAGE_SIZE).collect();
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            let _ = self.tree.insert(seq.to_be_bytes(), page);
        }
    }

    /// Flush any remaining buffered bytes to sled (call on execution end).
    pub fn flush(&self) {
        let mut buf = self.buffer.borrow_mut();
        if !buf.is_empty() {
            let page: Vec<u8> = buf.drain(..).collect();
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            let _ = self.tree.insert(seq.to_be_bytes(), page);
        }
    }
}

// ── Op definitions ──────────────────────────────────────────────────────

/// Create a cppgc-managed ConsoleWriter from the sled tree stored in OpState.
/// Called once during JS wrapper initialization.
#[op2]
#[cppgc]
fn op_console_init(state: &mut OpState) -> ConsoleWriter {
    let tree = state.take::<sled::Tree>();
    ConsoleWriter::new(tree)
}

/// Sync op: writes formatted console output bytes into the buffered WAL.
/// Called from JS via the console wrapper, passing the cppgc writer handle.
/// level: 0=log, 1=info, 2=warn, 3=error
#[op2(fast)]
fn op_console_write(#[cppgc] writer: &ConsoleWriter, #[string] msg: &str, #[smi] level: i32) {
    let formatted = match level {
        2 => format!("[WARN] {}\n", msg),
        3 => format!("[ERROR] {}\n", msg),
        1 => format!("[INFO] {}\n", msg),
        _ => format!("{}\n", msg),
    };

    writer.write(formatted.as_bytes());
}

// ── Extension registration ───────────────────────────────────────────────

deno_core::extension!(
    console_ext,
    ops = [op_console_init, op_console_write],
);

/// Create the console extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    console_ext::init()
}

// ── Inject console JS wrapper into the global scope ──────────────────────

/// Inject the `globalThis.console` JS wrapper. Must be called after the
/// runtime is created (with the console extension) but before user code runs.
pub fn inject_console(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<console-setup>", CONSOLE_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install console wrapper: {}", e))?;
    Ok(())
}

/// Overload for JsRuntimeForSnapshot (stateful mode).
pub fn inject_console_snapshot(runtime: &mut deno_core::JsRuntimeForSnapshot) -> Result<(), String> {
    runtime
        .execute_script("<console-setup>", CONSOLE_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install console wrapper: {}", e))?;
    Ok(())
}

/// JavaScript wrapper that overrides `globalThis.console` to route output
/// through the cppgc-managed `ConsoleWriter` via `op_console_write`.
///
/// The writer is created once via `op_console_init` and stored in a closure.
/// It's also set as `globalThis.__console_writer` so Rust can access it
/// for flushing via the V8 global object.
const CONSOLE_JS_WRAPPER: &str = r#"
(function() {
    var writer;
    if (Deno.core.ops.op_console_init) {
        writer = Deno.core.ops.op_console_init();
    }
    function formatArgs(args) {
        return args.map(function(a) {
            if (typeof a === 'string') return a;
            try { return JSON.stringify(a); } catch(e) { return String(a); }
        }).join(' ');
    }
    globalThis.console = {
        log: function() { if (writer) Deno.core.ops.op_console_write(writer, formatArgs(Array.from(arguments)), 0); },
        info: function() { if (writer) Deno.core.ops.op_console_write(writer, formatArgs(Array.from(arguments)), 1); },
        warn: function() { if (writer) Deno.core.ops.op_console_write(writer, formatArgs(Array.from(arguments)), 2); },
        error: function() { if (writer) Deno.core.ops.op_console_write(writer, formatArgs(Array.from(arguments)), 3); },
        debug: function() { if (writer) Deno.core.ops.op_console_write(writer, formatArgs(Array.from(arguments)), 0); },
        trace: function() { if (writer) Deno.core.ops.op_console_write(writer, formatArgs(Array.from(arguments)), 0); },
    };
    globalThis.__console_writer = writer;
})();
"#;

// ── Flush helper ─────────────────────────────────────────────────────────

/// Flush any remaining console output from the cppgc-managed ConsoleWriter.
/// Retrieves the writer from `globalThis.__console_writer` via V8's global
/// object and calls flush(). Call after V8 execution completes but before
/// the runtime is dropped.
pub fn flush_console(runtime: &mut JsRuntime) {
    use deno_core::v8;

    deno_core::scope!(scope, runtime);
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, "__console_writer").unwrap();
    if let Some(val) = global.get(scope, key.into()) {
        if let Some(writer) = deno_core::cppgc::try_unwrap_cppgc_object::<ConsoleWriter>(scope, val) {
            writer.flush();
        }
    }
}

/// Flush helper for JsRuntimeForSnapshot (stateful mode).
pub fn flush_console_snapshot(runtime: &mut deno_core::JsRuntimeForSnapshot) {
    use deno_core::v8;

    deno_core::scope!(scope, runtime);
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, "__console_writer").unwrap();
    if let Some(val) = global.get(scope, key.into()) {
        if let Some(writer) = deno_core::cppgc::try_unwrap_cppgc_object::<ConsoleWriter>(scope, val) {
            writer.flush();
        }
    }
}
