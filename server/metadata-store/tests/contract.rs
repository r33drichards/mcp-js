use aws_sdk_dynamodb::{
    Client,
    config::{Credentials, Region},
    types::{AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType},
};
use mcp_metadata_store::{DynamoDbStore, MetadataStore, SledStore, SqliteStore, Write};
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
    tokio::task::spawn_blocking(move || runtime_contract(runtime_store)).await.unwrap();
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
