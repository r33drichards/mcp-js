//! Console output capture for the JavaScript runtime.
//!
//! Intercepts `console.log`, `console.info`, `console.warn`, and `console.error`
//! calls and streams the output as a byte stream into a sled tree. Writes are
//! buffered in-memory and flushed to sled in fixed-size pages (WAL-style) for
//! efficient batching.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;

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

/// Replacement for deno_core's built-in `op_panic`. Instead of calling
/// `panic!()`, returns the message as a JS exception (Error).
#[op2(fast)]
fn op_safe_panic(#[string] msg: &str) -> Result<(), JsErrorBox> {
    Err(JsErrorBox::generic(format!("panic: {}", msg)))
}

/// Replacement for deno_core's built-in `Deno.core.print`. Routes output
/// through ConsoleLogState (if available) instead of writing to process
/// stdout/stderr which would corrupt the JSON-RPC protocol stream.
#[op2(fast)]
fn op_safe_print(state: &mut OpState, #[string] msg: &str, is_err: bool) {
    if let Some(console_state) = state.try_borrow::<ConsoleLogState>() {
        let formatted = if is_err {
            format!("[WARN] {}", msg)
        } else {
            msg.to_string()
        };
        console_state.write(formatted.as_bytes());
    }
    // If no ConsoleLogState, silently discard (safer than writing to stdout)
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

deno_core::extension!(
    sandbox_ext,
    ops = [op_safe_panic, op_safe_print],
);

/// Create the sandbox-hardening extension. Always loaded (regardless of
/// console_tree) so that safe replacement ops exist for `neutralize_dangerous_ops()`.
pub fn create_sandbox_extension() -> deno_core::Extension {
    sandbox_ext::init()
}

// ── Neutralize dangerous built-in ops ────────────────────────────────────

/// Replace dangerous built-in deno_core ops with safe alternatives.
///
/// Must be called after the runtime is created (with sandbox_ext loaded)
/// but before any user code runs.
/// 1. Replaces `Deno.core.ops.op_panic` with `Deno.core.ops.op_safe_panic`
/// 2. Replaces `Deno.core.print` with a wrapper around `Deno.core.ops.op_safe_print`
/// 3. Replaces `Deno.core.ops.op_print` with `Deno.core.ops.op_safe_print`
pub fn neutralize_dangerous_ops(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<sandbox-setup>", SANDBOX_SETUP_JS.to_string())
        .map_err(|e| format!("Failed to neutralize dangerous ops: {}", e))?;
    Ok(())
}

const SANDBOX_SETUP_JS: &str = r#"
(function() {
    // Replace dangerous ops on the (non-frozen) ops object.
    Deno.core.ops.op_panic = Deno.core.ops.op_safe_panic;
    Deno.core.ops.op_print = Deno.core.ops.op_safe_print;

    // Deno.core is frozen by deno_core bootstrap, so we cannot modify
    // Deno.core.print in-place. Instead, create a new core object that
    // inherits all original properties but overrides print.
    var origCore = Deno.core;
    var safePrint = function(msg, isErr) {
        Deno.core.ops.op_safe_print(msg, isErr || false);
    };
    var newCore = Object.create(origCore);
    Object.defineProperty(newCore, 'print', {
        value: safePrint,
        writable: false,
        configurable: false,
        enumerable: true,
    });
    // The Deno object is NOT frozen, so we can replace the core property.
    Object.defineProperty(globalThis.Deno, 'core', {
        value: newCore,
        writable: false,
        configurable: true,
        enumerable: true,
    });
})();
"#;

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
