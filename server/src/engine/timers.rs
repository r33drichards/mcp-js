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

/// JavaScript wrapper that provides `setTimeout` and `clearTimeout`.
///
/// The async op `Deno.core.ops.op_timer_sleep(delay_ms)` returns a
/// Promise<void> that resolves after the delay. The wrapper manages timer
/// IDs and cancellation tokens on the JS side.
const TIMERS_JS_WRAPPER: &str = r#"
(function() {
    var _sleep = Deno.core.ops.op_timer_sleep;
    var _nextId = 1;
    var _active = new Map();

    globalThis.setTimeout = function setTimeout(callback, delay) {
        if (typeof callback !== 'function') {
            throw new TypeError('setTimeout: callback must be a function');
        }
        var ms = Math.max(0, Number(delay) || 0);
        var id = _nextId++;
        var token = { cancelled: false };
        _active.set(id, token);

        _sleep(ms).then(function() {
            _active.delete(id);
            if (!token.cancelled) {
                callback();
            }
        });

        return id;
    };

    globalThis.clearTimeout = function clearTimeout(id) {
        var token = _active.get(id);
        if (token) {
            token.cancelled = true;
            _active.delete(id);
        }
    };
})();
"#;
