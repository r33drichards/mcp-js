use mcp_metadata_store::{
    MetadataStore, RuntimeStore, RuntimeTree, SledStore, Write,
    blob::BlobStorage,
    content::{ObjectKind, ObjectRef},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
#[derive(Default)]
struct Blobs {
    values: Mutex<HashMap<String, Vec<u8>>>,
    gets: AtomicUsize,
    puts: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    hold: AtomicBool,
    delay: Mutex<Duration>,
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl BlobStorage for Blobs {
    async fn put(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.values
            .lock()
            .unwrap()
            .insert(key.into(), value.to_vec());
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, String> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.values
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or("missing blob".into())
    }
    async fn put_immutable(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        let _active = Active(&self.active);
        while self.hold.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let delay = *self.delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        let mut values = self.values.lock().unwrap();
        match values.get(key) {
            Some(old) if old != value => Err("immutable conflict".into()),
            Some(_) => Ok(()),
            None => {
                values.insert(key.into(), value.to_vec());
                Ok(())
            }
        }
    }
}
fn database() -> Arc<SledStore> {
    Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    ))
}
#[tokio::test]
async fn upgrade_preserves_raw_legacy_marker_bytes_for_local_and_remote_console_rows() {
    let db = database();
    let blobs = Arc::new(Blobs::default());
    let runtime = RuntimeStore::new(db.clone())
        .unwrap()
        .with_blobs(blobs.clone());
    for marker in [
        b"\0mcp-runtime-content-v1\0".as_slice(),
        b"\0mcp-runtime-blob-v1\0".as_slice(),
    ] {
        let mut bytes = marker.to_vec();
        bytes.extend(
            ObjectRef::identify(ObjectKind::ConsolePage, b"looks like a reference").encode(),
        );
        let local = db.0.open_tree("ex:local").unwrap();
        local.insert(b"key", bytes.clone()).unwrap();
        let local = RuntimeTree::from(local).with_payload_store(runtime.clone());
        assert_eq!(local.get(b"key").unwrap(), Some(bytes.clone()));
        assert_eq!(local.iter().next().unwrap().unwrap().1, bytes);
        let mut encoded = vec![1];
        encoded.extend(&bytes);
        db.put(Write::new("ex:remote", b"key", encoded))
            .await
            .unwrap();
        assert_eq!(runtime.tree("ex:remote").get(b"key").unwrap(), Some(bytes));
    }
    assert_eq!(
        blobs.gets.load(Ordering::SeqCst),
        0,
        "inline bytes must never be dereferenced"
    );
}
#[tokio::test]
async fn malformed_legacy_references_never_reach_blob_storage() {
    let db = database();
    let blobs = Arc::new(Blobs::default());
    let runtime = RuntimeStore::new(db.clone())
        .unwrap()
        .with_blobs(blobs.clone());
    for name in [
        "runtime/../../secret",
        "runtime/not-a-uuid",
        "runtime/00000000-0000-4000-8000-000000000001/extra",
    ] {
        let mut encoded = vec![2];
        encoded.extend(name.as_bytes());
        db.put(Write::new("artifacts", b"key", encoded))
            .await
            .unwrap();
        assert!(
            runtime
                .tree("artifacts")
                .get(b"key")
                .unwrap_err()
                .contains("invalid legacy")
        );
    }
    assert_eq!(blobs.gets.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn page_index_and_iteration_are_lazy_even_when_a_later_object_is_missing() {
    let db = database();
    let blobs = Arc::new(Blobs::default());
    let tree = RuntimeStore::new(db)
        .unwrap()
        .with_blobs(blobs.clone())
        .tree("ex:pages");
    tree.insert(0u64.to_be_bytes(), b"first\n").unwrap();
    tree.insert(1u64.to_be_bytes(), b"later\n").unwrap();
    blobs
        .values
        .lock()
        .unwrap()
        .remove(&ObjectRef::identify(ObjectKind::ConsolePage, b"later\n").key());
    let pages = tree.pages().unwrap();
    assert_eq!(blobs.gets.load(Ordering::SeqCst), 0);
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1].newlines, 1);
    assert_eq!(tree.iter().next().unwrap().unwrap().1, b"first\n");
    assert_eq!(blobs.gets.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn blocked_blob_work_does_not_block_metadata() {
    let db = database();
    let blobs = Arc::new(Blobs::default());
    blobs.hold.store(true, Ordering::SeqCst);
    let runtime = RuntimeStore::new(db).unwrap().with_blobs(blobs.clone());
    let metadata = runtime.tree("execution_records");
    metadata.insert(b"status", b"ready").unwrap();
    let page = runtime.tree("ex:blocked");
    let writer = std::thread::spawn(move || page.insert(b"key", b"blocked"));
    tokio::time::timeout(Duration::from_secs(2), async {
        while blobs.active.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    let status = metadata.get(b"status");
    blobs.hold.store(false, Ordering::SeqCst);
    assert_eq!(status.unwrap(), Some(b"ready".to_vec()));
    writer.join().unwrap().unwrap();
}
#[tokio::test]
async fn blob_deadline_includes_queue_time_and_does_not_publish_expired_work() {
    let db = database();
    let blobs = Arc::new(Blobs::default());
    *blobs.delay.lock().unwrap() = Duration::from_millis(150);
    let runtime = RuntimeStore::new(db.clone())
        .unwrap()
        .with_blobs(blobs.clone())
        .with_blob_limits(1, 2, Duration::from_millis(220))
        .unwrap();
    let first = runtime.tree("ex:queue");
    let writer = std::thread::spawn(move || first.insert(b"first", b"first"));
    tokio::time::timeout(Duration::from_secs(2), async {
        while blobs.active.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    let second = runtime.tree("ex:queue").insert(b"second", b"second");
    writer.join().unwrap().unwrap();
    assert!(second.unwrap_err().contains("queue time"));
    assert!(runtime.tree("ex:queue").get(b"second").unwrap().is_none());
    assert_eq!(blobs.max_active.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn blob_concurrency_and_queue_capacity_are_bounded() {
    let blobs = Arc::new(Blobs::default());
    *blobs.delay.lock().unwrap() = Duration::from_millis(80);
    let runtime = RuntimeStore::new(database())
        .unwrap()
        .with_blobs(blobs.clone())
        .with_blob_limits(2, 1, Duration::from_secs(2))
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let mut threads = vec![];
    for i in 0u64..8 {
        let tree = runtime.tree("ex:bounded");
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            tree.insert(i.to_be_bytes(), b"value")
        }));
    }
    barrier.wait();
    let mut rejected = 0;
    for thread in threads {
        if let Err(error) = thread.join().unwrap() {
            assert!(error.contains("queue is full"), "{error}");
            rejected += 1;
        }
    }
    assert!(rejected > 0);
    assert!(blobs.max_active.load(Ordering::SeqCst) <= 2);
}
