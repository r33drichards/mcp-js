//! Regression tests derived from Check Point Research's *"When Agentic Glue
//! Melts"* (2026), which exploited Cloudflare workerd's "Code Mode" — the layer
//! that binds untrusted JavaScript to native host capabilities. mcp-v8 is the
//! same shape of system: one `run_js` tool exposing a V8 isolate whose host
//! capabilities (fetch, fs, subprocess, modules, MCP, heap snapshots) are the
//! "agentic glue."
//!
//! The article describes five weakness classes. This file, together with the
//! tests it cross-references, records where each one lands for mcp-v8:
//!
//! | # | Article weakness                          | mcp-v8 status | Where tested |
//! |---|-------------------------------------------|---------------|--------------|
//! | 1 | OOB read — URLPattern native count vs V8  | N/A           | see note (a) |
//! | 2 | UAF — zlib / HTMLRewriter native bindings | N/A           | see note (b) |
//! | 3 | SQL authorizer bypass (authz logic gap)   | APPLICABLE    | `engine::fetch` tests (`test_do_fetch_denies_direct_request_to_disallowed_host`, `test_do_fetch_redirect_to_denied_host_must_not_bypass_policy`) |
//! | 4 | Arbitrary deserialization of untrusted data | APPLICABLE  | this file — `snapshot_gate::*` |
//! | 5 | "The engine is not the whole boundary"    | APPLICABLE    | `server/tests/sandbox_ops.rs` + `glue_boundary::*` here |
//!
//! Notes:
//!   (a) The URLPattern OOB read is a memory-safety bug in workerd's C++ glue,
//!       where native code trusts a capture-group count it derives differently
//!       from V8. mcp-v8 is Rust and does not reimplement URLPattern; the
//!       nearest "native trusts a JS-supplied length" surfaces are the bounded
//!       ArrayBuffer allocator and the wasmparser resource validator, both of
//!       which validate lengths against a budget before allocating. Rust's
//!       bounds checks turn the residual risk into a panic, not an OOB read —
//!       and the engine already converts panics into recoverable errors
//!       (see `sandbox_ops::test_op_panic_*`).
//!   (b) The zlib / HTMLRewriter use-after-frees are dangling-pointer bugs in
//!       workerd's C++ reimplementations of those Node/Cloudflare APIs. mcp-v8
//!       exposes neither; Rust ownership rules out the "JS frees the buffer, a
//!       retained native pointer writes through it later" pattern entirely.
//!
//! So the two classes that genuinely apply to a memory-safe host are the
//! *logic* ones — an authorizer that doesn't cover every path to the protected
//! resource, and a deserializer fed bytes it was designed to trust. Those are
//! the ones exercised here and in the fetch tests.

use std::sync::Once;

use server::engine::{ExecutionConfig, HardeningConfig};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

/// A temp sled tree for capturing console output, mirroring `sandbox_ops.rs`.
fn console_tree() -> (sled::Tree, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-glue-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = sled::open(&tmp).expect("open sled db");
    let tree = db.open_tree("console").expect("open tree");
    (tree, tmp)
}

fn read_console(tree: &sled::Tree) -> String {
    let mut buf = Vec::new();
    for entry in tree.iter().flatten() {
        buf.extend_from_slice(&entry.1);
    }
    String::from_utf8_lossy(&buf).to_string()
}

// ── Article case #4: arbitrary deserialization of untrusted data ────────────
//
// The article's fifth bug fed attacker-controlled bytes into V8's
// structured-clone deserializer, a code path "designed for trusted, in-process
// data [that] receives attacker-controlled serialized objects."
//
// mcp-v8's equivalent deserializer is the V8 *heap snapshot* restore path. In
// stateful mode the isolate heap is serialized to a content-addressed blob and
// handed back to `JsRuntimeForSnapshot` on the next call. V8's
// `Snapshot::Initialize` calls `V8_Fatal`/`abort()` on malformed snapshot data
// — an unrecoverable process kill that Rust's `catch_unwind` cannot intercept.
//
// The defense is `engine::unwrap_snapshot`, an envelope check that runs BEFORE
// any bytes reach V8 (see `Engine::run_js`, which does `unwrap_snapshot(&data)?`
// on the stored blob before calling `execute_stateful`). The envelope is:
//
//     [b"MCPV8SNAP\0" (10)] [SHA-256 of payload (32)] [V8 snapshot payload]
//
// with a 100 KiB minimum payload. These tests lock that gate in place: the
// thing standing between a corrupted/adversarial stored blob and V8's aborting
// deserializer must keep rejecting everything that isn't a snapshot this server
// itself produced.
mod snapshot_gate {
    use super::*;

