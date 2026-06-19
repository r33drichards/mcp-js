//! End-to-end fs snapshot flow through the `Engine`: mount → write → push →
//! inspect the resulting snapshot, plus heap/fs handle independence.
//!
//! Uses the real `Engine::run_js().execute()` path (async execution on the
//! blocking pool), which is the only place async fs ops are correctly driven.
//! Stateless executions return an empty `result` (output goes to console), so
//! we assert on the durable artifact — the pushed manifest — by reading it back
//! out of the shared `FsStore`.

use std::sync::{Arc, Once};

use server::engine::execution::ExecutionRegistry;
use server::engine::fs::FsConfig;
use server::engine::fs_labels::LabelStore;
use server::engine::fs_mount::SessionMount;
use server::engine::fs_store::FsStore;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::{initialize_v8, parse_ca_hex, Engine};

static INIT: Once = Once::new();
fn ensure_v8() {
    INIT.call_once(initialize_v8);
}

fn tmp_dir(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "mcp-fs-e2e-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_str()
        .unwrap()
        .to_string()
}

struct Harness {
    engine: Engine,
    store: Arc<FsStore>,
}

fn build_harness() -> Harness {
    let registry = ExecutionRegistry::new(&tmp_dir("reg")).expect("registry");
    let fs_config = FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All)));
    let store = Arc::new(FsStore::in_memory());
    let engine = Engine::new_stateless(64 * 1024 * 1024, 30, 4)
        .with_fs_config(fs_config)
        .with_execution_registry(Arc::new(registry))
        .with_fs_snapshots(store.clone(), Arc::new(LabelStore::in_memory()));
    Harness { engine, store }
}

/// Read a file out of a pushed snapshot by its CA-id hex.
async fn read_snapshot_file(store: &FsStore, ca_hex: &str, path: &str) -> Vec<u8> {
    let id = blake3::Hash::from_bytes(parse_ca_hex(ca_hex).expect("valid ca hex"));
    let mount = SessionMount::pull(store.clone(), id).await.unwrap();
    mount.read(path.as_ref()).await.unwrap()
}

/// Submit code with an optional fs handle and wait for terminal status.
/// Returns the resulting fs CA id (hex), if any.
async fn run(engine: &Engine, code: &str, fs: Option<&str>) -> Option<String> {
    let exec_id = engine
        .run_js(code.to_string())
        .maybe_fs(fs.map(|s| s.to_string()))
        .execute()
        .await
        .expect("submit");

    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return info.fs,
                "failed" => panic!("execution failed: {:?}", info.error),
                "timed_out" => panic!("execution timed out"),
                _ => continue,
            }
        }
    }
    panic!("timeout waiting for execution");
}

#[tokio::test]
async fn mount_write_push_then_read_back_by_ca_id() {
    ensure_v8();
    let h = build_harness();

    // Mount the (new) label "main", write a file. The resulting fs CA id is
    // surfaced on completion without advancing the label.
    let ca1 = run(
        &h.engine,
        r#"(async () => {
            await fs.writeFile("/data/note.txt", "hello snapshot");
        })()"#,
        Some("main"),
    )
    .await
    .expect("first run should yield an fs CA id");

    // The write is durable in the content store.
    assert_eq!(
        read_snapshot_file(&h.store, &ca1, "data/note.txt").await,
        b"hello snapshot"
    );

    // Re-mounting that exact snapshot (detached, by CA id) and reading it works
    // end-to-end through the engine too.
    let ca2 = run(
        &h.engine,
        r#"(async () => {
            const c = await fs.readFile("/data/note.txt", "utf8");
            if (c !== "hello snapshot") throw new Error("readback mismatch: " + c);
            await fs.writeFile("/data/extra.txt", "more");
        })()"#,
        Some(&ca1),
    )
    .await
    .unwrap();
    assert_eq!(
        read_snapshot_file(&h.store, &ca2, "data/note.txt").await,
        b"hello snapshot"
    );
    assert_eq!(
        read_snapshot_file(&h.store, &ca2, "data/extra.txt").await,
        b"more"
    );
}

