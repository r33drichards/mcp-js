//! Console output capture for the JavaScript runtime.
//!
//! Intercepts `console.log`, `console.info`, `console.warn`, and `console.error`
//! calls and streams the output as a byte stream into a sled tree. Writes are
//! buffered in-memory and flushed to sled in fixed-size pages (WAL-style) for
//! efficient batching.

use std::cell::RefCell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
};

use deno_core::{JsRuntime, OpState, op2, v8};

// ── Configuration ────────────────────────────────────────────────────────

/// Page size for WAL-style writes to sled. When the in-memory buffer reaches
/// this size, a full page is flushed to sled. Remaining bytes are flushed on
/// execution end.
const PAGE_SIZE: usize = 4096;

#[derive(Clone, Default)]
pub struct ProcessExitState {
    exit_code: Arc<AtomicI32>,
    exit_requested: Arc<AtomicBool>,
}

impl ProcessExitState {
    pub fn exit_code(&self) -> i32 {
        self.exit_code.load(Ordering::SeqCst)
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::SeqCst)
    }

    fn set_exit_code(&self, code: i32) {
        self.exit_code.store(code, Ordering::SeqCst);
    }

    fn request_exit(&self, code: i32) {
        self.set_exit_code(code);
        self.exit_requested.store(true, Ordering::SeqCst);
    }
}

