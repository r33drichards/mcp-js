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

fn engine() -> Arc<Engine> {
    let registry = ExecutionRegistry::new(&tmp("reg")).unwrap();
    Engine::from_engine(Engine::new_stateless(32 * 1024 * 1024, 30, 2)
        .with_fs_config(FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All))))
        .with_execution_registry(Arc::new(registry))
        .with_fs_snapshots(
            Arc::new(FsStore::in_memory()),
            Arc::new(LabelStore::in_memory()),
        ))
}

fn hexid(b: u8) -> String {
    ca_to_hex(&[b; 32])
}

/// An engine whose fs-snapshot pointer moves are gated by an inline Rego policy.
fn engine_with_policy(rego: &str) -> Arc<Engine> {
    use server::engine::opa::{build_policy_chain, EvalMode, OperationPolicies, PolicySource};
    let dir = std::env::temp_dir().join(format!("mcp-fssnap-rego-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "snap-{}.rego",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, rego).unwrap();
    let op = OperationPolicies {
        mode: EvalMode::All,
        policies: vec![PolicySource {
            url: format!("file://{}", path.display()),
            policy_path: None,
            rule: None,
        }],
    };
    let chain =
        build_policy_chain(&op, "mcp/fs_snapshot", "data.mcp.fs_snapshot.allow").unwrap();
    let registry = ExecutionRegistry::new(&tmp("reg")).unwrap();
    Engine::from_engine(
        Engine::new_stateless(32 * 1024 * 1024, 30, 2)
            .with_fs_config(FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All))))
            .with_execution_registry(Arc::new(registry))
            .with_fs_snapshots(
                Arc::new(FsStore::in_memory()),
                Arc::new(LabelStore::in_memory()),
            )
            .with_fs_snapshot_policy(Arc::new(chain)),
    )
}

#[tokio::test]
async fn fs_snapshot_policy_denies_push_to_protected_label() {
    // Allow any op except a push to the "protected" label.
    let rego = r#"
package mcp.fs_snapshot
default allow = false
allow if { input.op != "push" }
allow if {
    input.op == "push"
    input.label != "protected"
}
"#;
    let e = engine_with_policy(rego);
    let c0 = hexid(0);

    // Push to an ordinary label is allowed.
    assert!(e.fs_push("main".to_string(), c0.clone(), None, false, None).await.is_ok());

    // Push to the protected label is denied by policy.
    let err = e.fs_push("protected".to_string(), c0.clone(), None, false, None).await.unwrap_err();
    assert!(err.to_string().contains("denied by policy"), "unexpected error: {err}");
    // And the label was never created.
    assert_eq!(e.fs_resolve_label("protected".to_string()).await.unwrap(), None);
}

