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

// ── Neutralize dangerous built-in ops ────────────────────────────────────

/// Replace dangerous built-in deno_core ops with safe pure-JS alternatives.
///
/// Implemented entirely in JavaScript to avoid registering an additional
/// deno_core extension. This matters because `JsRuntimeForSnapshot`'s
/// `prepare_for_snapshot()` calls `std::mem::forget(self)`, which leaks the
/// internal `extensions: Vec<&'static str>` — each extra extension adds to
/// that leak. Pure-JS neutralization avoids the problem entirely.
///
/// Must be called after the runtime is created but before any user code runs.
/// 1. Replaces `Deno.core.ops.op_panic` with a JS function that throws
/// 2. Replaces `Deno.core.ops.op_print` with a JS function that routes
///    through `op_console_write` (if available) or silently discards
/// 3. Replaces `Deno.core` with a flat copy (null prototype) that overrides
///    `print` — severing the prototype chain to the original frozen core
///    whose `print` held a closure reference to the native `op_print`
/// 4. Makes `Deno.core` non-configurable to prevent user code from
///    reversing the replacement
pub fn neutralize_dangerous_ops(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<sandbox-setup>", SANDBOX_SETUP_JS.to_string())
        .map_err(|e| format!("Failed to neutralize dangerous ops: {}", e))?;
    Ok(())
}

const SANDBOX_SETUP_JS: &str = r#"
(function() {
    // Replace op_panic: throw a JS Error instead of calling Rust panic!().
    Deno.core.ops.op_panic = function(msg) {
        throw new Error("panic: " + msg);
    };

    // Replace op_print: route through console capture if available, else discard.
    // This prevents direct writes to stdout/stderr which would corrupt the
    // JSON-RPC protocol stream.
    var safePrint = function(msg, isErr) {
        if (typeof Deno.core.ops.op_console_write === 'function') {
            Deno.core.ops.op_console_write(isErr ? "[WARN] " + msg : msg, isErr ? 2 : 0);
        }
        // If no console capture, silently discard (safer than writing to stdout)
    };
    Deno.core.ops.op_print = safePrint;

    // CRITICAL FIX: Deno.core is frozen by deno_core bootstrap, so we cannot
    // modify Deno.core.print in-place. The old approach used Object.create()
    // to inherit from origCore and shadow `print`, but this left the original
    // native print accessible via Object.getPrototypeOf(Deno.core).print,
    // which captures the native op_print in a bootstrap closure — bypassing
    // neutralization and writing directly to stdout (corrupting JSON-RPC).
    //
    // Fix: copy all own properties from origCore to a plain object (no
    // prototype chain), overriding `print` with the safe version.
    var origCore = Deno.core;
    var newCore = Object.create(null);

    // Copy all own properties (including symbols) from the original core.
    var names = Object.getOwnPropertyNames(origCore);
    for (var i = 0; i < names.length; i++) {
        if (names[i] === 'print') continue; // override below
        try {
            var desc = Object.getOwnPropertyDescriptor(origCore, names[i]);
            if (desc) {
                Object.defineProperty(newCore, names[i], desc);
            }
        } catch (e) {
            try { newCore[names[i]] = origCore[names[i]]; } catch (_) {}
        }
    }
    var syms = Object.getOwnPropertySymbols(origCore);
    for (var i = 0; i < syms.length; i++) {
        try {
            var desc = Object.getOwnPropertyDescriptor(origCore, syms[i]);
            if (desc) {
                Object.defineProperty(newCore, syms[i], desc);
            }
        } catch (e) {
            try { newCore[syms[i]] = origCore[syms[i]]; } catch (_) {}
        }
    }

    // Override print with the safe version (non-configurable, non-writable).
    Object.defineProperty(newCore, 'print', {
        value: safePrint,
        writable: false,
        configurable: false,
        enumerable: true,
    });

    // Replace Deno.core with the new object that has NO prototype chain
    // back to the original frozen core. Use configurable: false so user
    // code cannot reverse this replacement.
    Object.defineProperty(globalThis.Deno, 'core', {
        value: newCore,
        writable: false,
        configurable: false,
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

// ── Post-setup sandbox hardening ─────────────────────────────────────────

/// Final hardening pass that locks down the sandbox after all extensions and
/// JS wrappers have been injected, but before user code runs.
///
/// Must be called AFTER inject_console, neutralize_dangerous_ops, inject_fetch,
/// inject_fs, inject_mcp, and inject_timers — otherwise it will freeze ops
/// before they are set up, breaking the runtime.
///
/// 1. Neutralizes dangerous introspection ops (op_get_proxy_details, etc.)
/// 2. Freezes Deno.core.ops to prevent interception/replacement
/// 3. Removes __bootstrap (event loop hooks, primordials, internals)
/// 4. Removes SharedArrayBuffer (Spectre timer prerequisite)
pub fn harden_runtime(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<sandbox-hardening>", HARDENING_JS.to_string())
        .map_err(|e| format!("Failed to harden sandbox: {}", e))?;
    Ok(())
}

const HARDENING_JS: &str = r#"
(function() {
    // 1. Neutralize dangerous introspection/info-leak ops before freezing.
    //    These must be replaced on the ops object before Object.freeze.
    Deno.core.ops.op_get_proxy_details = function() { return undefined; };
    Deno.core.ops.op_memory_usage = function() { return {}; };
    Deno.core.ops.op_is_terminal = function() { return false; };

    // 2. Freeze ops to prevent interception/replacement of any op.
    //    All setup (console, fetch, fs, mcp) has completed by this point.
    Object.freeze(Deno.core.ops);

    // 3. Remove __bootstrap: exposes event-loop hooks (setMacrotaskCallback,
    //    setPromiseHooks, addMainModuleHandler, etc.), primordials (pristine
    //    Function constructor), and internal registration objects.
    //    deno_core's own bootstrap has already completed, so this is safe.
    delete globalThis.__bootstrap;

    // 4. Remove SharedArrayBuffer: prerequisite for Spectre-style timing
    //    attacks. Workers are not available, but defense-in-depth applies.
    //    V8 flags cannot disable it (stable spec feature), so remove from JS.
    delete globalThis.SharedArrayBuffer;
    delete globalThis.Atomics;
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
