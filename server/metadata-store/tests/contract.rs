#[cfg(feature="dynamodb")]
use aws_sdk_dynamodb::{
    Client,
    config::{Credentials, Region},
    types::{AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType},
};
use mcp_metadata_store::{MetadataStore, SledStore, SqliteStore, Write};
#[cfg(feature="dynamodb")]
use mcp_metadata_store::DynamoDbStore;
use std::sync::Arc;

async fn contract(store: Arc<dyn MetadataStore>) {
    assert_eq!(store.get("heads", b"main").await.unwrap(), None);
    assert!(
        store
            .compare_and_swap(
                Write::new("heads", b"main", b"one"),
                None,
                vec![Write::new("log", b"main\0first", b"created")]
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .compare_and_swap(
                Write::new("heads", b"main", b"bad"),
                None,
                vec![Write::new("log", b"main\0bad", b"must not exist")]
            )
            .await
            .unwrap()
    );
    assert_eq!(store.get("log", b"main\0bad").await.unwrap(), None);
    assert_eq!(
        store.get("heads", b"main").await.unwrap(),
        Some(b"one".to_vec())
    );
    // Reject malformed atomic batches before changing the primary.
    assert!(
        store
            .compare_and_swap(
                Write::new("heads", b"main", b"bad"),
                Some(b"one".to_vec()),
                vec![Write::new("heads", b"main", b"duplicate")]
            )
            .await
            .is_err()
    );
    assert_eq!(
        store.get("heads", b"main").await.unwrap(),
        Some(b"one".to_vec())
    );
    let mut tasks = Vec::new();
    for n in 0..8 {
        let s = store.clone();
        tasks.push(tokio::spawn(async move {
            let key = format!("winner-{n}");
            s.compare_and_swap(
                Write::new("heads", b"main", key.as_bytes()),
                Some(b"one".to_vec()),
                vec![Write::new("race-log", key.as_bytes(), b"committed")],
            )
            .await
            .unwrap()
        }));
    }
    let mut wins = 0;
    for task in tasks {
        wins += usize::from(task.await.unwrap());
    }
    assert_eq!(wins, 1);
    assert_eq!(
        store
            .scan("race-log", b"", false, None)
            .await
            .unwrap()
            .len(),
        1
    );
    // Binary keys, prefix isolation, bounded reverse reads, and page traversal.
    let payload = vec![42; 30_000];
    for n in 0u16..40 {
        let mut key = b"p\0".to_vec();
        key.extend_from_slice(&n.to_be_bytes());
        store
            .put(Write::new("pages", key, payload.clone()))
            .await
            .unwrap();
    }
    store
        .put(Write::new("pages", b"p2", b"excluded"))
        .await
        .unwrap();
    let rows = store.scan("pages", b"p\0", false, None).await.unwrap();
    assert_eq!(rows.len(), 40); // >1 MiB: must follow LastEvaluatedKey.
    assert!(rows.windows(2).all(|w| w[0].0 < w[1].0));
    let tail = store.scan("pages", b"p\0", true, Some(2)).await.unwrap();
    assert_eq!(tail[0], rows[39]);
    assert_eq!(tail[1], rows[38]);
    assert!(
        store
            .scan("pages", b"", false, Some(0))
            .await
            .unwrap()
            .is_empty()
    );
    let a = store.generate_id().await.unwrap();
    let b = store.generate_id().await.unwrap();
    assert!(b > a);
    assert!(store.tree_names().await.unwrap().contains(&"heads".into()));
    store.flush().unwrap();
}

#[tokio::test]
async fn sled_contract() {
    let dir = tempfile::tempdir().unwrap();
    let store = SledStore(sled::open(dir.path()).unwrap());
    contract(Arc::new(store)).await;
    // Durable reopen, not just a second handle to memory.
    let reopened = SledStore(sled::open(dir.path()).unwrap());
    assert!(reopened.get("heads", b"main").await.unwrap().is_some());
}

#[tokio::test]
async fn sqlite_contract() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite");
    contract(Arc::new(SqliteStore::open(&path, "test").unwrap())).await;
    let reopened = SqliteStore::open(&path, "test").unwrap();
    assert!(reopened.get("heads", b"main").await.unwrap().is_some());
    let id = reopened.generate_id().await.unwrap();
    drop(reopened);
    assert!(
        SqliteStore::open(&path, "test")
            .unwrap()
            .generate_id()
            .await
            .unwrap()
            > id
    );
    let other = SqliteStore::open(&path, "other").unwrap();
    assert_eq!(other.get("heads", b"main").await.unwrap(), None);
    assert!(other.tree_names().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sqlite_independent_connections_serialize_cas_and_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite");
    let stores: Vec<_> = (0..8)
        .map(|_| SqliteStore::open(&path, "test").unwrap())
        .collect();
    let mut tasks = Vec::new();
    for (n, store) in stores.into_iter().enumerate() {
        tasks.push(tokio::spawn(async move {
            let won = store
                .compare_and_swap(
                    Write::new("heads", b"key", vec![n as u8]),
                    None,
                    vec![Write::new("log", vec![n as u8], b"won")],
                )
                .await
                .unwrap();
            (won, store.generate_id().await.unwrap())
        }));
    }
    let mut wins = 0;
    let mut ids = std::collections::BTreeSet::new();
    for task in tasks {
        let (won, id) = task.await.unwrap();
        wins += usize::from(won);
        assert!(ids.insert(id));
    }
    assert_eq!(wins, 1);
    let store = SqliteStore::open(&path, "test").unwrap();
    assert_eq!(store.scan("log", b"", false, None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn sqlite_binary_prefix_boundaries_and_storage_error_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite");
    let store = SqliteStore::open(&path, "test").unwrap();
    for key in [
        vec![],
        vec![0],
        vec![0, 255],
        vec![1],
        vec![255],
        vec![255, 0],
        vec![255, 255],
    ] {
        store
            .put(Write::new("binary", key, b"value"))
            .await
            .unwrap();
    }
    for prefix in [vec![], vec![0], vec![0, 255], vec![255], vec![255, 255]] {
        let all = store.scan("binary", b"", false, None).await.unwrap();
        let expected: Vec<_> = all
            .into_iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .collect();
        assert_eq!(
            store.scan("binary", &prefix, false, None).await.unwrap(),
            expected
        );
        let reversed: Vec<_> = expected.into_iter().rev().take(1).collect();
        assert_eq!(
            store.scan("binary", &prefix, true, Some(1)).await.unwrap(),
            reversed
        );
    }
    // Force a real SQLite error after the primary write, verifying rollback.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_log BEFORE INSERT ON metadata
        WHEN NEW.tree='fail' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
    assert!(
        store
            .compare_and_swap(
                Write::new("heads", b"main", b"new"),
                None,
                vec![Write::new("fail", b"entry", b"bad")]
            )
            .await
            .is_err()
    );
    assert_eq!(store.get("heads", b"main").await.unwrap(), None);
    assert!(SqliteStore::open(dir.path().join("missing").join("db"), "test").is_err());
}

#[cfg(feature="dynamodb")]
async fn dynamodb_contract(endpoint: String) {
    // Explicit fake credentials and endpoint: these tests never use AWS IAM.
    assert!(endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"));
    let config = aws_sdk_dynamodb::config::Builder::new()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("test", "test", None, None, "local-test"))
        .endpoint_url(endpoint)
        .build();
    let client = Client::from_conf(config);
    let table = format!(
        "metadata-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    client
        .create_table()
        .table_name(&table)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("sk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("sk")
                .key_type(KeyType::Range)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .unwrap();
    for _ in 0..100 {
        let description = client
            .describe_table()
            .table_name(&table)
            .send()
            .await
            .unwrap();
        if description
            .table()
            .and_then(|t| t.table_status())
            .is_some_and(|s| s.as_str() == "ACTIVE")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    contract(Arc::new(
        DynamoDbStore::new(client.clone(), &table, "test").unwrap(),
    ))
    .await;
    let runtime_store = Arc::new(DynamoDbStore::new(client.clone(), &table, "runtime-test").unwrap());
    {
        use mcp_metadata_store::publication::{MetadataPublication,Publication,PublishOutcome};
        let publication=MetadataPublication::new(runtime_store.clone(),"contract").unwrap();
        let mut request=Publication {operation_id:"once".into(),key:b"head".to_vec(),expected_revision:None,value:Some(vec![7;200_000]),companions:vec![Write::new("reflog",b"entry",b"event")]};
        assert_eq!(publication.publish(&request).await.unwrap(),PublishOutcome::Committed {revision:1});
        assert_eq!(publication.publish(&request).await.unwrap(),PublishOutcome::Committed {revision:1});
        request.operation_id="oversized".into();request.key=b"large".to_vec();request.value=Some(vec![0;500_000]);
        assert!(publication.publish(&request).await.unwrap_err().contains("size limit exceeded"));
    }
    tokio::task::spawn_blocking(move || {
        runtime_contract(runtime_store.clone());
        let blobs = Arc::new(TestBlobs::default());
        let runtime = mcp_metadata_store::RuntimeStore::new(runtime_store)
            .unwrap().with_blobs(blobs.clone());
        let tree = runtime.tree("runtime-size-limit");
        // Above the old threshold but below DynamoDB's native item limit.
        tree.insert(b"metadata", vec![7; 200_000]).unwrap();
        let error = tree.insert(b"large", vec![0; 500_000]).unwrap_err();
        assert!(error.contains("size limit exceeded"), "{error}");
        assert!(tree.get(b"large").unwrap().is_none());
        assert!(tree.compare_and_swap(b"metadata", Some(vec![7; 200_000]), Some(vec![0; 500_000])).is_err());
        assert_eq!(tree.get(b"metadata").unwrap(), Some(vec![7; 200_000]));
        assert!(blobs.0.lock().unwrap().is_empty(), "metadata must never spill to S3");
        for name in ["artifacts", "ex:test"] {
            let payloads = runtime.tree(name);
            for bytes in [b"tiny".to_vec(), vec![8; 500_000]] {
                payloads.insert(b"key", &bytes).unwrap();
                assert_eq!(payloads.get(b"key").unwrap(), Some(bytes));
            }
        }
    }).await.unwrap();
    let reopened = DynamoDbStore::new(client.clone(), &table, "test").unwrap();
    assert!(reopened.get("heads", b"main").await.unwrap().is_some());
    let other = DynamoDbStore::new(client.clone(), &table, "test-other").unwrap();
    assert_eq!(other.get("heads", b"main").await.unwrap(), None);
    assert!(other.tree_names().await.unwrap().is_empty());
    // Service errors are not absence or CAS conflicts.
    let missing = DynamoDbStore::new(client.clone(), "does-not-exist", "test").unwrap();
    assert!(missing.get("heads", b"main").await.is_err());
    assert!(
        missing
            .compare_and_swap(Write::new("heads", b"x", b"x"), None, vec![])
            .await
            .is_err()
    );
    assert!(
        reopened
            .put(Write::new("oversize", b"x", vec![0; 400 * 1024]))
            .await
            .is_err()
    );
    client
        .delete_table()
        .table_name(&table)
        .send()
        .await
        .unwrap();
}

#[tokio::test]
#[cfg(feature="dynamodb")]
#[ignore = "requires a loopback DynamoDB Local/LocalStack endpoint"]
async fn dynamodb_endpoint_contract() {
    dynamodb_contract(std::env::var("DYNAMODB_TEST_ENDPOINT").expect("set DYNAMODB_TEST_ENDPOINT"))
        .await;
}

#[cfg(feature = "testcontainers-tests")]
#[tokio::test]
async fn localstack_testcontainer_contract() {
    use testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };
    let container = GenericImage::new("localstack/localstack", "4.7.0")
        .with_exposed_port(4566.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready."))
        .with_env_var("SERVICES", "dynamodb")
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(4566.tcp()).await.unwrap();
    dynamodb_contract(format!("http://127.0.0.1:{port}")).await;
    // Container lifecycle is owned by Testcontainers, including panic cleanup.
}

fn runtime_contract(store: Arc<dyn MetadataStore>) {
    use mcp_metadata_store::RuntimeStore;
    let runtime = RuntimeStore::new(store.clone()).unwrap();
    let tree = runtime.tree("runtime-contract");
    tree.insert(b"a", b"first").unwrap();
    let reopened = RuntimeStore::new(store).unwrap().tree("runtime-contract");
    assert_eq!(reopened.get(b"a").unwrap(), Some(b"first".to_vec()));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let t = reopened.clone();
        handles.push(std::thread::spawn(move || t.compare_and_swap(b"a", Some(b"first".to_vec()), Some(b"claimed".to_vec())).unwrap().is_ok()));
    }
    assert_eq!(handles.into_iter().map(|h| usize::from(h.join().unwrap())).sum::<usize>(), 1);
    // A second value and a tombstone must not appear as a phantom scan row.
    tree.insert(b"b", b"second").unwrap();
    tree.remove(b"b").unwrap();
    assert!(tree.get(b"b").unwrap().is_none());
    assert_eq!(tree.iter().collect::<Result<Vec<_>,_>>().unwrap().len(), 1);
    assert!(tree.compare_and_swap(b"b", None, Some(b"reborn".to_vec())).unwrap().is_ok());
    assert_eq!(tree.get(b"b").unwrap(), Some(b"reborn".to_vec()));
}

#[test]
fn runtime_sled_contract() {
    let db = sled::Config::new().temporary(true).open().unwrap();
    runtime_contract(Arc::new(SledStore(db)));
}

#[derive(Default)]
struct TestBlobs(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);
#[async_trait::async_trait]
impl mcp_metadata_store::runtime_tree::RuntimeBlobs for TestBlobs {
    async fn put_immutable(&self, name: &str, value: &[u8]) -> Result<(), String> {
        let mut objects = self.0.lock().unwrap();
        match objects.get(name) {
            Some(existing) if existing != value => Err("immutable conflict".into()),
            Some(_) => Ok(()),
            None => { objects.insert(name.into(), value.to_vec()); Ok(()) }
        }
    }
    async fn put(&self, name: &str, value: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().insert(name.into(), value.to_vec()); Ok(())
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        self.0.lock().unwrap().get(name).cloned().ok_or("missing runtime blob".into())
    }
}
#[tokio::test(flavor="current_thread")]
async fn runtime_artifact_values_are_blob_references_and_missing_blobs_fail() {
    use mcp_metadata_store::RuntimeStore;
    let db = Arc::new(SledStore(sled::Config::new().temporary(true).open().unwrap()));
    let blobs = Arc::new(TestBlobs::default());
    let tree = RuntimeStore::new(db.clone()).unwrap().with_blobs(blobs.clone()).tree("artifacts");
    let value = vec![42; 500_000];
    tree.insert(b"artifact", &value).unwrap();
    assert_eq!(tree.get(b"artifact").unwrap(), Some(value.clone()));
    let raw = db.get("artifacts", b"artifact").await.unwrap().unwrap();
    assert_eq!(raw[0], 1); assert!(raw.len() < 100);
    let reopened = RuntimeStore::new(db).unwrap().with_blobs(blobs.clone()).tree("artifacts");
    assert_eq!(reopened.get(b"artifact").unwrap(), Some(value));
    blobs.0.lock().unwrap().clear();
    assert!(reopened.get(b"artifact").is_err());
}

#[test]
fn runtime_sqlite_contract() {
    let dir = tempfile::tempdir().unwrap();
    runtime_contract(Arc::new(
        SqliteStore::open(dir.path().join("metadata.sqlite"), "runtime").unwrap(),
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_sqlite_metadata_reopen_and_cas_without_blob_storage() {
    use mcp_metadata_store::RuntimeStore;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite");
    let store = Arc::new(SqliteStore::open(&path, "runtime").unwrap());
    let tree = RuntimeStore::new(store.clone()).unwrap().tree("execution_records");
    let first = vec![42; 500_000];
    tree.insert(b"large", &first).unwrap();
    drop(tree);

    // A fresh connection must read the payload from SQLite, with no blob client.
    let reopened = RuntimeStore::new(Arc::new(SqliteStore::open(&path, "runtime").unwrap()))
        .unwrap()
        .tree("execution_records");
    assert_eq!(reopened.get(b"large").unwrap(), Some(first.clone()));
    let second = vec![43; 600_000];
    assert!(
        reopened
            .compare_and_swap(b"large", Some(first.clone()), Some(second.clone()))
            .unwrap()
            .is_ok()
    );
    assert!(
        reopened
            .compare_and_swap(b"large", Some(first), Some(vec![0]))
            .unwrap()
            .is_err()
    );
    assert_eq!(
        reopened.iter().collect::<Result<Vec<_>, _>>().unwrap(),
        vec![(b"large".to_vec(), second.clone())]
    );
    let raw = store.get("execution_records", b"large").await.unwrap().unwrap();
    assert_eq!(
        raw[0], 1,
        "metadata remains inline, subject to SQLite limits"
    );
    assert_eq!(&raw[1..], second);
    reopened.remove(b"large").unwrap();
    assert!(reopened.get(b"large").unwrap().is_none());
    assert!(
        reopened
            .compare_and_swap(b"large", None, Some(second.clone()))
            .unwrap()
            .is_ok()
    );
    assert_eq!(reopened.get(b"large").unwrap(), Some(second));
    assert!(
        SqliteStore::open(&path, "other")
            .unwrap()
            .get("execution_records", b"large")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_payloads_always_use_blobs_and_metadata_never_spills() {
    use mcp_metadata_store::RuntimeStore;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.sqlite");
    let db = Arc::new(SqliteStore::open(&path, "runtime").unwrap());
    let blobs = Arc::new(TestBlobs::default());
    let store = RuntimeStore::new(db.clone()).unwrap().with_blobs(blobs.clone());
    for name in ["artifacts", "ex:stdout", "ex:stderr"] {
        let tree = store.tree(name);
        // Even empty and tiny payloads must live in blob storage.
        for payload in [vec![], b"tiny".to_vec(), vec![42; 500_000]] {
            tree.insert(b"key", &payload).unwrap();
            let raw = db.get(name, b"key").await.unwrap().unwrap();
            assert!(raw.len() < 128, "payload was stored in the database");
            assert_ne!(raw, [vec![1], payload.clone()].concat());
            let reopened = RuntimeStore::new(Arc::new(SqliteStore::open(&path, "runtime").unwrap()))
                .unwrap().with_blobs(blobs.clone()).tree(name);
            assert_eq!(reopened.get(b"key").unwrap(), Some(payload));
        }
    }
    let metadata = store.tree("execution_records");
    metadata.insert(b"key", b"small").unwrap();
    let count = blobs.0.lock().unwrap().len();
    metadata.insert(b"key", vec![0; 500_000]).unwrap();
    assert_eq!(metadata.get(b"key").unwrap(), Some(vec![0; 500_000]));
    assert_eq!(blobs.0.lock().unwrap().len(), count, "metadata must never spill to S3");
    let no_blobs = RuntimeStore::new(db.clone()).unwrap().tree("artifacts");
    assert!(no_blobs.insert(b"missing", b"tiny").is_err());
    assert!(db.get("artifacts", b"missing").await.unwrap().is_none());
    blobs.0.lock().unwrap().clear();
    assert!(store.tree("artifacts").get(b"key").is_err());
}

#[tokio::test(flavor="current_thread")]
async fn legacy_uuid_and_dynamodb_blob_references_remain_readable() {
    use mcp_metadata_store::{RuntimeStore,Write};
    let backend=Arc::new(TestBlobs::default());
    let db=Arc::new(SledStore(sled::Config::new().temporary(true).open().unwrap()));
    let name="runtime/legacy-object";
    mcp_metadata_store::blob::BlobStorage::put(backend.as_ref(),name,b"old payload").await.unwrap();
    let mut inline=vec![1];inline.extend(b"\0mcp-runtime-blob-v1\0");inline.extend(name.as_bytes());
    db.put(Write::new("artifacts",b"uuid",inline)).await.unwrap();
    let mut old_ddb=vec![2];old_ddb.extend(name.as_bytes());
    db.put(Write::new("artifacts",b"ddb",old_ddb)).await.unwrap();
    let tree=RuntimeStore::new(db).unwrap().with_blobs(backend).tree("artifacts");
    assert_eq!(tree.get(b"uuid").unwrap(),Some(b"old payload".to_vec()));
    assert_eq!(tree.get(b"ddb").unwrap(),Some(b"old payload".to_vec()));
}