/// Per-execution console output state, stored in deno_core's `OpState`.
/// Buffers console output bytes and flushes them to sled in fixed-size pages.
pub struct ConsoleLogState {
    tree: sled::Tree,
    seq: AtomicU64,
    buffer: RefCell<Vec<u8>>,
    stderr_tree: Option<sled::Tree>,
    stderr_seq: AtomicU64,
    stderr_buffer: RefCell<Vec<u8>>,
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
            stderr_tree: None,
            stderr_seq: AtomicU64::new(0),
            stderr_buffer: RefCell::new(Vec::with_capacity(PAGE_SIZE)),
        }
    }

    pub fn new_with_stderr(tree: sled::Tree, stderr_tree: sled::Tree) -> Self {
        let mut state = Self::new(tree);
        state.stderr_tree = Some(stderr_tree);
        state
    }

    fn write_buffer(
        tree: &sled::Tree,
        seq: &AtomicU64,
        buffer: &RefCell<Vec<u8>>,
        data: &[u8],
    ) {
        let mut buffer = buffer.borrow_mut();
        buffer.extend_from_slice(data);
        while buffer.len() >= PAGE_SIZE {
            let page: Vec<u8> = buffer.drain(..PAGE_SIZE).collect();
            let seq = seq.fetch_add(1, Ordering::Relaxed);
            let _ = tree.insert(seq.to_be_bytes(), page);
        }
    }

    /// Append bytes to the stdout buffer, flushing full pages to sled.
    pub fn write(&self, data: &[u8]) {
        Self::write_buffer(&self.tree, &self.seq, &self.buffer, data);
    }

    pub fn write_stderr(&self, data: &[u8]) {
        if let Some(tree) = &self.stderr_tree {
            Self::write_buffer(tree, &self.stderr_seq, &self.stderr_buffer, data);
        } else {
            self.write(data);
        }
    }

    pub fn separates_stderr(&self) -> bool {
        self.stderr_tree.is_some()
    }

    fn flush_buffer(tree: &sled::Tree, seq: &AtomicU64, buffer: &RefCell<Vec<u8>>) {
        let mut buffer = buffer.borrow_mut();
        if !buffer.is_empty() {
            let page: Vec<u8> = buffer.drain(..).collect();
            let seq = seq.fetch_add(1, Ordering::Relaxed);
            let _ = tree.insert(seq.to_be_bytes(), page);
        }
    }

    /// Flush any remaining buffered bytes to sled (call on execution end).
    pub fn flush(&self) {
        Self::flush_buffer(&self.tree, &self.seq, &self.buffer);
        if let Some(tree) = &self.stderr_tree {
            Self::flush_buffer(tree, &self.stderr_seq, &self.stderr_buffer);
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

    if console_state.separates_stderr() {
        let formatted = format!("{}\n", msg);
        if matches!(level, 2 | 3) {
            console_state.write_stderr(formatted.as_bytes());
        } else {
            console_state.write(formatted.as_bytes());
        }
        return;
    }

    let formatted = match level {
        2 => format!("[WARN] {}\n", msg),
        3 => format!("[ERROR] {}\n", msg),
        1 => format!("[INFO] {}\n", msg),
        _ => format!("{}\n", msg),
    };
    console_state.write(formatted.as_bytes());
}

#[op2]
#[string]
fn op_process_exec_path() -> String {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/usr/bin/node".to_owned())
}

#[op2(fast)]
fn op_process_set_exit_code(state: &mut OpState, #[smi] code: i32) {
    if let Some(exit_state) = state.try_borrow::<ProcessExitState>() {
        exit_state.set_exit_code(code);
    }
}

#[op2(fast)]
fn op_process_exit(
    scope: &mut v8::PinScope<'_, '_>,
    state: &mut OpState,
    #[smi] code: i32,
) {
    if let Some(exit_state) = state.try_borrow::<ProcessExitState>() {
        exit_state.request_exit(code);
        scope.terminate_execution();
    }
}

// ── Extension registration ───────────────────────────────────────────────

deno_core::extension!(
    console_ext,
    ops = [op_console_write, op_process_exec_path, op_process_set_exit_code, op_process_exit],
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
///
/// Shaped like the Console Standard's namespace object: prototype chain is
/// console -> (empty object) -> Object.prototype, @@toStringTag "console",
/// and the full method set. The inspector is depth- and length-capped so
/// logging huge arrays/typed arrays stays O(cap), not O(n).
const CONSOLE_JS_WRAPPER: &str = r#"
(function() {
    var MAX_ITEMS = 100;      // array/map/set elements shown
    var MAX_PROPS = 50;       // object properties shown
    var MAX_DEPTH = 4;
    var MAX_STRING = 10000;   // per-string cap inside structures

    function inspect(value, depth, seen) {
        switch (typeof value) {
            case 'undefined': return 'undefined';
            case 'boolean': return String(value);
            case 'number':
                return Object.is(value, -0) ? '-0' : String(value);
            case 'bigint': return String(value) + 'n';
            case 'symbol': return value.toString();
            case 'function': {
                var name = value.name ? ': ' + value.name : ' (anonymous)';
                return '[Function' + name + ']';
            }
            case 'string':
                if (depth === 0) return value;
                return "'" + (value.length > MAX_STRING
                    ? value.slice(0, MAX_STRING) + '...' : value) + "'";
        }
        if (value === null) return 'null';
        if (seen.indexOf(value) !== -1) return '[Circular]';
        if (depth > MAX_DEPTH) return '[Object]';
        seen = seen.concat([value]);
        var next = depth + 1;
        try {
            if (Array.isArray(value)) {
                var out = [];
                var n = Math.min(value.length, MAX_ITEMS);
                for (var i = 0; i < n; i++) {
                    out.push(i in value ? inspect(value[i], next, seen) : '<empty>');
                }
                if (value.length > n) out.push('... ' + (value.length - n) + ' more items');
                return '[ ' + out.join(', ') + ' ]';
            }
            if (ArrayBuffer.isView(value) && !(value instanceof DataView)) {
                var tag = value.constructor && value.constructor.name || 'TypedArray';
                var parts = [];
                var m = Math.min(value.length, MAX_ITEMS);
                for (var j = 0; j < m; j++) parts.push(String(value[j]));
                if (value.length > m) parts.push('... ' + (value.length - m) + ' more items');
                return tag + '(' + value.length + ') [ ' + parts.join(', ') + ' ]';
            }
            if (value instanceof Date) return value.toISOString();
            if (value instanceof RegExp) return value.toString();
            if (value instanceof Error) {
                return value.stack || (value.name + ': ' + value.message);
            }
            if (typeof Map === 'function' && value instanceof Map) {
                var me = [];
                var mi = 0;
                for (var entry of value) {
                    if (mi++ >= MAX_ITEMS) { me.push('...'); break; }
                    me.push(inspect(entry[0], next, seen) + ' => ' + inspect(entry[1], next, seen));
                }
                return 'Map(' + value.size + ') { ' + me.join(', ') + ' }';
            }
            if (typeof Set === 'function' && value instanceof Set) {
                var se = [];
                var si = 0;
                for (var sv of value) {
                    if (si++ >= MAX_ITEMS) { se.push('...'); break; }
                    se.push(inspect(sv, next, seen));
                }
                return 'Set(' + value.size + ') { ' + se.join(', ') + ' }';
            }
            if (value instanceof ArrayBuffer) {
                return 'ArrayBuffer { byteLength: ' + value.byteLength + ' }';
            }
            if (typeof Promise === 'function' && value instanceof Promise) {
                return 'Promise { }';
            }
            var keys = Object.keys(value);
            var props = [];
            var kn = Math.min(keys.length, MAX_PROPS);
            for (var k = 0; k < kn; k++) {
                var kv;
                try { kv = inspect(value[keys[k]], next, seen); }
                catch (e) { kv = '[Getter threw]'; }
                props.push(keys[k] + ': ' + kv);
            }
            if (keys.length > kn) props.push('... ' + (keys.length - kn) + ' more');
            var ctor = value.constructor && value.constructor.name;
            var prefix = ctor && ctor !== 'Object' ? ctor + ' ' : '';
            return prefix + '{ ' + props.join(', ') + ' }';
        } catch (e) {
            try { return String(value); } catch (_) { return '[Unrepresentable]'; }
        }
    }

    // Console Standard "Formatter": %s %d %i %f %o %O %c %% in a leading
    // format string consume subsequent args.
    function formatArgs(args) {
        var out = [];
        var start = 0;
        if (args.length > 0 && typeof args[0] === 'string' && /%[sdifoOc%]/.test(args[0])) {
            var fmt = args[0];
            var argIndex = 1;
            var result = '';
            for (var i = 0; i < fmt.length; i++) {
                if (fmt[i] === '%' && i + 1 < fmt.length) {
                    var c = fmt[i + 1];
                    if (c === '%') { result += '%'; i++; continue; }
                    if ('sdifoO'.indexOf(c) !== -1 && argIndex < args.length) {
                        var a = args[argIndex++];
                        if (c === 's') result += typeof a === 'string' ? a : inspect(a, 0, []);
                        else if (c === 'd' || c === 'i') {
                            result += typeof a === 'symbol' ? 'NaN' : String(parseInt(a, 10));
                        } else if (c === 'f') {
                            result += typeof a === 'symbol' ? 'NaN' : String(parseFloat(a));
                        } else result += inspect(a, 1, []);
                        i++;
                        continue;
                    }
                    if (c === 'c') { argIndex < args.length && argIndex++; i++; continue; }
                }
                result += fmt[i];
            }
            out.push(result);
            start = argIndex;
        }
        for (var j = start; j < args.length; j++) {
            out.push(inspect(args[j], 0, []));
        }
        return out.join(' ');
    }

    function write(level, args) {
        Deno.core.ops.op_console_write(formatArgs(Array.from(args)), level);
    }

    var counts = new Map();
    var timers = new Map();
    var groupDepth = 0;

    var methods = {
        log: function log() { write(0, arguments); },
        info: function info() { write(1, arguments); },
        warn: function warn() { write(2, arguments); },
        error: function error() { write(3, arguments); },
        debug: function debug() { write(0, arguments); },
        trace: function trace() { write(0, arguments); },
        dir: function dir() { write(0, arguments); },
        dirxml: function dirxml() { write(0, arguments); },
        table: function table() { write(0, arguments); },
        clear: function clear() {},
        group: function group() { if (arguments.length) write(0, arguments); groupDepth++; },
        groupCollapsed: function groupCollapsed() { if (arguments.length) write(0, arguments); groupDepth++; },
        groupEnd: function groupEnd() { if (groupDepth > 0) groupDepth--; },
        count: function count(label) {
            label = arguments.length === 0 ? 'default' : String(label);
            var n = (counts.get(label) || 0) + 1;
            counts.set(label, n);
            write(0, [label + ': ' + n]);
        },
        countReset: function countReset(label) {
            label = arguments.length === 0 ? 'default' : String(label);
            if (counts.has(label)) counts.set(label, 0);
            else write(2, ["Count for '" + label + "' does not exist"]);
        },
        assert: function assert(condition) {
            if (condition) return;
            var rest = Array.prototype.slice.call(arguments, 1);
            if (rest.length > 0 && typeof rest[0] === 'string') {
                rest[0] = 'Assertion failed: ' + rest[0];
            } else {
                rest.unshift('Assertion failed');
            }
            write(3, rest);
        },
        time: function time(label) {
            label = arguments.length === 0 ? 'default' : String(label);
            if (timers.has(label)) {
                write(2, ["Timer '" + label + "' already exists"]);
                return;
            }
            timers.set(label, Date.now());
        },
        timeLog: function timeLog(label) {
            label = arguments.length === 0 ? 'default' : String(label);
            if (!timers.has(label)) {
                write(2, ["Timer '" + label + "' does not exist"]);
                return;
            }
            var rest = Array.prototype.slice.call(arguments, 1);
            write(0, [label + ': ' + (Date.now() - timers.get(label)) + 'ms']
                .concat(rest.map(function (a) { return inspect(a, 0, []); })));
        },
        timeEnd: function timeEnd(label) {
            label = arguments.length === 0 ? 'default' : String(label);
            if (!timers.has(label)) {
                write(2, ["Timer '" + label + "' does not exist"]);
                return;
            }
            write(0, [label + ': ' + (Date.now() - timers.get(label)) + 'ms']);
            timers.delete(label);
        },
    };

    // Namespace-object shape: console -> {} -> Object.prototype, with
    // @@toStringTag "console" so Object.prototype.toString says
    // "[object console]".
    var consoleProto = Object.create(Object.prototype);
    var consoleObj = Object.create(consoleProto);
    for (var name in methods) {
        Object.defineProperty(consoleObj, name, {
            value: methods[name],
            writable: true,
            enumerable: true,
            configurable: true,
        });
    }
    Object.defineProperty(consoleObj, Symbol.toStringTag, {
        value: 'console',
        writable: false,
        enumerable: false,
        configurable: true,
    });
    globalThis.console = consoleObj;
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
/// Each mitigation is **opt-in** and OFF by default: with a default
/// `HardeningConfig` this is a no-op and the runtime is left unhardened. Enable
/// mitigations individually via the `--harden-*` CLI flags. Whatever is enabled
/// is applied in an order that keeps op-neutralization before the
/// `Object.freeze` that would otherwise lock those ops in place.
///
/// Mitigations (each gated by its `HardeningConfig` field):
/// - `neutralize_proxy_details`: `op_get_proxy_details` → `undefined` (else it bypasses `Proxy` handlers)
/// - `neutralize_introspection`: `op_memory_usage`/`op_is_terminal` neutralized (host info leaks)
/// - `freeze_ops`: `Object.freeze(Deno.core.ops)` (no op interception/replacement)
/// - `remove_bootstrap`: delete `__bootstrap` (event-loop hooks, primordials, internals)
/// - `remove_shared_memory`: delete `SharedArrayBuffer`/`Atomics` (Spectre timer prerequisite; also the primitives emscripten wasm-threads need)
pub fn harden_runtime(runtime: &mut JsRuntime, config: HardeningConfig) -> Result<(), String> {
    if config.is_noop() {
        return Ok(());
    }
    let mut js = String::from("(function() {\n");
    // Op neutralization must run BEFORE Object.freeze (freeze locks the ops object).
    if config.neutralize_proxy_details {
        js.push_str("  Deno.core.ops.op_get_proxy_details = function() { return undefined; };\n");
    }
    if config.neutralize_introspection {
        js.push_str("  Deno.core.ops.op_memory_usage = function() { return {}; };\n");
        js.push_str("  Deno.core.ops.op_is_terminal = function() { return false; };\n");
    }
    if config.freeze_ops {
        js.push_str("  Object.freeze(Deno.core.ops);\n");
    }
    if config.remove_bootstrap {
        // __bootstrap exposes event-loop hooks (setMacrotaskCallback,
        // setPromiseHooks, …), primordials (pristine Function constructor), and
        // internal registration objects. deno_core's own bootstrap has already
        // completed, so deleting it here is safe.
        js.push_str("  delete globalThis.__bootstrap;\n");
    }
    if config.remove_shared_memory {
        // SharedArrayBuffer is the prerequisite for a high-resolution Spectre
        // timer (and for emscripten wasm-threads). V8 flags cannot disable it
        // (stable spec feature), so remove from JS.
        js.push_str("  delete globalThis.SharedArrayBuffer;\n");
        js.push_str("  delete globalThis.Atomics;\n");
    }
    js.push_str("})();");
    runtime
        .execute_script("<sandbox-hardening>", js)
        .map_err(|e| format!("Failed to harden sandbox: {}", e))?;
    Ok(())
}

/// Per-mitigation sandbox-hardening switches. All fields default to `false`
/// (OFF) — mcp-v8 runs UNHARDENED unless mitigations are explicitly enabled (see
/// the `--harden-*` CLI flags). Each field maps to one mitigation from the
/// original combined hardening pass (commit a1d644d).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HardeningConfig {
    /// Freeze `Deno.core.ops` so no op can be replaced/intercepted (e.g. a
    /// persistent trojan op surviving in stateful/snapshot mode).
    pub freeze_ops: bool,
    /// Neutralize `op_get_proxy_details` (otherwise it bypasses `Proxy` handlers
    /// and can read a proxied target).
    pub neutralize_proxy_details: bool,
    /// Neutralize `op_memory_usage` + `op_is_terminal` (host info leaks).
    pub neutralize_introspection: bool,
    /// Remove `globalThis.__bootstrap` (event-loop hooks, primordials such as a
    /// pristine `Function` constructor, and internal registries).
    pub remove_bootstrap: bool,
    /// Remove `globalThis.SharedArrayBuffer` + `globalThis.Atomics` — the
    /// high-resolution Spectre-timer prerequisite (and the shared-memory
    /// primitives emscripten wasm-threads require).
    pub remove_shared_memory: bool,
}

impl HardeningConfig {
    /// Every mitigation enabled (the original combined hardening behavior).
    pub fn all() -> Self {
        Self {
            freeze_ops: true,
            neutralize_proxy_details: true,
            neutralize_introspection: true,
            remove_bootstrap: true,
            remove_shared_memory: true,
        }
    }

    /// True when no mitigation is enabled — `harden_runtime` is a no-op.
    pub fn is_noop(&self) -> bool {
        *self == Self::default()
    }
}

// ── Base64 globals (atob / btoa) ────────────────────────────────────────

pub fn inject_base64(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<base64-setup>", BASE64_JS.to_string())
        .map_err(|e| format!("Failed to install atob/btoa: {}", e))?;
    Ok(())
}

pub fn inject_base64_snapshot(runtime: &mut deno_core::JsRuntimeForSnapshot) -> Result<(), String> {
    runtime
        .execute_script("<base64-setup>", BASE64_JS.to_string())
        .map_err(|e| format!("Failed to install atob/btoa: {}", e))?;
    Ok(())
}

const BASE64_JS: &str = r#"
(function() {
    var chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';

    // DOMException is installed later in the injection sequence, so look
    // it up at throw time; fall back to a local error type if absent.
    function InvalidCharacterError(message) {
        if (typeof globalThis.DOMException === 'function') {
            return new globalThis.DOMException(message, 'InvalidCharacterError');
        }
        this.name = 'InvalidCharacterError';
        this.message = message;
    }
    InvalidCharacterError.prototype = Object.create(Error.prototype);
    InvalidCharacterError.prototype.constructor = InvalidCharacterError;

    globalThis.btoa = function btoa(input) {
        var str = String(input);
        for (var i = 0; i < str.length; i++) {
            if (str.charCodeAt(i) > 255) {
                throw new InvalidCharacterError(
                    "The string to be encoded contains characters outside of the Latin1 range."
                );
            }
        }
        var out = '';
        for (var i = 0; i < str.length; i += 3) {
            var a = str.charCodeAt(i);
            var b = i + 1 < str.length ? str.charCodeAt(i + 1) : 0;
            var c = i + 2 < str.length ? str.charCodeAt(i + 2) : 0;
            out += chars[a >> 2];
            out += chars[((a & 3) << 4) | (b >> 4)];
            out += i + 1 < str.length ? chars[((b & 15) << 2) | (c >> 6)] : '=';
            out += i + 2 < str.length ? chars[c & 63] : '=';
        }
        return out;
    };

    globalThis.atob = function atob(input) {
        // Forgiving-base64 decode (WHATWG Infra): strip ASCII whitespace;
        // when the length is a multiple of four, up to two trailing '='
        // may be removed; any other '=' or a length % 4 of 1 is an error.
        var str = String(input).replace(/[\t\n\f\r ]/g, '');
        if (str.length % 4 === 0) {
            str = str.replace(/={1,2}$/, '');
        }
        if (str.length % 4 === 1) {
            throw new InvalidCharacterError(
                "The string to be decoded is not correctly encoded."
            );
        }
        var out = '';
        var buf = 0, bits = 0;
        for (var i = 0; i < str.length; i++) {
            var idx = chars.indexOf(str[i]);
            if (idx === -1) {
                throw new InvalidCharacterError(
                    "The string to be decoded contains invalid characters."
                );
            }
            buf = (buf << 6) | idx;
            bits += 6;
            if (bits >= 8) {
                bits -= 8;
                out += String.fromCharCode((buf >> bits) & 0xff);
            }
        }
        return out;
    };
})();
"#;

// ── Blob / File / FormData globals ──────────────────────────────────────

pub fn inject_web_apis(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<web-apis-setup>", WEB_APIS_JS.to_string())
        .map_err(|e| format!("Failed to install Blob/File/FormData: {}", e))?;
    Ok(())
}

pub fn inject_web_apis_snapshot(runtime: &mut deno_core::JsRuntimeForSnapshot) -> Result<(), String> {
    runtime
        .execute_script("<web-apis-setup>", WEB_APIS_JS.to_string())
        .map_err(|e| format!("Failed to install Blob/File/FormData: {}", e))?;
    Ok(())
}

const WEB_APIS_JS: &str = r#"
(function() {
    // TextEncoder / TextDecoder are installed by the web_compat encoding
    // layer (src/engine/web_compat/encoding.js), which owns the full WHATWG
    // label table via encoding_rs ops.

    // A Blob's bytes live in `_data` as a latin1 byte-string: one code unit per
    // byte, each in 0x00-0xFF. `arrayBuffer()`/`bytes()` read it back with
    // `charCodeAt(i) & 0xff` and FormData._serialize splices it in verbatim, so
    // every producer must hand over bytes in that form. Strings are UTF-8
    // encoded on the way in (per the WHATWG constructor) and BufferSource parts
    // are copied byte-for-byte — a Blob built from binary must survive intact,
    // which is why parts are never stringified.
    const _latin1FromBytes = function(bytes) {
        let out = '';
        const CHUNK = 0x8000;  // stay under String.fromCharCode's argument limit
        for (let i = 0; i < bytes.length; i += CHUNK) {
            out += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
        }
        return out;
    };
    const _bytesFromLatin1 = function(data) {
        const out = new Uint8Array(data.length);
        for (let i = 0; i < data.length; i++) out[i] = data.charCodeAt(i) & 0xff;
        return out;
    };
    const _latin1FromPart = function(part) {
        if (part instanceof ArrayBuffer) return _latin1FromBytes(new Uint8Array(part));
        if (ArrayBuffer.isView(part)) {
            return _latin1FromBytes(new Uint8Array(part.buffer, part.byteOffset, part.byteLength));
        }
        return _latin1FromBytes(new TextEncoder().encode(String(part)));
    };
    // Build a Blob directly from bytes already in latin1 form, skipping the
    // constructor's encoding step (which would double-encode them).
    const _blobFromLatin1 = function(data, type) {
        const b = new Blob([], { type: type });
        b._data = data;
        b.size = data.length;
        return b;
    };

    globalThis.Blob = function Blob(parts, options) {
        const opt = options || {};
        this.type = opt.type || '';
        const chunks = [];
        for (const part of (parts || [])) {
            if (part instanceof Blob) {
                chunks.push(part._data);
            } else {
                chunks.push(_latin1FromPart(part));
            }
        }
        this._data = chunks.join('');
        this.size = this._data.length;
    };
    Blob.prototype.text = function() {
        return Promise.resolve(new TextDecoder().decode(_bytesFromLatin1(this._data)));
    };
    Blob.prototype.slice = function(start, end, contentType) {
        // `_data` is one code unit per byte, so a string slice is a byte slice.
        return _blobFromLatin1(this._data.slice(start, end), contentType || this.type);
    };
    Blob.prototype.arrayBuffer = function() {
        return Promise.resolve(_bytesFromLatin1(this._data).buffer);
    };
    Blob.prototype.bytes = function() {
        return Promise.resolve(_bytesFromLatin1(this._data));
    };

    globalThis.File = function File(parts, name, options) {
        Blob.call(this, parts, options);
        this.name = name;
        this.lastModified = (options && options.lastModified) || Date.now();
    };
    File.prototype = Object.create(Blob.prototype);
    File.prototype.constructor = File;

    globalThis.FormData = function FormData() {
        this._entries = [];
    };
    FormData.prototype.append = function(name, value, filename) {
        this._entries.push({ name: String(name), value: value, filename: filename });
    };
    FormData.prototype.set = function(name, value, filename) {
        const n = String(name);
        this._entries = this._entries.filter(function(e) { return e.name !== n; });
        this._entries.push({ name: n, value: value, filename: filename });
    };
    FormData.prototype.get = function(name) {
        const n = String(name);
        for (const e of this._entries) { if (e.name === n) return e.value; }
        return null;
    };
    FormData.prototype.getAll = function(name) {
        const n = String(name);
        return this._entries.filter(function(e) { return e.name === n; }).map(function(e) { return e.value; });
    };
    FormData.prototype.has = function(name) {
        const n = String(name);
        return this._entries.some(function(e) { return e.name === n; });
    };
    FormData.prototype.delete = function(name) {
        const n = String(name);
        this._entries = this._entries.filter(function(e) { return e.name !== n; });
    };
    FormData.prototype.entries = function() { return this._entries.map(function(e) { return [e.name, e.value]; }); };
    FormData.prototype.keys = function() { return this._entries.map(function(e) { return e.name; }); };
    FormData.prototype.values = function() { return this._entries.map(function(e) { return e.value; }); };
    FormData.prototype.forEach = function(cb) {
        for (const e of this._entries) { cb(e.value, e.name, this); }
    };
    // Returns `body` as a latin1 byte-string (see the Blob comment above), not
    // as text: file parts are spliced in from `Blob._data` byte-for-byte, so
    // UTF-8 encoding the result would corrupt every binary upload. Callers must
    // put it on the wire with `charCodeAt(i) & 0xff`, not a TextEncoder.
    FormData.prototype._serialize = function() {
        const boundary = '----FormData' + Math.random().toString(36).slice(2) + Date.now().toString(36);
        const parts = [];
        for (const entry of this._entries) {
            let disposition = 'form-data; name="' + entry.name + '"';
            let contentType = null;
            let body;
            if (entry.value instanceof File) {
                const fn = entry.filename || entry.value.name || 'blob';
                disposition += '; filename="' + fn + '"';
                contentType = entry.value.type || 'application/octet-stream';
                body = entry.value._data;
            } else if (entry.value instanceof Blob) {
                const fn = entry.filename || 'blob';
                disposition += '; filename="' + fn + '"';
                contentType = entry.value.type || 'application/octet-stream';
                body = entry.value._data;
            } else {
                body = _latin1FromPart(String(entry.value));
            }
            let head = '--' + boundary + '\r\nContent-Disposition: ' + disposition + '\r\n';
            if (contentType) head += 'Content-Type: ' + contentType + '\r\n';
            head += '\r\n';
            parts.push(_latin1FromPart(head) + body + '\r\n');
        }
        parts.push('--' + boundary + '--\r\n');
        return { boundary: boundary, body: parts.join('') };
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