    const MAGIC: &[u8] = b"MCPV8SNAP\x00";
    const HEADER_LEN: usize = 10 + 32; // magic + SHA-256
    const MIN_PAYLOAD: usize = 100 * 1024;

    #[test]
    fn rejects_data_shorter_than_the_envelope_header() {
        // Fewer than 42 bytes can't even hold the magic + checksum.
        for len in [0usize, 1, 9, 41] {
            let data = vec![0u8; len];
            let err = server::engine::unwrap_snapshot(&data)
                .expect_err("sub-header input must be rejected");
            assert!(err.contains("too small"), "unexpected error for len {len}: {err}");
        }
    }

    #[test]
    fn rejects_wrong_magic_header() {
        // Header-length and payload-length gates pass, but the magic doesn't:
        // this is the first thing that stops "some other file" from being fed
        // to V8 as a snapshot.
        let mut data = vec![0u8; HEADER_LEN + MIN_PAYLOAD];
        data[..10].copy_from_slice(b"NOTASNAP!!");
        let err = server::engine::unwrap_snapshot(&data)
            .expect_err("wrong magic must be rejected");
        assert!(err.contains("magic"), "expected a magic-header error, got: {err}");
    }

    #[test]
    fn rejects_payload_smaller_than_a_real_snapshot() {
        // Correct magic, but the payload is far below the 100 KiB floor a real
        // V8 snapshot always exceeds. Rejecting undersized payloads is what
        // stops a fuzzer (or an attacker) from hand-assembling a "valid" tiny
        // envelope whose checksum they can trivially satisfy.
        let mut data = vec![0u8; HEADER_LEN + 64];
        data[..10].copy_from_slice(MAGIC);
        let err = server::engine::unwrap_snapshot(&data)
            .expect_err("undersized payload must be rejected");
        assert!(err.contains("too small"), "expected a size error, got: {err}");
    }

    #[test]
    fn rejects_checksum_mismatch() {
        // Correct magic and an over-floor payload, but the stored checksum
        // (all zeros) does not match SHA-256(payload). This is the integrity
        // gate against a corrupted stored blob.
        let mut data = vec![0u8; HEADER_LEN + MIN_PAYLOAD + 1];
        data[..10].copy_from_slice(MAGIC);
        // bytes [10, 42) stay zero → a checksum that cannot match a nonempty
        // (here, all-zero) payload's real SHA-256.
        let err = server::engine::unwrap_snapshot(&data)
            .expect_err("checksum mismatch must be rejected");
        assert!(err.contains("checksum"), "expected a checksum error, got: {err}");
    }

    /// The one legitimate path: a snapshot this server produced must survive a
    /// round-trip through the gate, and tampering with a single payload byte
    /// must be caught before the bytes could reach V8.
    ///
    /// This also documents the gate's trust model, which is exactly the
    /// article's warning: the checksum guards against *corruption*, not a
    /// motivated adversary (anyone who can craft the payload can also write its
    /// matching SHA-256). The gate is only a safe boundary because the `heap`
    /// argument to `run_js` is a content-addressed lookup *key*, not raw bytes
    /// — the server retrieves what it earlier wrote. If untrusted raw snapshot
    /// bytes ever became directly supplyable, this envelope would not be
    /// sufficient protection on its own.
    #[test]
    fn accepts_a_real_snapshot_and_catches_tampering() {
        ensure_v8();
        let heap_bytes = 16 * 1024 * 1024;

        // deno_core bakes the extension/op set into the snapshot, so the
        // creating and restoring configs must line up (same extensions). Both
        // calls carry a console tree so the console extension is present in the
        // snapshot and again on restore.
        let (create_tree, create_tmp) = console_tree();
        let (result, _oom) = server::engine::execute_stateful(
            "globalThis.__glue_marker = 41;",
            None,
            ExecutionConfig::new(heap_bytes).console_tree(create_tree),
        );
        let (_out, wrapped, _hash) = result.expect("stateful execution should produce a snapshot");
        let _ = std::fs::remove_dir_all(&create_tmp);

        assert!(
            wrapped.len() > HEADER_LEN + MIN_PAYLOAD,
            "a real snapshot should clear the size floor"
        );
        assert_eq!(&wrapped[..10], MAGIC, "server output must carry the magic header");

        // The gate accepts the server's own output and hands back the payload.
        let payload = server::engine::unwrap_snapshot(&wrapped)
            .expect("a genuine snapshot must pass the gate");
        assert_eq!(payload.len(), wrapped.len() - HEADER_LEN);

        // Restoring that payload is the legitimate stateful path: the marker
        // baked into the heap survives into the next execution.
        let (tree, tmp) = console_tree();
        let (restore, _oom) = server::engine::execute_stateful(
            "console.log('marker=' + globalThis.__glue_marker);",
            Some(payload),
            ExecutionConfig::new(heap_bytes).console_tree(tree.clone()),
        );
        assert!(restore.is_ok(), "restoring a valid snapshot should work: {restore:?}");
        assert!(
            read_console(&tree).contains("marker=41"),
            "restored heap should retain the pre-snapshot marker"
        );
        let _ = std::fs::remove_dir_all(&tmp);

        // Flip one payload byte: the checksum no longer matches, so the gate
        // rejects it and the corrupted bytes never reach V8's deserializer.
        let mut tampered = wrapped.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        let err = server::engine::unwrap_snapshot(&tampered)
            .expect_err("a tampered snapshot must be rejected before it reaches V8");
        assert!(err.contains("checksum"), "expected a checksum error, got: {err}");
    }
}

