use mcp_metadata_store::{
    MetadataStore, Result, Row, SledStore, SqliteStore, Write,
    publication::{MetadataPublication, Publication, PublishOutcome, ReadMode},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
fn request(id: &str, expected: Option<u64>, value: Option<&[u8]>) -> Publication {
    Publication {
        operation_id: id.into(),
        key: b"key\0binary".to_vec(),
        expected_revision: expected,
        value: value.map(<[u8]>::to_vec),
        companions: vec![Write::new("history", id.as_bytes(), b"event")],
    }
}
async fn contract(store: Arc<dyn MetadataStore>) {
    let service = MetadataPublication::new(store.clone(), "test").unwrap();
    let first = request("first", None, Some(b"value"));
    assert_eq!(
        service.publish(&first).await.unwrap(),
        PublishOutcome::Committed { revision: 1 }
    );
    assert_eq!(
        service.publish(&first).await.unwrap(),
        PublishOutcome::Committed { revision: 1 }
    );
    assert_eq!(
        store
            .scan(&service.index_tree("history"), b"", false, None)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut changed = first.clone();
    changed.value = Some(b"different".to_vec());
    assert!(service.publish(&changed).await.is_err());
    let deletion = request("delete", Some(1), None);
    assert_eq!(
        service.publish(&deletion).await.unwrap(),
        PublishOutcome::Committed { revision: 2 }
    );
    assert_eq!(
        service
            .read(&first.key, ReadMode::Linearizable)
            .await
            .unwrap()
            .unwrap()
            .value,
        None
    );
    assert_eq!(
        service
            .publish(&request("aba", None, Some(b"new")))
            .await
            .unwrap(),
        PublishOutcome::Conflict
    );
    assert!(
        store
            .get(&service.index_tree("history"), b"aba")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        service
            .publish(&request("recreate", Some(2), Some(b"new")))
            .await
            .unwrap(),
        PublishOutcome::Committed { revision: 3 }
    );
    assert_eq!(
        service.publish(&first).await.unwrap(),
        PublishOutcome::Committed { revision: 1 },
        "old retry must not rewrite new state"
    );
    assert_eq!(
        service
            .read(&first.key, ReadMode::Linearizable)
            .await
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    let other = MetadataPublication::new(store, "other").unwrap();
    assert!(
        other
            .read(&first.key, ReadMode::Linearizable)
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn publication_sled_contract() {
    contract(Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    )))
    .await
}
#[tokio::test]
async fn publication_sqlite_contract() {
    let dir = tempfile::tempdir().unwrap();
    contract(Arc::new(
        SqliteStore::open(dir.path().join("metadata"), "test").unwrap(),
    ))
    .await
}

struct ReceiptGate {
    enabled: AtomicBool,
    paused: AtomicBool,
    released: AtomicBool,
}
struct LostReply {
    store: Arc<dyn MetadataStore>,
    fail: AtomicBool,
    gate: Option<Arc<ReceiptGate>>,
}
#[async_trait::async_trait]
impl MetadataStore for LostReply {
    async fn get(&self, t: &str, k: &[u8]) -> Result<Option<Vec<u8>>> {
        let result = self.store.get(t, k).await;
        if let Some(gate) = &self.gate {
            if t.ends_with("/operations") && gate.enabled.swap(false, Ordering::SeqCst) {
                gate.paused.store(true, Ordering::SeqCst);
                while !gate.released.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            }
        }
        result
    }
    async fn put(&self, w: Write) -> Result<()> {
        self.store.put(w).await
    }
    async fn scan(&self, t: &str, p: &[u8], r: bool, l: Option<usize>) -> Result<Vec<Row>> {
        self.store.scan(t, p, r, l).await
    }
    async fn compare_and_swap(&self, w: Write, e: Option<Vec<u8>>, c: Vec<Write>) -> Result<bool> {
        let applied = self.store.compare_and_swap(w, e, c).await?;
        if applied && self.fail.swap(false, Ordering::SeqCst) {
            Err("lost commit acknowledgement".into())
        } else {
            Ok(applied)
        }
    }
    async fn generate_id(&self) -> Result<u64> {
        self.store.generate_id().await
    }
    async fn tree_names(&self) -> Result<Vec<String>> {
        self.store.tree_names().await
    }
    fn flush(&self) -> Result<()> {
        self.store.flush()
    }
}
#[tokio::test]
async fn lost_commit_reply_replays_receipt_without_duplicate_companions() {
    let raw = Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    ));
    let service = MetadataPublication::new(
        Arc::new(LostReply {
            store: raw.clone(),
            fail: AtomicBool::new(true),
            gate: None,
        }),
        "test",
    )
    .unwrap();
    let publication = request("once", None, Some(b"value"));
    assert!(matches!(
        service.publish(&publication).await.unwrap(),
        PublishOutcome::UnknownOutcome { .. }
    ));
    assert_eq!(
        service.publish(&publication).await.unwrap(),
        PublishOutcome::Committed { revision: 1 }
    );
    assert_eq!(
        raw.scan(&service.index_tree("history"), b"", false, None)
            .await
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_publication_has_one_committed_winner() {
    let raw = Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    ));
    let service = MetadataPublication::new(raw.clone(), "test").unwrap();
    let mut tasks = vec![];
    for i in 0..16 {
        let service = service.clone();
        tasks.push(tokio::spawn(async move {
            service
                .publish(&request(&i.to_string(), None, Some(b"value")))
                .await
                .unwrap()
        }))
    }
    let mut wins = 0;
    for task in tasks {
        match task.await.unwrap() {
            PublishOutcome::Committed { revision: 1 } => wins += 1,
            PublishOutcome::Conflict => (),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(wins, 1);
    assert_eq!(
        raw.scan(&service.index_tree("history"), b"", false, None)
            .await
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn failed_object_write_cannot_publish_metadata() {
    use mcp_metadata_store::{
        blob::BlobStorage,
        content::{ContentObjects, ObjectKind},
        publication::ObjectPublication,
    };
    struct Failed;
    #[async_trait::async_trait]
    impl BlobStorage for Failed {
        async fn put(&self, _: &str, _: &[u8]) -> Result<()> {
            Err("failed".into())
        }
        async fn get(&self, _: &str) -> Result<Vec<u8>> {
            Err("missing".into())
        }
        async fn put_immutable(&self, _: &str, _: &[u8]) -> Result<()> {
            Err("failed object write".into())
        }
    }
    let raw = Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    ));
    let metadata = MetadataPublication::new(raw, "test").unwrap();
    let service = ObjectPublication {
        objects: ContentObjects::new(Arc::new(Failed)),
        metadata: metadata.clone(),
    };
    assert!(
        service
            .publish(
                request("failure", None, None),
                ObjectKind::Artifact,
                b"bytes"
            )
            .await
            .is_err()
    );
    assert!(
        metadata
            .read(b"key\0binary", ReadMode::Linearizable)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_stale_receipt_read_cannot_overwrite_an_operation_id_with_a_different_request() {
    let raw = Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    ));
    let gate = Arc::new(ReceiptGate {
        enabled: AtomicBool::new(true),
        paused: AtomicBool::new(false),
        released: AtomicBool::new(false),
    });
    let service = MetadataPublication::new(
        Arc::new(LostReply {
            store: raw,
            fail: AtomicBool::new(false),
            gate: Some(gate.clone()),
        }),
        "race",
    )
    .unwrap();
    let concurrent = service.clone();
    let second = tokio::spawn(async move {
        concurrent
            .publish(&request("same", Some(1), Some(b"second")))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !gate.paused.load(Ordering::SeqCst) {
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    assert_eq!(
        service
            .publish(&request("same", None, Some(b"first")))
            .await
            .unwrap(),
        PublishOutcome::Committed { revision: 1 }
    );
    gate.released.store(true, Ordering::SeqCst);
    assert!(second.await.unwrap().unwrap_err().contains("reused"));
    let record = service
        .read(b"key\0binary", ReadMode::Linearizable)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.revision, 1);
    assert_eq!(record.value, Some(b"first".to_vec()));
}
