//! Engine-level fs_merge: 3-way merge through the store, returning either a new
//! snapshot CA id or a structured conflict list.

use std::sync::Arc;

use server::engine::execution::ExecutionRegistry;
use server::engine::fs::FsConfig;
use server::engine::fs_labels::LabelStore;
use server::engine::fs_merge::Prefer;
use server::engine::fs_mount::SessionMount;
use server::engine::fs_store::FsStore;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::{ca_to_hex, parse_ca_hex, Engine, FsMergeResult};

fn tmp(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "mcp-fsmrg-{tag}-{}-{}",
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

struct H {
    engine: Engine,
    store: Arc<FsStore>,
}

fn harness() -> H {
    let store = Arc::new(FsStore::in_memory());
    let engine = Engine::new_stateless(32 * 1024 * 1024, 30, 2)
        .with_fs_config(FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All))))
        .with_execution_registry(Arc::new(ExecutionRegistry::new(&tmp("reg")).unwrap()))
        .with_fs_snapshots(store.clone(), Arc::new(LabelStore::in_memory()));
    H { engine, store }
}

/// Build a snapshot from `(path, bytes)` and return its CA id (hex).
async fn snap(store: &FsStore, files: &[(&str, &[u8])]) -> String {
    let mut m = SessionMount::empty(store.clone());
    for (p, b) in files {
        m.write(p.as_ref(), b).await.unwrap();
    }
    ca_to_hex(m.push().await.unwrap().as_bytes())
}

async fn read_file(store: &FsStore, ca: &str, path: &str) -> Vec<u8> {
    let id = blake3::Hash::from_bytes(parse_ca_hex(ca).unwrap());
    SessionMount::pull(store.clone(), id)
        .await
        .unwrap()
        .read(path.as_ref())
        .await
        .unwrap()
}

#[tokio::test]
async fn three_way_merge_combines_non_overlapping_changes() {
    let h = harness();
    let base = snap(&h.store, &[("a", b"a0"), ("b", b"b0")]).await;
    let ours = snap(&h.store, &[("a", b"a1"), ("b", b"b0")]).await;
    let theirs = snap(&h.store, &[("a", b"a0"), ("b", b"b1")]).await;

    let merged = match h
        .engine
        .fs_merge(&ours, &theirs, Some(base), Prefer::None)
        .await
        .unwrap()
    {
        FsMergeResult::Merged { ca_id } => ca_id,
        other => panic!("expected clean merge, got {other:?}"),
    };
    assert_eq!(read_file(&h.store, &merged, "a").await, b"a1");
    assert_eq!(read_file(&h.store, &merged, "b").await, b"b1");
}

#[tokio::test]
async fn divergent_path_reports_structured_conflict() {
    let h = harness();
    let base = snap(&h.store, &[("a", b"a0")]).await;
    let ours = snap(&h.store, &[("a", b"a1")]).await;
    let theirs = snap(&h.store, &[("a", b"a2")]).await;

    match h
        .engine
        .fs_merge(&ours, &theirs, Some(base), Prefer::None)
        .await
        .unwrap()
    {
        FsMergeResult::Conflict { conflicts } => {
            assert_eq!(conflicts.len(), 1);
            let c = &conflicts[0];
            assert_eq!(c.path, "a");
            // All three sides present, ours != theirs.
            assert!(c.base.is_some() && c.ours.is_some() && c.theirs.is_some());
            assert_ne!(c.ours, c.theirs);
        }
        other => panic!("expected conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn prefer_ours_resolves_conflict_into_a_snapshot() {
    let h = harness();
    let base = snap(&h.store, &[("a", b"a0")]).await;
    let ours = snap(&h.store, &[("a", b"mine")]).await;
    let theirs = snap(&h.store, &[("a", b"yours")]).await;

    let merged = match h
        .engine
        .fs_merge(&ours, &theirs, Some(base), Prefer::Ours)
        .await
        .unwrap()
    {
        FsMergeResult::Merged { ca_id } => ca_id,
        other => panic!("expected merge, got {other:?}"),
    };
    assert_eq!(read_file(&h.store, &merged, "a").await, b"mine");
}

#[tokio::test]
async fn clean_merge_is_deterministic_and_dedups() {
    let h = harness();
    let base = snap(&h.store, &[("a", b"a0"), ("b", b"b0")]).await;
    let ours = snap(&h.store, &[("a", b"a1"), ("b", b"b0")]).await;
    let theirs = snap(&h.store, &[("a", b"a0"), ("b", b"b1")]).await;

    let m1 = h.engine.fs_merge(&ours, &theirs, Some(base.clone()), Prefer::None).await.unwrap();
    let m2 = h.engine.fs_merge(&ours, &theirs, Some(base), Prefer::None).await.unwrap();
    let (FsMergeResult::Merged { ca_id: a }, FsMergeResult::Merged { ca_id: b }) = (m1, m2) else {
        panic!("expected two clean merges");
    };
    assert_eq!(a, b, "same inputs must yield the same merged CA id");
}

#[tokio::test]
async fn two_way_merge_without_base_conflicts_on_divergence() {
    let h = harness();
    let ours = snap(&h.store, &[("a", b"a1"), ("x", b"x")]).await;
    let theirs = snap(&h.store, &[("a", b"a2"), ("y", b"y")]).await;

    // No base: divergent 'a' conflicts.
    match h.engine.fs_merge(&ours, &theirs, None, Prefer::None).await.unwrap() {
        FsMergeResult::Conflict { conflicts } => {
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].path, "a");
            assert!(conflicts[0].base.is_none());
        }
        other => panic!("expected conflict, got {other:?}"),
    }
    // prefer=theirs keeps both disjoint additions plus theirs' 'a'.
    let merged = match h.engine.fs_merge(&ours, &theirs, None, Prefer::Theirs).await.unwrap() {
        FsMergeResult::Merged { ca_id } => ca_id,
        other => panic!("expected merge, got {other:?}"),
    };
    assert_eq!(read_file(&h.store, &merged, "x").await, b"x");
    assert_eq!(read_file(&h.store, &merged, "y").await, b"y");
    assert_eq!(read_file(&h.store, &merged, "a").await, b"a2");
}
