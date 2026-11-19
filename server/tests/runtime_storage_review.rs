use mcp_metadata_store::{
    RuntimeStore, SledStore,
    blob::BlobStorage,
    content::{ObjectKind, ObjectRef},
};
use server::engine::{
    console::{BLOB_PAGE_SIZE, ConsoleLogState},
    execution::ExecutionRegistry,
    heap_storage::{FileHeapStorage, MemoryHeapStorage, WriteThroughCacheHeapStorage},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
#[derive(Default)]
struct Counting {
    inner: MemoryHeapStorage,
    gets: AtomicUsize,
    puts: AtomicUsize,
}
#[async_trait::async_trait]
impl BlobStorage for Counting {
    async fn put(&self, k: &str, v: &[u8]) -> Result<(), String> {
        self.inner.put(k, v).await
    }
    async fn get(&self, k: &str) -> Result<Vec<u8>, String> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(k).await
    }
    async fn put_immutable(&self, k: &str, v: &[u8]) -> Result<(), String> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        self.inner.put_immutable(k, v).await
    }
}
fn registry(blobs: Arc<Counting>) -> ExecutionRegistry {
    ExecutionRegistry::from_store(Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    )))
    .unwrap()
    .with_blobs(blobs)
}
#[tokio::test]
async fn console_batches_one_megabyte_into_four_blob_writes() {
    let blobs = Arc::new(Counting::default());
    let registry = registry(blobs.clone());
    let tree = registry.register("batch").unwrap();
    let state = ConsoleLogState::new(tree);
    state.write(&vec![b'x'; 1024 * 1024]);
    state.flush().unwrap();
    assert_eq!(BLOB_PAGE_SIZE, 256 * 1024);
    assert_eq!(blobs.puts.load(Ordering::SeqCst), 4);
    let result = registry
        .get_console_output("batch", None, None, Some(0), Some(16))
        .unwrap();
    assert_eq!(result.data, "x".repeat(16));
    assert_eq!(result.total_bytes, 1024 * 1024);
    assert_eq!(blobs.gets.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn early_console_windows_do_not_fetch_missing_later_objects() {
    let blobs = Arc::new(Counting::default());
    let registry = registry(blobs.clone());
    let tree = registry.register("pages").unwrap();
    tree.insert(0u64.to_be_bytes(), b"first\nsecond\n").unwrap();
    let later = b"later\n".repeat(50_000);
    tree.insert(1u64.to_be_bytes(), &later).unwrap();
    blobs
        .inner
        .delete(&ObjectRef::identify(ObjectKind::ConsolePage, &later).key())
        .await
        .unwrap();
    let bytes = registry
        .get_console_output("pages", None, None, Some(0), Some(5))
        .unwrap();
    assert_eq!(bytes.data, "first");
    assert_eq!(bytes.total_lines, 50_002);
    assert_eq!(blobs.gets.load(Ordering::SeqCst), 1);
    let lines = registry
        .get_console_output("pages", Some(1), Some(1), None, None)
        .unwrap();
    assert_eq!(lines.data, "first");
    assert_eq!(lines.end_byte, 6);
    assert_eq!(blobs.gets.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn console_windows_cross_pages_and_preserve_line_coordinates() {
    let blobs = Arc::new(Counting::default());
    let registry = registry(blobs.clone());
    let tree = registry.register("cross").unwrap();
    tree.insert(0u64.to_be_bytes(), b"alpha\nbe").unwrap();
    tree.insert(1u64.to_be_bytes(), b"ta\ngamma\n").unwrap();
    let line = registry
        .get_console_output("cross", Some(2), Some(1), None, None)
        .unwrap();
    assert_eq!(line.data, "beta");
    assert_eq!((line.start_byte, line.end_byte), (6, 11));
    assert_eq!((line.start_line, line.end_line), (2, 2));
    assert!(line.has_more);
    let bytes = registry
        .get_console_output("cross", None, None, Some(8), Some(4))
        .unwrap();
    assert_eq!(bytes.data, "ta\ng");
    assert_eq!((bytes.start_line, bytes.end_line), (2, 3));
    assert_eq!(
        registry
            .get_console_output("cross", None, None, Some(u64::MAX), Some(u64::MAX))
            .unwrap()
            .data,
        ""
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cached_immutable_rewrites_never_expose_partial_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(WriteThroughCacheHeapStorage::new(
        MemoryHeapStorage::new(),
        root.path(),
    ));
    let bytes = Arc::new(vec![42; 1024 * 1024]);
    store
        .put_immutable("objects/content", &bytes)
        .await
        .unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let reader = {
        let store = store.clone();
        let bytes = bytes.clone();
        let done = done.clone();
        let reads = reads.clone();
        tokio::spawn(async move {
            while !done.load(Ordering::SeqCst) {
                assert_eq!(store.get("objects/content").await.unwrap(), *bytes);
                reads.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
        })
    };
    for _ in 0..30 {
        store
            .put_immutable("objects/content", &bytes)
            .await
            .unwrap();
        tokio::task::yield_now().await;
    }
    done.store(true, Ordering::SeqCst);
    reader.await.unwrap();
    assert!(reads.load(Ordering::SeqCst) > 0);
}
#[cfg(unix)]
#[tokio::test]
async fn writable_blob_root_does_not_require_read_access_to_existing_ancestors() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let ancestor = root.path().join("search-only");
    let directory = ancestor.join("writable");
    std::fs::create_dir_all(&directory).unwrap();
    struct Restore(std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _restore = Restore(ancestor.clone());
    std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o111)).unwrap();
    assert!(
        std::fs::read_dir(&ancestor).is_err(),
        "test must run without directory read privilege"
    );
    let store = FileHeapStorage::new(directory);
    store
        .put_immutable("objects/new/content", b"durable")
        .await
        .unwrap();
    assert_eq!(store.get("objects/new/content").await.unwrap(), b"durable");
}
#[path = "support/s3.rs"]
mod s3;
#[tokio::test]
async fn stalled_s3_put_does_not_block_an_unrelated_metadata_request() {
    let fixture = s3::S3::start().await;
    fixture.objects.hold_put.store(true, Ordering::SeqCst);
    let endpoint = fixture.endpoint.clone();
    let store = RuntimeStore::from_factory(async move {
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "test", "test", None, None, "test",
            ))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .build();
        let backend = server::engine::heap_storage::S3HeapStorage::from_client(
            "runtime-test",
            aws_sdk_s3::Client::from_conf(config),
        );
        Ok((
            Arc::new(SledStore(
                sled::Config::new().temporary(true).open().unwrap(),
            )) as Arc<dyn mcp_metadata_store::MetadataStore>,
            Some(Arc::new(backend) as Arc<dyn BlobStorage>),
        ))
    })
    .unwrap();
    store
        .tree("execution_records")
        .insert(b"status", b"ready")
        .unwrap();
    let writer = {
        let page = store.tree("ex:slow-s3");
        tokio::task::spawn_blocking(move || page.insert(b"key", b"payload"))
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.objects.put_started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await
        }
    })
    .await
    .unwrap();
    let status = store.tree("execution_records").get(b"status");
    fixture.objects.hold_put.store(false, Ordering::SeqCst);
    assert_eq!(status.unwrap(), Some(b"ready".to_vec()));
    writer.await.unwrap().unwrap();
}