#[tokio::test]
async fn push_creates_then_advances_with_expected() {
    let e = engine();
    let c0 = hexid(0);
    let c1 = hexid(1);

    // First push to a fresh label creates it.
    match e.fs_push("main".to_string(), c0.clone(), None, false, None).await.unwrap() {
        FsPushOutcome::Advanced { ca_id, .. } => assert_eq!(ca_id, c0),
        other => panic!("expected Advanced, got {other:?}"),
    }
    assert_eq!(e.fs_resolve_label("main".to_string()).await.unwrap(), Some(c0.clone()));

    // Fast-forward with the correct expected head succeeds.
    match e.fs_push("main".to_string(), c1.clone(), Some(c0.clone()), false, None).await.unwrap() {
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
    e.fs_push("main".to_string(), c0.clone(), None, false, None).await.unwrap();
    e.fs_push("main".to_string(), c1.clone(), Some(c0.clone()), false, None).await.unwrap();

    // A push expecting the now-stale c0 is rejected with the real current head.
    match e.fs_push("main".to_string(), c2.clone(), Some(c0.clone()), false, None).await.unwrap() {
        FsPushOutcome::Rejected { current, .. } => assert_eq!(current, Some(c1.clone())),
        other => panic!("expected Rejected, got {other:?}"),
    }
    assert_eq!(e.fs_resolve_label("main".to_string()).await.unwrap(), Some(c1.clone()));

    // Force overrides the conflict.
    match e.fs_push("main".to_string(), c2.clone(), None, true, None).await.unwrap() {
        FsPushOutcome::Advanced { ca_id, .. } => assert_eq!(ca_id, c2),
        other => panic!("expected Advanced, got {other:?}"),
    }
    assert_eq!(e.fs_resolve_label("main".to_string()).await.unwrap(), Some(c2));
}

#[tokio::test]
async fn reset_requires_reflog_membership_unless_overridden() {
    let e = engine();
    let c0 = hexid(0);
    let c1 = hexid(1);
    let unlogged = hexid(9);
    e.fs_push("main".to_string(), c0.clone(), None, false, None).await.unwrap();
    e.fs_push("main".to_string(), c1.clone(), Some(c0.clone()), false, None).await.unwrap();

    // c0 is in the reflog → reset allowed.
    e.fs_reset("main".to_string(), c0.clone(), false, None).await.unwrap();
    assert_eq!(e.fs_resolve_label("main".to_string()).await.unwrap(), Some(c0.clone()));

    // A CA id never seen by this label is rejected without allow_unlogged.
    assert!(e.fs_reset("main".to_string(), unlogged.clone(), false, None).await.is_err());
    e.fs_reset("main".to_string(), unlogged.clone(), true, None).await.unwrap();
    assert_eq!(e.fs_resolve_label("main".to_string()).await.unwrap(), Some(unlogged));
}

#[tokio::test]
async fn list_and_log_reflect_operations() {
    let e = engine();
    let c0 = hexid(0);
    let c1 = hexid(1);
    e.fs_push("a".to_string(), c0.clone(), None, false, None).await.unwrap();
    e.fs_set_label("b".to_string(), c1.clone(), None).await.unwrap();

    let mut labels: Vec<_> = e
        .fs_list_labels()
        .await
        .unwrap()
        .into_iter()
        .map(|l| (l.name, l.ca_id))
        .collect();
    labels.sort();
    assert_eq!(labels, vec![("a".into(), c0.clone()), ("b".into(), c1)]);

    let log = e.fs_label_log("a".to_string(), None).await.unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].op, "create");
    assert_eq!(log[0].to, c0);
}

#[tokio::test]
async fn push_and_reset_messages_surface_in_the_reflog_view() {
    let e = engine();
    let c0 = hexid(0);
    let c1 = hexid(1);

    e.fs_push("main".to_string(), c0.clone(), None, false, Some("import baseline".into()))
        .await
        .unwrap();
    e.fs_push("main".to_string(), c1.clone(), Some(c0.clone()), false, Some("apply migration".into()))
        .await
        .unwrap();
    e.fs_reset("main".to_string(), c0.clone(), false, Some("revert migration".into()))
        .await
        .unwrap();

    let log = e.fs_label_log("main".to_string(), None).await.unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].message.as_deref(), Some("import baseline"));
    assert_eq!(log[1].message.as_deref(), Some("apply migration"));
    assert_eq!(log[2].message.as_deref(), Some("revert migration"));

    // An oversized message is rejected at the engine boundary.
    let too_long = "x".repeat(8192);
    let err = e
        .fs_push("main".to_string(), c1.clone(), Some(c0.clone()), false, Some(too_long))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("message too long"), "got: {err}");
}

#[tokio::test]
async fn log_limit_returns_the_most_recent_entries_oldest_first() {
    let e = engine();

    // Build a history of 5 moves: create, then four fast-forwards.
    let ids: Vec<String> = (0..5).map(hexid).collect();
    e.fs_push("main".to_string(), ids[0].clone(), None, false, None).await.unwrap();
    for i in 1..5 {
        e.fs_push("main".to_string(), ids[i].clone(), Some(ids[i - 1].clone()), false, None)
            .await
            .unwrap();
    }

    // No limit → full history.
    assert_eq!(e.fs_label_log("main".to_string(), None).await.unwrap().len(), 5);

    // limit=2 → the two most recent moves, still oldest-first within the window.
    let tail = e.fs_label_log("main".to_string(), Some(2)).await.unwrap();
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].to, ids[3]);
    assert_eq!(tail[1].to, ids[4]);

    // A limit larger than the history is clamped to what exists; limit=0 is empty.
    assert_eq!(e.fs_label_log("main".to_string(), Some(100)).await.unwrap().len(), 5);
    assert_eq!(e.fs_label_log("main".to_string(), Some(0)).await.unwrap().len(), 0);
}
