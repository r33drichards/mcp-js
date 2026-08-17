//! Timer APIs (`setTimeout`, `clearTimeout`) for the JavaScript runtime.
//!
//! Provides a single async op (`op_timer_sleep`) that sleeps for a given
//! number of milliseconds using `tokio::time::sleep`. The JS wrapper builds
//! `setTimeout` / `clearTimeout` on top of this op.

use deno_core::{JsRuntime, op2};
use deno_error::JsErrorBox;

// ── Async deno_core op ──────────────────────────────────────────────────

/// Async op: sleeps for `delay_ms` milliseconds. Called from JS via
/// `Deno.core.ops.op_timer_sleep(delay_ms)`.
/// Returns a Promise that resolves after the delay.
#[op2(async)]
async fn op_timer_sleep(#[number] delay_ms: u64) -> Result<(), JsErrorBox> {
    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    Ok(())
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    timers_ext,
    ops = [op_timer_sleep],
);

/// Create the timers extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    timers_ext::init()
}

// ── Inject timer JS wrappers into the global scope ──────────────────────

/// Inject `globalThis.setTimeout` and `globalThis.clearTimeout` JS wrappers.
/// Must be called after the runtime is created (with the timers extension)
/// but before user code runs and before sandbox hardening (which freezes ops).
pub fn inject_timers(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<timers-setup>", TIMERS_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install timers wrapper: {}", e))?;
    Ok(())
}

/// JavaScript wrapper that provides the HTML spec timer API:
/// `setTimeout` / `clearTimeout` / `setInterval` / `clearInterval`.
///
/// The async op `Deno.core.ops.op_timer_sleep(delay_ms)` returns a
/// Promise<void> that resolves after the delay. The wrapper manages timer
/// IDs, cancellation, and the spec's argument-conversion quirks on the JS
/// side:
/// - the timeout is converted per WebIDL `long` (ToNumber then ToInt32, so
///   2^32 wraps to 0), and negative values clamp to 0;
/// - non-function handlers are converted to strings at call time and
///   compiled/run in global scope at fire time (indirect eval);
/// - extra arguments are passed to the handler;
/// - nested timers deeper than 5 levels clamp to a 4ms minimum;
/// - `clearTimeout` and `clearInterval` share one ID space.
const TIMERS_JS_WRAPPER: &str = r#"
(function() {
    var _sleep = Deno.core.ops.op_timer_sleep;
    var _unrefOp = Deno.core.unrefOpPromise;
    var _refOp = Deno.core.refOpPromise;
    var _geval = eval; // indirect eval: runs in global scope
    var _nextId = 1;
    var _active = new Map();
    var _nestingLevel = 0;

    // Node's Timeout handle. Returned by setTimeout/setInterval so packages
    // written for Node can call .unref() — without it a library's background
    // timer (e.g. @grpc/grpc-js's 30-minute channel idle timer) keeps the
    // execution's event loop alive until it fires. Coerces to its numeric id
    // so browser-style `clearTimeout(id)` and arithmetic still work.
    function Timeout(id, timer) {
        this._id = id;
        this._timer = timer;
    }
    Timeout.prototype.unref = function unref() {
        this._timer.refed = false;
        if (this._timer.promise && _unrefOp) _unrefOp(this._timer.promise);
        return this;
    };
    Timeout.prototype.ref = function ref() {
        this._timer.refed = true;
        if (this._timer.promise && _refOp) _refOp(this._timer.promise);
        return this;
    };
    Timeout.prototype.hasRef = function hasRef() { return this._timer.refed; };
    Timeout.prototype.refresh = function refresh() { return this; };
    Timeout.prototype.valueOf = function valueOf() { return this._id; };
    Timeout.prototype.toString = function toString() { return String(this._id); };
    Timeout.prototype[Symbol.toPrimitive] = function (hint) {
        return hint === 'string' ? String(this._id) : this._id;
    };

    function scheduleTimer(handler, timeout, args, repeat) {
        var handlerFn, code;
        if (typeof handler === 'function') {
            handlerFn = handler;
        } else {
            // WebIDL: conversion to DOMString happens at call time.
            code = String(handler);
            args = [];
        }
        // WebIDL long: ToNumber then ToInt32 (modulo-wraps 2^32 to 0; NaN to 0).
        var ms = Number(timeout) | 0;
        if (ms < 0) ms = 0;

        var id = _nextId++;
        var timer = { cancelled: false, nesting: _nestingLevel + 1, refed: true, promise: null };
        _active.set(id, timer);

        function run(delay) {
            if (timer.nesting > 5 && delay < 4) delay = 4;
            var pending = _sleep(delay);
            timer.promise = pending;
            // An unref'd repeating timer must stay unref'd across ticks.
            if (!timer.refed && _unrefOp) _unrefOp(pending);
            pending.then(function () {
                if (timer.cancelled) return;
                var prevNesting = _nestingLevel;
                _nestingLevel = timer.nesting;
                try {
                    if (handlerFn) {
                        handlerFn.apply(globalThis, args);
                    } else {
                        _geval(code);
                    }
                } finally {
                    _nestingLevel = prevNesting;
                }
                if (repeat && !timer.cancelled) {
                    timer.nesting++;
                    run(ms);
                } else {
                    _active.delete(id);
                }
            });
        }
        run(ms);
        return new Timeout(id, timer);
    }

    function clearTimer(id) {
        var timer = _active.get(id);
        if (timer) {
            timer.cancelled = true;
            _active.delete(id);
        }
    }

    globalThis.setTimeout = function setTimeout(handler, timeout) {
        if (arguments.length < 1) {
            throw new TypeError(
                "Failed to execute 'setTimeout': 1 argument required, but only 0 present.");
        }
        return scheduleTimer(handler, timeout, Array.prototype.slice.call(arguments, 2), false);
    };

    globalThis.setInterval = function setInterval(handler, timeout) {
        if (arguments.length < 1) {
            throw new TypeError(
                "Failed to execute 'setInterval': 1 argument required, but only 0 present.");
        }
        return scheduleTimer(handler, timeout, Array.prototype.slice.call(arguments, 2), true);
    };

    // The spec gives clearTimeout and clearInterval one shared ID space.
    // Accepts a Timeout handle (coerced via valueOf) or a numeric id.
    globalThis.clearTimeout = function clearTimeout(id) { clearTimer(Number(id) | 0); };
    globalThis.clearInterval = function clearInterval(id) { clearTimer(Number(id) | 0); };

    // Node-flavored extras (used by npm packages targeting Node, e.g.
    // @grpc/grpc-js). setImmediate shares the timer ID space; queueMicrotask
    // is only defined when the runtime doesn't already provide it.
    globalThis.setImmediate = function setImmediate(handler) {
        return scheduleTimer(handler, 0, Array.prototype.slice.call(arguments, 1), false);
    };
    globalThis.clearImmediate = function clearImmediate(id) { clearTimer(Number(id) | 0); };
    if (typeof globalThis.queueMicrotask !== 'function') {
        globalThis.queueMicrotask = function queueMicrotask(callback) {
            if (typeof callback !== 'function') {
                throw new TypeError('queueMicrotask: callback must be a function');
            }
            Promise.resolve().then(function () { callback(); });
        };
    }
})();
"#;
