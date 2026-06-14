use server::engine::fs_labels::{LabelStore, RefOp};

#[tokio::test]
async fn cas_advances_only_from_expected_head() {
    let s = LabelStore::in_memory();
    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    let c2 = [2u8; 32];
    s.create("main", c0).await.unwrap();

    assert!(s.cas("main", Some(c0), c1).await.unwrap()); // fast-forward ok
    assert!(!s.cas("main", Some(c0), c2).await.unwrap()); // stale expectation rejected
    assert_eq!(s.resolve("main").await.unwrap(), Some(c1));
}

#[tokio::test]
async fn reset_moves_pointer_and_reflog_records_history() {
    let s = LabelStore::in_memory();
    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    s.create("main", c0).await.unwrap();
    s.cas("main", Some(c0), c1).await.unwrap();
    s.force("main", c0).await.unwrap(); // reset back

    assert_eq!(s.resolve("main").await.unwrap(), Some(c0));
    let log = s.log("main").await.unwrap();
    assert_eq!(log.last().unwrap().op, RefOp::Reset);
    assert!(
        log.iter().any(|e| e.to == c1),
        "history retains the rolled-past id"
    );
}

#[tokio::test]
async fn create_is_exclusive_and_resolve_unknown_is_none() {
    let s = LabelStore::in_memory();
    let c0 = [9u8; 32];
    assert_eq!(s.resolve("nope").await.unwrap(), None);
    s.create("main", c0).await.unwrap();
    assert!(s.create("main", c0).await.is_err(), "create must be exclusive");
}

#[tokio::test]
async fn cas_on_missing_label_requires_none_expectation() {
    let s = LabelStore::in_memory();
    let c1 = [1u8; 32];
    // Wrong expectation against a non-existent label is rejected, not created.
    assert!(!s.cas("ghost", Some([0u8; 32]), c1).await.unwrap());
    assert_eq!(s.resolve("ghost").await.unwrap(), None);
}

#[tokio::test]
async fn list_returns_all_label_heads() {
    let s = LabelStore::in_memory();
    s.create("a", [1u8; 32]).await.unwrap();
    s.create("b", [2u8; 32]).await.unwrap();
    let mut labels = s.list().await.unwrap();
    labels.sort();
    assert_eq!(
        labels,
        vec![("a".to_string(), [1u8; 32]), ("b".to_string(), [2u8; 32])]
    );
}

#[tokio::test]
async fn reflog_is_ordered_create_then_push() {
    let s = LabelStore::in_memory();
    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    s.create("main", c0).await.unwrap();
    s.cas("main", Some(c0), c1).await.unwrap();
    let log = s.log("main").await.unwrap();
    assert_eq!(log[0].op, RefOp::Create);
    assert_eq!(log[0].to, c0);
    assert_eq!(log[1].op, RefOp::Push);
    assert_eq!(log[1].from, Some(c0));
    assert_eq!(log[1].to, c1);
}
