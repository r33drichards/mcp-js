//! Console output capture for the JavaScript runtime.
//!
//! Intercepts `console.log`, `console.info`, `console.warn`, and `console.error`
//! calls and streams the output as a byte stream into a sled tree. Writes are
//! buffered in-memory and flushed to sled in fixed-size pages (WAL-style) for
//! efficient batching.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use deno_core::{JsRuntime, OpState, op2};

// ── Configuration ────────────────────────────────────────────────────────

/// Page size for WAL-style writes to sled. When the in-memory buffer reaches
/// this size, a full page is flushed to sled. Remaining bytes are flushed on
/// execution end.
const PAGE_SIZE: usize = 4096;

/// Per-execution console output state, stored in deno_core's `OpState`.
/// Buffers console output bytes and flushes them to sled in fixed-size pages.
pub struct ConsoleLogState {
    tree: sled::Tree,
    seq: AtomicU64,
    buffer: RefCell<Vec<u8>>,
}

// Safety: ConsoleLogState is only accessed from a single V8 thread.
// The RefCell ensures runtime borrow checking. AtomicU64 is inherently
// thread-safe. sled::Tree is Send+Sync.
unsafe impl Send for ConsoleLogState {}
unsafe impl Sync for ConsoleLogState {}

impl ConsoleLogState {
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

// ── Op definition ────────────────────────────────────────────────────────

/// Sync op: writes formatted console output bytes into the buffered WAL.
/// Called from JS via `Deno.core.ops.op_console_write(msg, level)`.
/// level: 0=log, 1=info, 2=warn, 3=error
#[op2(fast)]
fn op_console_write(state: &mut OpState, #[string] msg: &str, #[smi] level: i32) {
    let console_state = state.borrow::<ConsoleLogState>();

    let formatted = match level {
        2 => format!("[WARN] {}\n", msg),
        3 => format!("[ERROR] {}\n", msg),
        1 => format!("[INFO] {}\n", msg),
        _ => format!("{}\n", msg),
    };

    console_state.write(formatted.as_bytes());
}

// ── Extension registration ───────────────────────────────────────────────

deno_core::extension!(
    console_ext,
    ops = [op_console_write],
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
/// through `op_console_write`.
const CONSOLE_JS_WRAPPER: &str = r#"
(function() {
    function formatArgs(args) {
        return args.map(function(a) {
            if (typeof a === 'string') return a;
            try { return JSON.stringify(a); } catch(e) { return String(a); }
        }).join(' ');
    }
    globalThis.console = {
        log: function() { Deno.core.ops.op_console_write(formatArgs(Array.from(arguments)), 0); },
        info: function() { Deno.core.ops.op_console_write(formatArgs(Array.from(arguments)), 1); },
        warn: function() { Deno.core.ops.op_console_write(formatArgs(Array.from(arguments)), 2); },
        error: function() { Deno.core.ops.op_console_write(formatArgs(Array.from(arguments)), 3); },
        debug: function() { Deno.core.ops.op_console_write(formatArgs(Array.from(arguments)), 0); },
        trace: function() { Deno.core.ops.op_console_write(formatArgs(Array.from(arguments)), 0); },
    };
})();
"#;

// ── Flush helper ─────────────────────────────────────────────────────────

/// Flush any remaining console output from the runtime's OpState.
/// Call this after V8 execution completes but before the runtime is dropped.
pub fn flush_console(runtime: &mut JsRuntime) {
    let state = runtime.op_state();
    let state = state.borrow();
    if let Some(console_state) = state.try_borrow::<ConsoleLogState>() {
        console_state.flush();
    }
}

/// Flush helper for JsRuntimeForSnapshot (stateful mode).
pub fn flush_console_snapshot(runtime: &mut deno_core::JsRuntimeForSnapshot) {
    let state = runtime.op_state();
    let state = state.borrow();
    if let Some(console_state) = state.try_borrow::<ConsoleLogState>() {
        console_state.flush();
    }
}
