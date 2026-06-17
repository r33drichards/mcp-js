use server::engine::fs_labels::{LabelStore, RefOp, MAX_MESSAGE_LEN};


async fn cas_advances_only_from_expected_head() {
    let s = LabelStore::in_memory();
    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    let c2 = [2u8; 32];
    s.create("main", c0, None).await.unwrap();

    assert!(s.cas("main", Some(c0), c1, None).await.unwrap());     assert!(!s.cas("main", Some(c0), c2, None).await.unwrap());     assert_eq!(s.resolve("main").await.unwrap(), Some(c1));
}


async fn reset_moves_pointer_and_reflog_records_history() {
    let s = LabelStore::in_memory();
    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    s.create("main", c0, None).await.unwrap();
    s.cas("main", Some(c0), c1, None).await.unwrap();
    s.force("main", c0, None).await.unwrap(); 
    assert_eq!(s.resolve("main").await.unwrap(), Some(c0));
    let log = s.log("main").await.unwrap();
    assert_eq!(log.last().unwrap().op, RefOp::Reset);
    assert!(
        log.iter().any(|e| e.to == c1),
        "history retains the rolled-past id"
    );
}


async fn create_is_exclusive_and_resolve_unknown_is_none() {
    let s = LabelStore::in_memory();
    let c0 = [9u8; 32];
    assert_eq!(s.resolve("nope").await.unwrap(), None);
    s.create("main", c0, None).await.unwrap();
    assert!(s.create("main", c0, None).await.is_err(), "create must be exclusive");
}


async fn cas_on_missing_label_requires_none_expectation() {
    let s = LabelStore::in_memory();
    let c1 = [1u8; 32];
        assert!(!s.cas("ghost", Some([0u8; 32]), c1, None).await.unwrap());
    assert_eq!(s.resolve("ghost").await.unwrap(), None);
}


async fn list_returns_all_label_heads() {
    let s = LabelStore::in_memory();
    s.create("a", [1u8; 32], None).await.unwrap();
    s.create("b", [2u8; 32], None).await.unwrap();
    let mut labels = s.list().await.unwrap();
    labels.sort();
    assert_eq!(
        labels,
        vec![("a".to_string(), [1u8; 32]), ("b".to_string(), [2u8; 32])]
    );
}


async fn reflog_is_ordered_create_then_push() {
    let s = LabelStore::in_memory();
    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    s.create("main", c0, None).await.unwrap();
    s.cas("main", Some(c0), c1, None).await.unwrap();
    let log = s.log("main").await.unwrap();
    assert_eq!(log[0].op, RefOp::Create);
    assert_eq!(log[0].to, c0);
    assert_eq!(log[1].op, RefOp::Push);
    assert_eq!(log[1].from, Some(c0));
    assert_eq!(log[1].to, c1);
}


async fn reflog_records_optional_message_per_entry() {
    let s = LabelStore::in_memory();
    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    s.create("main", c0, Some("seed".into())).await.unwrap();
        s.cas("main", Some(c0), c1, None).await.unwrap();
    s.force("main", c0, Some("roll back to seed".into())).await.unwrap();

    let log = s.log("main").await.unwrap();
    assert_eq!(log[0].message.as_deref(), Some("seed"));
    assert_eq!(log[1].message, None);
    assert_eq!(log[2].message.as_deref(), Some("roll back to seed"));
}


async fn oversized_message_is_rejected_and_nothing_moves() {
    let s = LabelStore::in_memory();
    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    s.create("main", c0, None).await.unwrap();

    let too_long = "x".repeat(MAX_MESSAGE_LEN + 1);
    let err = s
        .cas("main", Some(c0), c1, Some(too_long))
        .await
        .unwrap_err();
    assert!(err.contains("message too long"), "got: {err}");

        assert_eq!(s.resolve("main").await.unwrap(), Some(c0));
    assert_eq!(s.log("main").await.unwrap().len(), 1);

        let at_cap = "y".repeat(MAX_MESSAGE_LEN);
    assert!(s.cas("main", Some(c0), c1, Some(at_cap)).await.unwrap());
}
