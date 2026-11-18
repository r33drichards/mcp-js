use mcp_metadata_store::blob::BlobStorage;
use server::engine::heap_storage::{
    FileHeapStorage, MemoryHeapStorage, WriteThroughCacheHeapStorage,
};
use std::sync::Arc;

async fn immutable_contract(backend: Arc<dyn BlobStorage>) {
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let b = backend.clone();
        tasks.push(tokio::spawn(async move {
            b.put_immutable("objects/v1/key", b"same").await.unwrap();
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert!(
        backend
            .put_immutable("objects/v1/key", b"different")
            .await
            .is_err()
    );
    assert_eq!(backend.get("objects/v1/key").await.unwrap(), b"same");
}
#[tokio::test]
async fn immutable_memory_and_directory_contract() {
    immutable_contract(Arc::new(MemoryHeapStorage::new())).await;
    let dir = tempfile::tempdir().unwrap();
    immutable_contract(Arc::new(FileHeapStorage::new(dir.path()))).await;
    assert_eq!(
        FileHeapStorage::new(dir.path())
            .get("objects/v1/key")
            .await
            .unwrap(),
        b"same"
    );
    assert_eq!(
        std::fs::read_dir(dir.path().join("objects/v1"))
            .unwrap()
            .count(),
        1
    );
}
#[tokio::test]
async fn failed_primary_write_does_not_publish_a_cache_entry() {
    let dir = tempfile::tempdir().unwrap();
    let invalid = dir.path().join("not-a-directory");
    std::fs::write(&invalid, b"file").unwrap();
    let cached =
        WriteThroughCacheHeapStorage::new(FileHeapStorage::new(invalid), dir.path().join("cache"));
    assert!(cached.put("key", b"uncommitted").await.is_err());
    assert!(cached.get("key").await.is_err());
    assert!(
        cached
            .put_immutable("objects/key", b"uncommitted")
            .await
            .is_err()
    );
    assert!(!dir.path().join("cache/objects/key").exists());
}

#[path = "support/s3.rs"]
mod s3;
#[tokio::test]
async fn immutable_s3_contract_uses_conditional_writes() {
    let fixture = s3::S3::start().await;
    let config = aws_sdk_s3::config::Builder::new()
        .behavior_version(aws_config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_s3::config::Credentials::new(
            "test", "test", None, None, "contract",
        ))
        .endpoint_url(&fixture.endpoint)
        .force_path_style(true)
        .build();
    let store = server::engine::heap_storage::S3HeapStorage::from_client(
        "runtime-test",
        aws_sdk_s3::Client::from_conf(config),
    );
    immutable_contract(Arc::new(store.clone())).await;
    fixture
        .objects
        .fail_put
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        store
            .put_immutable("new", b"must not be published")
            .await
            .is_err()
    );
    assert!(
        !fixture
            .objects
            .values
            .lock()
            .unwrap()
            .contains_key("/runtime-test/new")
    );
}

#[tokio::test]
async fn artifact_metadata_changes_deduplicate_payload_and_listing_needs_no_blob_reads() {
    use mcp_metadata_store::{RuntimeStore, SledStore};
    use server::engine::artifacts::ArtifactStore;
    let backend = MemoryHeapStorage::new();
    let db = Arc::new(SledStore(
        sled::Config::new().temporary(true).open().unwrap(),
    ));
    let tree = RuntimeStore::new(db)
        .unwrap()
        .with_blobs(Arc::new(backend.clone()))
        .tree("artifacts");
    let artifacts = ArtifactStore::new(tree);
    artifacts
        .put("a", "text/plain", b"same bytes", Some("first"))
        .unwrap();
    artifacts
        .put(
            "b",
            "application/octet-stream",
            b"same bytes",
            Some("second"),
        )
        .unwrap();
    let keys = backend.list().await.unwrap();
    assert_eq!(keys.len(), 1, "metadata must not change payload identity");
    assert_eq!(artifacts.get("a").unwrap().unwrap().bytes, b"same bytes");
    backend.delete(&keys[0]).await.unwrap();
    let metadata = artifacts.list().unwrap();
    assert_eq!(metadata.len(), 2);
    assert_eq!(metadata[0].size_bytes, 10);
    assert!(
        artifacts.get("a").is_err(),
        "missing blobs must not fall back"
    );
}