#[tokio::test]
async fn fs_handle_and_heap_handle_are_independent() {
    ensure_v8();
    let h = build_harness();

    // A base snapshot, then two divergent branches off it produce two distinct
    // fs CA ids — the fs handle moves independently of any heap (stateless).
    let base = run(
        &h.engine,
        r#"(async () => { await fs.writeFile("/a", "base"); })()"#,
        Some("data"),
    )
    .await
    .unwrap();

    let fs_a = run(
        &h.engine,
        r#"(async () => { await fs.writeFile("/a", "branch-A"); })()"#,
        Some(&base),
    )
    .await
    .unwrap();
    let fs_b = run(
        &h.engine,
        r#"(async () => { await fs.writeFile("/a", "branch-B"); })()"#,
        Some(&base),
    )
    .await
    .unwrap();

    assert_ne!(fs_a, fs_b, "divergent writes must yield distinct CA ids");
    assert_eq!(read_snapshot_file(&h.store, &fs_a, "a").await, b"branch-A");
    assert_eq!(read_snapshot_file(&h.store, &fs_b, "a").await, b"branch-B");
    // The base snapshot is untouched by either branch.
    assert_eq!(read_snapshot_file(&h.store, &base, "a").await, b"base");
}

#[tokio::test]
async fn run_without_fs_handle_has_no_fs_ca_id() {
    ensure_v8();
    let h = build_harness();
    let fs = run(&h.engine, r#"(async () => { 1 + 1; })()"#, None).await;
    assert!(fs.is_none(), "no mount → no fs CA id");
}

#[tokio::test]
async fn overlay_symlink_lstat_readlink_and_node_compat() {
    ensure_v8();
    let h = build_harness();

    // Exercises the overlay (mount) branch of the Node-compatibility surface:
    // fs.promises, symlink/lstat/readlink, Stats predicate methods, and the
    // ENOENT code on a miss. The script throws on any mismatch, so a clean
    // completion (a returned fs CA id) is the assertion.
    let ca = run(
        &h.engine,
        r#"(async () => {
            // fs.promises must be an enumerable own property so libraries detect it.
            const d = Object.getOwnPropertyDescriptor(fs, "promises");
            if (!(d && d.enumerable)) throw new Error("fs.promises must be enumerable");

            await fs.promises.writeFile("/repo/target.txt", "T");
            // Node signature: symlink(target, path).
            await fs.promises.symlink("/repo/target.txt", "/repo/link.txt");

            const ls = await fs.promises.lstat("/repo/link.txt");
            if (!ls.isSymbolicLink()) throw new Error("lstat should report a symlink");
            if (ls.isFile()) throw new Error("a symlink must not report isFile()");

            const tgt = await fs.promises.readlink("/repo/link.txt");
            if (tgt !== "/repo/target.txt") throw new Error("readlink mismatch: " + tgt);

            const st = await fs.promises.stat("/repo/target.txt");
            if (!st.isFile()) throw new Error("target should be a file");
            if (st.isDirectory()) throw new Error("target should not be a directory");

            let code = null;
            try { await fs.promises.readFile("/repo/missing.txt"); }
            catch (e) { code = e.code; }
            if (code !== "ENOENT") throw new Error("expected ENOENT, got " + code);
        })()"#,
        Some("main"),
    )
    .await
    .expect("overlay node-compat flow should yield an fs CA id");

    // The symlink persists in the pushed snapshot and reads back as its target.
    let id = blake3::Hash::from_bytes(parse_ca_hex(&ca).expect("valid ca hex"));
    let mount = SessionMount::pull(h.store.as_ref().clone(), id).await.unwrap();
    let target = mount.readlink("/repo/link.txt".as_ref()).await.unwrap();
    assert_eq!(target, std::path::PathBuf::from("/repo/target.txt"));
}

#[tokio::test]
async fn create_write_stream_assembles_a_large_file() {
    ensure_v8();
    let h = build_harness();

    // Stream a large file in many small pieces via fs.createWriteStream — the
    // whole value is never materialised in one JS string/buffer.
    let ca = run(
        &h.engine,
        r#"(async () => {
            const w = await fs.createWriteStream("/big/data.bin");
            // 512 KiB of deterministic bytes, fed 4 KiB at a time.
            const piece = new Uint8Array(4096);
            for (let i = 0; i < piece.length; i++) piece[i] = (i * 7 + 3) & 0xff;
            for (let n = 0; n < 128; n++) await w.write(piece);
            // A trailing text chunk too, to exercise the text path.
            await w.write("TAIL");
            await w.close();
        })()"#,
        Some("main"),
    )
    .await
    .expect("streaming write should yield an fs CA id");

    let bytes = read_snapshot_file(&h.store, &ca, "big/data.bin").await;
    assert_eq!(bytes.len(), 128 * 4096 + 4);

    // Verify the assembled content matches what was streamed.
    let mut expected = Vec::with_capacity(bytes.len());
    let mut piece = [0u8; 4096];
    for (i, b) in piece.iter_mut().enumerate() {
        *b = ((i * 7 + 3) & 0xff) as u8;
    }
    for _ in 0..128 {
        expected.extend_from_slice(&piece);
    }
    expected.extend_from_slice(b"TAIL");
    assert_eq!(bytes, expected, "streamed file content must round-trip");
}