#[cfg(unix)]
fn local_store(root: &std::path::Path) -> RuntimeStore {
    RuntimeStore::new(Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    )))
    .unwrap()
    .with_blobs(Arc::new(FileHeapStorage::new(root.to_path_buf())))
}

#[cfg(unix)]
#[test]
fn local_collection_reclaims_overwrites_but_respects_readers_and_age() {
    use server::engine::artifacts::ArtifactStore;
    let root = tempfile::tempdir().unwrap();
    let store = local_store(root.path());
    let tree = store.tree("artifacts");
    let artifacts = ArtifactStore::new(tree.clone());
    artifacts.put("result", "text/plain", b"old", None).unwrap();
    let lease = tree.read_lease().unwrap();
    let old = tree.get_with_object(b"result").unwrap().unwrap().1.unwrap();
    artifacts.put("result", "text/plain", b"new", None).unwrap();
    assert!(
        RuntimeStore::collect_runtime_objects(&[store.clone()], Duration::ZERO)
            .unwrap_err()
            .contains("busy")
    );
    assert_eq!(tree.get_object(old.clone()).unwrap(), b"old");
    drop(lease);
    assert_eq!(
        RuntimeStore::collect_runtime_objects(&[store.clone()], Duration::from_secs(3600)).unwrap(),
        0
    );
    assert_eq!(
        RuntimeStore::collect_runtime_objects(&[store.clone()], Duration::ZERO).unwrap(),
        1
    );
    assert!(!root.path().join(old.key()).exists());
    assert_eq!(artifacts.get("result").unwrap().unwrap().bytes, b"new");
    assert_eq!(
        RuntimeStore::collect_runtime_objects(&[store], Duration::ZERO).unwrap(),
        0
    );
}

