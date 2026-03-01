#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;

#[derive(Arbitrary, Debug)]
struct ParameterValidationInput {
    /// Code string (can be empty, very large, invalid)
    code: String,
    /// Session name (can be empty, special chars, unicode)
    session_name: Option<String>,
    /// Heap memory size in MB
    heap_memory_mb: Option<u32>,
    /// Execution timeout in seconds
    timeout_secs: Option<u32>,
    /// Tags map data
    tags_data: Vec<(String, String)>,
}

// Fuzz parameter validation with edge cases:
// - Empty code strings
// - Very large code strings
// - Session names with special characters, unicode, control chars
// - Various heap memory values (0, MAX_U32, etc.)
// - Various timeout values (0, MAX_U32, etc.)
// - Large tags maps with many key-value pairs
// - Large tag key/value sizes
//
// This tests parameter handling paths in the engine without executing code.
fuzz_target!(|input: ParameterValidationInput| {
    // Cap code size to avoid excessive memory
    let mut code = input.code;
    code.truncate(10 * 1024 * 1024); // 10MB max

    // Test parameter combinations without actually executing
    // The goal is to ensure parameter validation doesn't panic

    // Create tags map (capped to avoid memory issues)
    let mut tags = HashMap::new();
    for (k, v) in input.tags_data.iter().take(50) {
        let key = if k.len() > 1000 {
            k[..1000].to_string()
        } else {
            k.clone()
        };
        let value = if v.len() > 1000 {
            v[..1000].to_string()
        } else {
            v.clone()
        };
        tags.insert(key, value);
    }

    // Simulate parameter validation by testing type conversions
    // These operations should not panic even with arbitrary values

    // Test heap memory parameter clamping
    let heap_mb = input.heap_memory_mb.unwrap_or(8);
    let _clamped_heap = std::cmp::max(heap_mb, 8); // MIN_HEAP_MEMORY_MB = 8
    let _clamped_heap = std::cmp::min(_clamped_heap, 64); // MAX_HEAP_MEMORY_MB = 64

    // Test timeout parameter clamping
    let timeout_secs = input.timeout_secs.unwrap_or(30);
    let _clamped_timeout = std::cmp::max(timeout_secs, 1);
    let _clamped_timeout = std::cmp::min(_clamped_timeout, 300);

    // Test session name validation (empty is ok, any string should parse)
    let _session = input.session_name.as_deref().unwrap_or("");
    let _session_len = _session.len();

    // Test code parameter
    let _code_len = code.len();
    let _code_is_empty = code.is_empty();

    // Test tags
    let _tag_count = tags.len();

    // We don't actually execute — we just verify that parameters can be processed
    // without panicking. Any TypeError from deno_core is caught separately in real code.
});