// ── Article case #5: "the engine is not the whole boundary" ─────────────────
//
// The article's thesis is that V8's isolate is not the security boundary — the
// native glue around it is. mcp-v8's glue is the deno_core op layer, and the
// dangerous built-ins it exposes (`op_panic`, `Deno.core.print`, the
// `Deno.core` object itself) are neutralized before user code runs. The
// exhaustive matrix lives in `server/tests/sandbox_ops.rs`; these two assert
// the security-critical invariant that the *always-on* neutralizations hold
// even with every opt-in `--harden-*` mitigation OFF (the default posture).
mod glue_boundary {
    use super::*;

    #[test]
    fn op_panic_is_a_js_error_not_a_process_abort_even_unhardened() {
        ensure_v8();
        // Default (unhardened) config: op_panic neutralization is not gated on
        // any --harden flag, so a hostile `op_panic` call must still surface as
        // a catchable JS error rather than aborting the host process.
        let (result, _oom) = server::engine::execute_stateless(
            r#"Deno.core.ops.op_panic("melt the glue")"#,
            ExecutionConfig::new(8 * 1024 * 1024),
        );
        let err = result.expect_err("op_panic should raise a JS error");
        assert!(err.contains("panic"), "error should mention the panic, got: {err}");
    }

    #[test]
    fn deno_core_prototype_is_severed_even_unhardened() {
        ensure_v8();
        // The SECURITY_FINDINGS "print prototype bypass" reached the original
        // native core via `Object.getPrototypeOf(Deno.core)`. That escape hatch
        // must be closed regardless of the opt-in hardening flags.
        let (tree, tmp) = console_tree();
        let (result, _oom) = server::engine::execute_stateless(
            r#"console.log("proto=" + (Object.getPrototypeOf(Deno.core) === null ? "null" : "reachable"));"#,
            ExecutionConfig::new(8 * 1024 * 1024).console_tree(tree.clone()),
        );
        assert!(result.is_ok(), "execution should succeed: {result:?}");
        assert!(
            read_console(&tree).contains("proto=null"),
            "Deno.core prototype chain must be severed"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// With the full opt-in mitigation set, the introspection glue the article
    /// leans on (a mutable op table, the `__bootstrap` internals, and
    /// `op_get_proxy_details`) is closed as defense in depth.
    #[test]
    fn opt_in_hardening_closes_the_introspection_glue() {
        ensure_v8();
        let (tree, tmp) = console_tree();
        let (result, _oom) = server::engine::execute_stateless(
            r#"
            console.log("ops_frozen=" + Object.isFrozen(Deno.core.ops));
            console.log("bootstrap=" + typeof globalThis.__bootstrap);
            "#,
            ExecutionConfig::new(8 * 1024 * 1024)
                .console_tree(tree.clone())
                .hardening(HardeningConfig::all()),
        );
        assert!(result.is_ok(), "execution should succeed: {result:?}");
        let out = read_console(&tree);
        assert!(out.contains("ops_frozen=true"), "op table should be frozen, got: {out}");
        assert!(out.contains("bootstrap=undefined"), "__bootstrap should be removed, got: {out}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
