//! Web-platform compatibility layer (WinterTC Minimum Common API).
//!
//! Injects pure-JS implementations of the DOM/HTML plumbing that
//! deno_core's bare runtime lacks: Event/EventTarget and subclasses,
//! DOMException, AbortController/AbortSignal, structuredClone,
//! MessageChannel/MessagePort, reportError, self, navigator, and
//! performance. Everything lives in the V8 heap (no ops), so it survives
//! heap-snapshot persistence; each file is guarded to be a no-op when
//! re-injected into a restored heap that already has the globals.
//!
//! Must be injected after `timers::inject_timers` (AbortSignal.timeout and
//! MessagePort delivery use setTimeout) and before sandbox hardening.

use deno_core::JsRuntime;

/// (script name, source) pairs, in dependency order.
const WEB_COMPAT_FILES: &[(&str, &str)] = &[
    ("<web-compat-events>", include_str!("web_compat/events.js")),
    (
        "<web-compat-structured-clone>",
        include_str!("web_compat/structured_clone.js"),
    ),
    ("<web-compat-globals>", include_str!("web_compat/globals.js")),
];

fn user_agent_prelude() -> String {
    format!(
        "globalThis.__mcpV8UserAgent = 'mcp-v8/{}';",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn inject_web_compat(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<web-compat-ua>", user_agent_prelude())
        .map_err(|e| format!("Failed to set user agent: {}", e))?;
    for (name, source) in WEB_COMPAT_FILES {
        runtime
            .execute_script(*name, source.to_string())
            .map_err(|e| format!("Failed to install {}: {}", name, e))?;
    }
    Ok(())
}

pub fn inject_web_compat_snapshot(
    runtime: &mut deno_core::JsRuntimeForSnapshot,
) -> Result<(), String> {
    runtime
        .execute_script("<web-compat-ua>", user_agent_prelude())
        .map_err(|e| format!("Failed to set user agent: {}", e))?;
    for (name, source) in WEB_COMPAT_FILES {
        runtime
            .execute_script(*name, source.to_string())
            .map_err(|e| format!("Failed to install {}: {}", name, e))?;
    }
    Ok(())
}
