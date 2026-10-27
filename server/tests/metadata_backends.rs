//! Application-level contract for the actual session/tag/label consumers.
use mcp_metadata_store::{DynamoDbStore, MetadataStore, SledStore};
use server::engine::{
    fs_labels::LabelStore,
    heap_tags::HeapTagStore,
    session_log::{SessionLog, SessionLogEntry},
};
use std::{collections::HashMap, sync::Arc};

async fn exercise(
    sessions: Arc<dyn MetadataStore>,
    tags: Arc<dyn MetadataStore>,
    labels: Arc<dyn MetadataStore>,
) {
    let log = SessionLog::from_store(sessions.clone());
    log.append(
        "parent",
        SessionLogEntry {
            input_heap: None,
            output_heap: "heap-1".into(),
            output_fs: Some("fs-1".into()),
            code: "42".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
        },
    )
    .await
    .unwrap();
    log.fork("parent", "child").await.unwrap();
    let reopened = SessionLog::from_store(sessions);
    assert_eq!(
        reopened
            .get_latest("child")
            .await
            .unwrap()
            .unwrap()
            .output_fs
            .as_deref(),
        Some("fs-1")
    );
    assert_eq!(reopened.list_entries("parent").await.unwrap().len(), 1);
    assert!(
        reopened
            .list_sessions()
            .await
            .unwrap()
            .contains(&"child".into())
    );

    let mut tasks = Vec::new();
    for i in 0..8 {
        let store = HeapTagStore::from_store(tags.clone());
        tasks.push(tokio::spawn(async move {
            store
                .merge_tags(
                    "heap",
                    HashMap::from([(format!("key-{i}"), "value".into())]),
                )
                .await
                .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let tags = HeapTagStore::from_store(tags);
    assert_eq!(tags.get_tags("heap").await.unwrap().len(), 8);
    assert_eq!(
        tags.query_by_tags(HashMap::from([("key-0".into(), "value".into())]))
            .await
            .unwrap()
            .len(),
        1
    );

    let store = LabelStore::from_store(labels.clone());
    store.create("main", [1; 32], None).await.unwrap();
    assert!(store.create("main", [2; 32], None).await.is_err());
    assert!(
        !store
            .cas("main", Some([9; 32]), [3; 32], None)
            .await
            .unwrap()
    );
    assert_eq!(store.log("main").await.unwrap().len(), 1);
    let mut tasks = Vec::new();
    for i in 2..10 {
        let store = LabelStore::from_store(labels.clone());
        tasks.push(tokio::spawn(async move {
            store
                .cas("main", Some([1; 32]), [i; 32], None)
                .await
                .unwrap()
        }));
    }
    let mut wins = 0;
    for task in tasks {
        wins += usize::from(task.await.unwrap());
    }
    assert_eq!(wins, 1);
    assert_eq!(store.log("main").await.unwrap().len(), 2);
    store
        .force("main", [42; 32], Some("reset".into()))
        .await
        .unwrap();
    let reopened = LabelStore::from_store(labels);
    assert_eq!(reopened.resolve("main").await.unwrap(), Some([42; 32]));
    assert_eq!(
        reopened.log_recent("main", 1).await.unwrap()[0].to,
        [42; 32]
    );
    assert_eq!(
        reopened.list().await.unwrap(),
        vec![("main".into(), [42; 32])]
    );
}

#[tokio::test]
async fn sled_consumers() {
    let db = || -> Arc<dyn MetadataStore> {
        Arc::new(SledStore(
            sled::Config::new().temporary(true).open().unwrap(),
        ))
    };
    exercise(db(), db(), db()).await;
}

#[tokio::test]
#[ignore = "requires loopback DynamoDB emulator"]
async fn dynamodb_consumers() {
    use aws_sdk_dynamodb::{
        config::{Credentials, Region},
        types::{AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType},
    };
    let endpoint = std::env::var("DYNAMODB_TEST_ENDPOINT").expect("set DYNAMODB_TEST_ENDPOINT");
    assert!(endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"));
    let config = aws_sdk_dynamodb::config::Builder::new()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("test", "test", None, None, "test"))
        .endpoint_url(endpoint)
        .build();
    let client = aws_sdk_dynamodb::Client::from_conf(config);
    let table = format!("mcp-consumers-{}", uuid::Uuid::new_v4());
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
        if client
            .describe_table()
            .table_name(&table)
            .send()
            .await
            .unwrap()
            .table()
            .and_then(|t| t.table_status())
            .is_some_and(|s| s.as_str() == "ACTIVE")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let db = |ns| -> Arc<dyn MetadataStore> {
        Arc::new(DynamoDbStore::new(client.clone(), &table, ns).unwrap())
    };
    exercise(db("sessions"), db("tags"), db("labels")).await;
    client
        .delete_table()
        .table_name(&table)
        .send()
        .await
        .unwrap();
}
