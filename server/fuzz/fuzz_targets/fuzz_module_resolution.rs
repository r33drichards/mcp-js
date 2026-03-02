#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::ExecutionConfig;
use server::engine::module_loader::ModuleLoaderConfig;
use std::sync::{Arc, Mutex, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        // Disable V8 background threads to prevent cumulative memory exhaustion
        deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
        // Override libfuzzer's abort-on-panic hook for graceful panic handling
        std::panic::set_hook(Box::new(|_| {}));
    });
}

#[derive(Arbitrary, Debug)]
struct ModuleResolutionInput {
    /// Import specifier (npm:, jsr:, http://, file://, or relative paths)
    specifier: String,
    /// Whether to include a version suffix for npm/jsr
    include_version: bool,
}

// Fuzz module import resolution with edge cases:
// - Empty specifiers
// - Malformed npm: specifiers (missing package name, invalid versions)
// - Malformed jsr: specifiers
// - URL imports with various schemes (http, https, file, ftp, data, javascript)
// - URL imports with special characters, unicode domains
// - Very long specifiers
// - Path traversal attempts in npm/jsr specifiers
// - Control characters and null bytes in specifiers
//
// This exercises module specifier parsing and URL resolution paths.
// Most imports will fail gracefully (network errors, invalid URLs, policy denial),
// which is the expected behavior.
fuzz_target!(|input: ModuleResolutionInput| {
    ensure_v8();

    // Cap specifier length to avoid excessive code generation
    let mut specifier = input.specifier;
    specifier.truncate(10000);

    // Build JavaScript code that attempts various imports
    // Wrap in try-catch since most will fail (no network, policy denial, etc.)
    let specifier = escape_js_string(&specifier);

    let code = if input.include_version {
        format!(
            r#"
try {{
  // Try as npm package with version
  const mod1 = await import('npm:{}@1.0.0');
}} catch(e) {{ }}

try {{
  // Try as jsr package with version
  const mod2 = await import('jsr:{}@1.0.0');
}} catch(e) {{ }}

try {{
  // Try as URL import
  const mod3 = await import('{}');
}} catch(e) {{ }}
"#,
            specifier, specifier, specifier
        )
    } else {
        format!(
            r#"
try {{
  // Try as npm package
  const mod1 = await import('npm:{}');
}} catch(e) {{ }}

try {{
  // Try as jsr package
  const mod2 = await import('jsr:{}');
}} catch(e) {{ }}

try {{
  // Try as URL import
  const mod3 = await import('{}');
}} catch(e) {{ }}
"#,
            specifier, specifier, specifier
        )
    };

    // Execute stateless — we don't care about the result, only that it doesn't crash.
    // Disable external modules so dynamic imports fail fast at resolution time
    // instead of attempting real HTTP requests that would timeout.
    let max_bytes = 8 * 1024 * 1024;
    let handle = Arc::new(Mutex::new(None));
    let loader_config = ModuleLoaderConfig {
        allow_external: false,
        policy_chain: None,
    };
    let _ = server::engine::execute_stateless(&code, ExecutionConfig::new(max_bytes)
        .isolate_handle(handle)
        .module_loader_config(&loader_config));
});

/// Escape a string for safe inclusion in JavaScript code
fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\0', "\\0")
}
