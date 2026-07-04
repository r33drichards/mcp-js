//! Differential tests: byte-level model vs verbatim copies of the mcp-js
//! originals, so Lean-level findings transfer to the real server code.

use mcpjs_verify::model;

// ── Verbatim copies from r33drichards/mcp-js ────────────────────────────────

/// server/src/main.rs:983 (anyhow::Result -> Result<usize, String>)
fn parse_memory_size_original(s: &str) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Empty memory size".into());
    }
    let (num_str, multiplier) = match s.as_bytes().last() {
        Some(b'k' | b'K') => (&s[..s.len() - 1], 1024usize),
        Some(b'm' | b'M') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'g' | b'G') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    let num: usize = num_str
        .parse()
        .map_err(|_| format!("Invalid memory size: '{}'", s))?;
    num.checked_mul(multiplier)
        .ok_or_else(|| format!("Memory size overflow: '{}'", s))
}

/// server/src/main.rs:1001
fn validate_wasm_name_original(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty".into());
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return Err("bad first char".into());
    }
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '_' && c != '$' {
            return Err("bad char".into());
        }
    }
    Ok(())
}

/// Host-pattern half of HeaderRule::matches, server/src/engine/fetch.rs:172-181
fn host_matches_original(pattern_host: &str, request_host: &str) -> bool {
    let pattern = pattern_host.to_lowercase();
    let host = request_host.to_lowercase();

    if let Some(suffix) = pattern.strip_prefix('*') {
        // "*.github.com" matches "api.github.com" and "github.com"
        host == pattern[2..] || host.ends_with(suffix)
    } else {
        host == pattern
    }
}

// ── Differential checks ─────────────────────────────────────────────────────

#[test]
fn parse_memory_size_agrees() {
    let cases = [
        "", " ", "0", "1", "1024", "1k", "1K", "5m", "5M", "2g", "2G",
        " 64m ", "18446744073709551615", "18446744073709551616",
        "18014398509481984k", "18014398509481985k", "17179869184g",
        "17179869185g", "-1", "1.5g", "k", "1kk", "0x10", "1 k",
    ];
    for c in cases {
        let orig = parse_memory_size_original(c).ok();
        let model = model::parse_memory_size(c.as_bytes());
        assert_eq!(orig, model, "disagreement on {c:?}");
    }
}

#[test]
fn validate_wasm_name_agrees() {
    let cases = [
        "", "a", "_", "$", "1a", "a1", "foo_bar", "foo-bar", "é", "aé",
        "a b", "A9$_", "9", "$$", "_x1", "日本", "a日",
    ];
    for c in cases {
        let orig = validate_wasm_name_original(c).is_ok();
        let model = model::validate_wasm_name(c.as_bytes());
        assert_eq!(orig, model, "disagreement on {c:?}");
    }
}

#[test]
fn host_matches_agrees_on_len_ge_2_patterns() {
    let patterns = ["*.github.com", "*github.com", "github.com", "*.", "ab", "*x"];
    let hosts = [
        "github.com", "api.github.com", "evilgithub.com", "github.com.evil.io",
        "GITHUB.COM", "x", "", "a.b",
    ];
    for p in patterns {
        for h in hosts {
            let orig = host_matches_original(p, h);
            let model = model::host_matches(p.as_bytes(), h.as_bytes());
            assert_eq!(orig, model, "disagreement on pattern {p:?} host {h:?}");
        }
    }
}

#[test]
fn host_matches_star_panics_in_both() {
    // pattern "*" -> pattern[2..] is out of bounds in the original (str, len 1)
    // and in the model (byte slice, len 1).
    let orig = std::panic::catch_unwind(|| host_matches_original("*", "anything.com"));
    let model =
        std::panic::catch_unwind(|| model::host_matches(b"*", b"anything.com"));
    assert!(orig.is_err(), "original should panic on pattern \"*\"");
    assert!(model.is_err(), "model should panic on pattern \"*\"");
}
