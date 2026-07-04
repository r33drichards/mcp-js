//! Byte-level ports of mcp-js pure functions, restricted to the Rust subset
//! Aeneas supports (no traits, no closures, no iterators, index loops only).
//!
//! Semantics preserved from the originals, including panics:
//! - `host_matches` mirrors `HeaderRule::matches` (server/src/engine/fetch.rs:164),
//!   host-pattern part. The original slices `pattern[2..]` on a `str`; the port
//!   keeps the same out-of-bounds panic for patterns shorter than 2 bytes.
//! - `parse_memory_size` mirrors `parse_memory_size` (server/src/main.rs:983),
//!   with `anyhow::Result` replaced by `Option` (None = any error).
//! - `validate_wasm_name` mirrors `validate_wasm_name` (server/src/main.rs:1001),
//!   returning `bool` instead of `Result<(), _>`.

fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn ends_with(haystack: &[u8], suffix: &[u8]) -> bool {
    if suffix.len() > haystack.len() {
        return false;
    }
    let start = haystack.len() - suffix.len();
    let mut i = 0;
    while i < suffix.len() {
        if haystack[start + i] != suffix[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn to_ascii_lowercase(s: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if b'A' <= c && c <= b'Z' {
            out.push(c + 32);
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Port of the host-pattern half of `HeaderRule::matches` (fetch.rs:164-181):
///
/// ```ignore
/// let pattern = self.host.to_lowercase();
/// let host = request_host.to_lowercase();
/// if let Some(suffix) = pattern.strip_prefix('*') {
///     // "*.github.com" matches "api.github.com" and "github.com"
///     host == pattern[2..] || host.ends_with(suffix)
/// } else {
///     host == pattern
/// }
/// ```
///
/// NB: `&pattern[2..]` panics when `pattern.len() < 2` — preserved here.
pub fn host_matches(pattern_raw: &[u8], request_host: &[u8]) -> bool {
    let pattern = to_ascii_lowercase(pattern_raw);
    let host = to_ascii_lowercase(request_host);
    if !pattern.is_empty() && pattern[0] == b'*' {
        let suffix = &pattern[1..];
        let bare = &pattern[2..]; // panics if pattern.len() < 2, as in the original
        bytes_eq(&host, bare) || ends_with(&host, suffix)
    } else {
        bytes_eq(&host, &pattern)
    }
}

fn is_ascii_whitespace(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' || c == 0x0c
}

fn trim(s: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < s.len() && is_ascii_whitespace(s[start]) {
        start += 1;
    }
    let mut end = s.len();
    while end > start && is_ascii_whitespace(s[end - 1]) {
        end -= 1;
    }
    &s[start..end]
}

/// Digit-by-digit `usize` parse mirroring `str::parse::<usize>` (fails on
/// empty input, non-digits, and overflow).
fn parse_usize(s: &[u8]) -> Option<usize> {
    if s.is_empty() {
        return None;
    }
    let mut acc: usize = 0;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c < b'0' || c > b'9' {
            return None;
        }
        let d = (c - b'0') as usize;
        acc = match acc.checked_mul(10) {
            Some(v) => v,
            None => return None,
        };
        acc = match acc.checked_add(d) {
            Some(v) => v,
            None => return None,
        };
        i += 1;
    }
    Some(acc)
}

/// Port of `parse_memory_size` (main.rs:983-998).
pub fn parse_memory_size(s: &[u8]) -> Option<usize> {
    let s = trim(s);
    if s.is_empty() {
        return None; // "Empty memory size"
    }
    let last = s[s.len() - 1];
    let (num_end, multiplier): (usize, usize) = if last == b'k' || last == b'K' {
        (s.len() - 1, 1024)
    } else if last == b'm' || last == b'M' {
        (s.len() - 1, 1024 * 1024)
    } else if last == b'g' || last == b'G' {
        (s.len() - 1, 1024 * 1024 * 1024)
    } else {
        (s.len(), 1)
    };
    let num = match parse_usize(&s[0..num_end]) {
        Some(n) => n,
        None => return None, // "Invalid memory size"
    };
    num.checked_mul(multiplier) // None = "Memory size overflow"
}

fn is_ascii_alphabetic(c: u8) -> bool {
    (b'a' <= c && c <= b'z') || (b'A' <= c && c <= b'Z')
}

fn is_ascii_alphanumeric(c: u8) -> bool {
    is_ascii_alphabetic(c) || (b'0' <= c && c <= b'9')
}

/// Port of `validate_wasm_name` (main.rs:1001-1016): valid JS identifier,
/// ASCII letters/digits/underscore/dollar, must not start with a digit.
pub fn validate_wasm_name(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name[0];
    if !is_ascii_alphabetic(first) && first != b'_' && first != b'$' {
        return false;
    }
    let mut i = 1;
    while i < name.len() {
        let c = name[i];
        if !is_ascii_alphanumeric(c) && c != b'_' && c != b'$' {
            return false;
        }
        i += 1;
    }
    true
}