#[cfg(unix)]
#[test]
fn local_collection_requires_every_metadata_owner_and_retains_shared_objects() {
    use server::engine::artifacts::ArtifactStore;
    let root = tempfile::tempdir().unwrap();
    let a = local_store(root.path());
    let b = local_store(root.path());
    let aa = ArtifactStore::new(a.tree("artifacts"));
    let bb = ArtifactStore::new(b.tree("artifacts"));
    aa.put("a", "text/plain", b"shared", None).unwrap();
    bb.put("b", "text/plain", b"shared", None).unwrap();
    aa.put("a", "text/plain", b"replacement a", None).unwrap();
    let error = RuntimeStore::collect_runtime_objects(&[a.clone()], Duration::ZERO).unwrap_err();
    assert!(error.contains("all registered metadata owners"), "{error}");
    assert_eq!(
        RuntimeStore::collect_runtime_objects(&[a.clone(), b.clone()], Duration::ZERO).unwrap(),
        0
    );
    assert_eq!(bb.get("b").unwrap().unwrap().bytes, b"shared");
    bb.put("b", "text/plain", b"replacement b", None).unwrap();
    assert_eq!(
        RuntimeStore::collect_runtime_objects(&[a, b], Duration::ZERO).unwrap(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn local_collection_refuses_unmanaged_objects() {
    use server::engine::artifacts::ArtifactStore;
    let root = tempfile::tempdir().unwrap();
    let raw = FileHeapStorage::new(root.path().to_path_buf());
    let reference = ObjectRef::identify(ObjectKind::Artifact, b"unknown owner");
    raw.put_immutable(&reference.key(), b"unknown owner")
        .await
        .unwrap();
    let store = local_store(root.path());
    ArtifactStore::new(store.tree("artifacts"))
        .put("a", "text/plain", b"managed", None)
        .unwrap();
    assert!(
        RuntimeStore::collect_runtime_objects(&[store], Duration::ZERO)
            .unwrap_err()
            .contains("unmanaged")
    );
    assert_eq!(raw.get(&reference.key()).await.unwrap(), b"unknown owner");
    assert!(raw.list().await.unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn console_page_snapshots_pin_objects_until_readers_finish() {
    let root = tempfile::tempdir().unwrap();
    let store = local_store(root.path());
    let tree = store.tree("ex:pin");
    tree.insert(b"page", b"old page").unwrap();
    let pages = tree.pages().unwrap();
    tree.insert(b"page", b"new page").unwrap();
    assert!(
        RuntimeStore::collect_runtime_objects(&[store.clone()], Duration::ZERO)
            .unwrap_err()
            .contains("busy")
    );
    assert_eq!(pages[0].read().unwrap(), b"old page");
    drop(pages);
    let iter = tree.iter();
    assert!(
        RuntimeStore::collect_runtime_objects(&[store.clone()], Duration::ZERO)
            .unwrap_err()
            .contains("busy")
    );
    drop(iter);
    assert_eq!(
        RuntimeStore::collect_runtime_objects(&[store], Duration::ZERO).unwrap(),
        1
    );
}

struct FailingBlob;
#[async_trait::async_trait]
impl BlobStorage for FailingBlob {
    async fn put(&self, _: &str, _: &[u8]) -> Result<(), String> {
        Err("injected publication failure".into())
    }
    async fn get(&self, _: &str) -> Result<Vec<u8>, String> {
        Err("missing".into())
    }
    async fn put_immutable(&self, _: &str, _: &[u8]) -> Result<(), String> {
        Err("injected publication failure".into())
    }
}
#[test]
fn console_flush_surfaces_blob_failures_without_publishing_pages() {
    let store = RuntimeStore::new(Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    )))
    .unwrap()
    .with_blobs(Arc::new(FailingBlob));
    let tree = store.tree("ex:failed");
    let state = ConsoleLogState::new(tree.clone());
    state.write(b"tail buffered until execution ends");
    assert!(
        state
            .flush()
            .unwrap_err()
            .contains("injected publication failure")
    );
    assert!(tree.pages().unwrap().is_empty());
    state.write(&vec![b'x'; BLOB_PAGE_SIZE]);
    assert!(
        state
            .flush()
            .unwrap_err()
            .contains("injected publication failure")
    );
    assert!(tree.pages().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn sqlite_runtime_collection_retains_the_current_artifact_after_restart() {
    use server::engine::artifacts::ArtifactStore;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("blobs");
    let open = || {
        RuntimeStore::new(Arc::new(
            mcp_metadata_store::SqliteStore::open(dir.path().join("metadata.sqlite"), "runtime")
                .unwrap(),
        ))
        .unwrap()
        .with_blobs(Arc::new(FileHeapStorage::new(root.clone())))
    };
    {
        let store = open();
        let artifacts = ArtifactStore::new(store.tree("artifacts"));
        artifacts.put("key", "text/plain", b"before", None).unwrap();
        artifacts.put("key", "text/plain", b"after", None).unwrap();
    }
    let store = open();
    assert_eq!(
        RuntimeStore::collect_runtime_objects(&[store.clone()], Duration::ZERO).unwrap(),
        1
    );
    assert_eq!(
        ArtifactStore::new(store.tree("artifacts"))
            .get("key")
            .unwrap()
            .unwrap()
            .bytes,
        b"after"
    );
}

#[cfg(unix)]
#[test]
fn local_collection_refuses_a_symlinked_runtime_directory() {
    use server::engine::artifacts::ArtifactStore;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let store = local_store(root.path());
    ArtifactStore::new(store.tree("artifacts"))
        .put("a", "text/plain", b"current", None)
        .unwrap();
    std::fs::rename(
        root.path().join("runtime"),
        root.path().join("saved-runtime"),
    )
    .unwrap();
    let orphan = ObjectRef::identify(ObjectKind::Artifact, b"outside");
    let path = outside
        .path()
        .join("v2/artifact")
        .join(orphan.key().rsplit('/').next().unwrap());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"outside").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("runtime")).unwrap();
    assert!(
        RuntimeStore::collect_runtime_objects(&[store], Duration::ZERO)
            .unwrap_err()
            .contains("symlink")
    );
    assert_eq!(std::fs::read(path).unwrap(), b"outside");
}
