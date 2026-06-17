//! Console output capture for the JavaScript runtime.
//!
//! Intercepts `console.log`, `console.info`, `console.warn`, and `console.error`
//! calls and streams the output as a byte stream into a sled tree. Writes are
//! buffered in-memory and flushed to sled in fixed-size pages (WAL-style) for
//! efficient batching.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use deno_core::{JsRuntime, OpState, op2};


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


/// Sync op: writes formatted console output bytes into the buffered WAL.
/// Called from JS via `Deno.core.ops.op_console_write(msg, level)`.
/// level: 0=log, 1=info, 2=warn, 3=error

fn op_console_write(state: &mut OpState,  msg: &str,  level: i32) {
    let console_state = state.borrow::<ConsoleLogState>();

    let formatted = match level {
        2 => format!("[WARN] {}\n", msg),
        3 => format!("[ERROR] {}\n", msg),
        1 => format!("[INFO] {}\n", msg),
        _ => format!("{}\n", msg),
    };

    console_state.write(formatted.as_bytes());
}


deno_core::extension!(
    console_ext,
    ops = [op_console_write],
);

/// Create the console extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    console_ext::init()
}


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
                                        js.push_str("  delete globalThis.__bootstrap;\n");
    }
    if config.remove_shared_memory {
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

    function InvalidCharacterError(message) {
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
        var str = String(input).replace(/[\t\n\f\r ]/g, '');
        if (str.length % 4 === 1) {
            throw new InvalidCharacterError(
                "The string to be decoded is not correctly encoded."
            );
        }
        var out = '';
        var buf = 0, bits = 0;
        for (var i = 0; i < str.length; i++) {
            if (str[i] === '=') break;
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
    globalThis.Blob = function Blob(parts, options) {
        const opt = options || {};
        this.type = opt.type || '';
        const chunks = [];
        for (const part of (parts || [])) {
            if (part instanceof Blob) {
                chunks.push(part._data);
            } else if (typeof part === 'string') {
                chunks.push(part);
            } else {
                chunks.push(String(part));
            }
        }
        this._data = chunks.join('');
        this.size = this._data.length;
    };
    Blob.prototype.text = function() { return Promise.resolve(this._data); };
    Blob.prototype.slice = function(start, end, contentType) {
        const s = this._data.slice(start, end);
        return new Blob([s], { type: contentType || this.type });
    };
    Blob.prototype.arrayBuffer = function() {
        const buf = new ArrayBuffer(this._data.length);
        const view = new Uint8Array(buf);
        for (let i = 0; i < this._data.length; i++) view[i] = this._data.charCodeAt(i) & 0xff;
        return Promise.resolve(buf);
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
                body = String(entry.value);
            }
            let part = '--' + boundary + '\r\nContent-Disposition: ' + disposition + '\r\n';
            if (contentType) part += 'Content-Type: ' + contentType + '\r\n';
            part += '\r\n' + body + '\r\n';
            parts.push(part);
        }
        parts.push('--' + boundary + '--\r\n');
        return { boundary: boundary, body: parts.join('') };
    };
})();
"#;


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
