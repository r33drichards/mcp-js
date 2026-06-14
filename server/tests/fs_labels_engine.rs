//! Engine-level fs label operations: push (reject-and-rebase / force), reset
//! (reflog-validated), set/list/log. These back the MCP, HTTP, and CLI surfaces.

use std::sync::Arc;

use server::engine::execution::ExecutionRegistry;
use server::engine::fs::FsConfig;
use server::engine::fs_labels::LabelStore;
use server::engine::fs_store::FsStore;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::{ca_to_hex, Engine, FsPushOutcome};

fn tmp(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "mcp-fslbl-{tag}-{}-{}",
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

fn engine() -> Engine {
    let registry = ExecutionRegistry::new(&tmp("reg")).unwrap();
    Engine::new_stateless(32 * 1024 * 1024, 30, 2)
        .with_fs_config(FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All))))
        .with_execution_registry(Arc::new(registry))
        .with_fs_snapshots(
            Arc::new(FsStore::in_memory()),
            Arc::new(LabelStore::in_memory()),
        )
}

fn hexid(b: u8) -> String {
    ca_to_hex(&[b; 32])
}

#[tokio::test]
async fn push_creates_then_advances_with_expected() {
    let e = engine();
    let c0 = hexid(0);
    let c1 = hexid(1);

    // First push to a fresh label creates it.
    match e.fs_push("main", &c0, None, false).await.unwrap() {
        FsPushOutcome::Advanced { ca_id, .. } => assert_eq!(ca_id, c0),
        other => panic!("expected Advanced, got {other:?}"),
    }
    assert_eq!(e.fs_resolve_label("main").await.unwrap(), Some(c0.clone()));

    // Fast-forward with the correct expected head succeeds.
    match e.fs_push("main", &c1, Some(c0.clone()), false).await.unwrap() {
        FsPushOutcome::Advanced { ca_id, .. } => assert_eq!(ca_id, c1),
        other => panic!("expected Advanced, got {other:?}"),
    }
}

#[tokio::test]
async fn push_with_stale_expected_is_rejected_then_force_wins() {
    let e = engine();
    let c0 = hexid(0);
    let c1 = hexid(1);
    let c2 = hexid(2);
    e.fs_push("main", &c0, None, false).await.unwrap();
    e.fs_push("main", &c1, Some(c0.clone()), false).await.unwrap();

    // A push expecting the now-stale c0 is rejected with the real current head.
    match e.fs_push("main", &c2, Some(c0.clone()), false).await.unwrap() {
        FsPushOutcome::Rejected { current, .. } => assert_eq!(current, Some(c1.clone())),
        other => panic!("expected Rejected, got {other:?}"),
    }
    assert_eq!(e.fs_resolve_label("main").await.unwrap(), Some(c1.clone()));

    // Force overrides the conflict.
    match e.fs_push("main", &c2, None, true).await.unwrap() {
        FsPushOutcome::Advanced { ca_id, .. } => assert_eq!(ca_id, c2),
        other => panic!("expected Advanced, got {other:?}"),
    }
    assert_eq!(e.fs_resolve_label("main").await.unwrap(), Some(c2));
}

#[tokio::test]
async fn reset_requires_reflog_membership_unless_overridden() {
    let e = engine();
    let c0 = hexid(0);
    let c1 = hexid(1);
    let unlogged = hexid(9);
    e.fs_push("main", &c0, None, false).await.unwrap();
    e.fs_push("main", &c1, Some(c0.clone()), false).await.unwrap();

    // c0 is in the reflog → reset allowed.
    e.fs_reset("main", &c0, false).await.unwrap();
    assert_eq!(e.fs_resolve_label("main").await.unwrap(), Some(c0.clone()));

    // A CA id never seen by this label is rejected without allow_unlogged.
    assert!(e.fs_reset("main", &unlogged, false).await.is_err());
    e.fs_reset("main", &unlogged, true).await.unwrap();
    assert_eq!(e.fs_resolve_label("main").await.unwrap(), Some(unlogged));
}

#[tokio::test]
async fn list_and_log_reflect_operations() {
    let e = engine();
    let c0 = hexid(0);
    let c1 = hexid(1);
    e.fs_push("a", &c0, None, false).await.unwrap();
    e.fs_set_label("b", &c1).await.unwrap();

    let mut labels: Vec<_> = e
        .fs_list_labels()
        .await
        .unwrap()
        .into_iter()
        .map(|l| (l.name, l.ca_id))
        .collect();
    labels.sort();
    assert_eq!(labels, vec![("a".into(), c0.clone()), ("b".into(), c1)]);

    let log = e.fs_label_log("a").await.unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].op, "create");
    assert_eq!(log[0].to, c0);
}
