use server::engine::session_log::{SessionLog, SessionLogEntry};
use std::sync::Arc;

fn make_entry(input: Option<&str>, output: &str, code: &str) -> SessionLogEntry {
    SessionLogEntry {
        input_heap: input.map(|s| s.to_string()),
        output_heap: output.to_string(),
        code: code.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

fn temp_session_log() -> SessionLog {
    SessionLog::from_config(sled::Config::new().temporary(true)).expect("failed to open temp sled")
}

#[tokio::test]
async fn test_basic_append_and_list() {
    let log = temp_session_log();

    // Initially no sessions
    let sessions = log.list_sessions().await.unwrap();
    assert!(sessions.is_empty());

    // Append to a session
    let seq = log
        .append("my-session", make_entry(None, "hash_a", "1 + 1"))
        .await
        .unwrap();
    assert!(seq > 0 || seq == 0); // just check it returns

    // Session should now appear
    let sessions = log.list_sessions().await.unwrap();
    assert_eq!(sessions, vec!["my-session"]);

    // Append another entry
    log.append(
        "my-session",
        make_entry(Some("hash_a"), "hash_b", "2 + 2"),
    )
    .await
    .unwrap();

    // List entries
    let entries = log.list_entries("my-session", None).await.unwrap();
    assert_eq!(entries.len(), 2);

    // First entry
    assert_eq!(entries[0]["input_heap"], serde_json::Value::Null);
    assert_eq!(entries[0]["output_heap"], "hash_a");
    assert_eq!(entries[0]["code"], "1 + 1");

    // Second entry
    assert_eq!(entries[1]["input_heap"], "hash_a");
    assert_eq!(entries[1]["output_heap"], "hash_b");
    assert_eq!(entries[1]["code"], "2 + 2");
}

#[tokio::test]
async fn test_multiple_sessions() {
    let log = temp_session_log();

    log.append("session-a", make_entry(None, "h1", "code1"))
        .await
        .unwrap();
    log.append("session-b", make_entry(None, "h2", "code2"))
        .await
        .unwrap();
    log.append("session-a", make_entry(Some("h1"), "h3", "code3"))
        .await
        .unwrap();

    let mut sessions = log.list_sessions().await.unwrap();
    sessions.sort();
    assert_eq!(sessions, vec!["session-a", "session-b"]);

    let a_entries = log.list_entries("session-a", None).await.unwrap();
    assert_eq!(a_entries.len(), 2);

    let b_entries = log.list_entries("session-b", None).await.unwrap();
    assert_eq!(b_entries.len(), 1);
}

#[tokio::test]
async fn test_get_latest() {
    let log = temp_session_log();

    // No entries yet
    assert!(log.get_latest("empty").await.unwrap().is_none());

    log.append("s", make_entry(None, "h1", "c1")).await.unwrap();
    log.append("s", make_entry(Some("h1"), "h2", "c2")).await.unwrap();
    log.append("s", make_entry(Some("h2"), "h3", "c3")).await.unwrap();

    let latest = log.get_latest("s").await.unwrap().unwrap();
    assert_eq!(latest.output_heap, "h3");
    assert_eq!(latest.code, "c3");
}

#[tokio::test]
async fn test_field_filtering() {
    let log = temp_session_log();

    log.append("s", make_entry(None, "h1", "code1")).await.unwrap();
    log.append("s", make_entry(Some("h1"), "h2", "code2"))
        .await
        .unwrap();

    // Request only output_heap and code
    let entries = log
        .list_entries(
            "s",
            Some(vec!["output_heap".to_string(), "code".to_string()]),
        )
        .await
        .unwrap();

    assert_eq!(entries.len(), 2);

    // Should have only the requested fields
    let obj = entries[0].as_object().unwrap();
    assert!(obj.contains_key("output_heap"));
    assert!(obj.contains_key("code"));
    assert!(!obj.contains_key("input_heap"));
    assert!(!obj.contains_key("timestamp"));
    assert!(!obj.contains_key("index"));

    // Request with index
    let entries = log
        .list_entries("s", Some(vec!["index".to_string(), "output_heap".to_string()]))
        .await
        .unwrap();
    let obj = entries[0].as_object().unwrap();
    assert!(obj.contains_key("index"));
    assert!(obj.contains_key("output_heap"));
    assert!(!obj.contains_key("code"));
}

#[tokio::test]
async fn test_empty_session_entries() {
    let log = temp_session_log();

    // Listing entries for a non-existent session should return empty (open_tree creates it lazily)
    let entries = log.list_entries("nonexistent", None).await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn test_monotonic_sequence_numbers() {
    let log = temp_session_log();

    let mut seqs = Vec::new();
    for i in 0..10 {
        let seq = log
            .append("s", make_entry(None, &format!("h{}", i), &format!("c{}", i)))
            .await
            .unwrap();
        seqs.push(seq);
    }

    // Sequence numbers should be strictly increasing
    for i in 1..seqs.len() {
        assert!(seqs[i] > seqs[i - 1], "seq[{}]={} should be > seq[{}]={}", i, seqs[i], i - 1, seqs[i - 1]);
    }
}

#[tokio::test]
async fn test_concurrent_writes_same_session() {
    let log = Arc::new(temp_session_log());
    let num_tasks = 50;

    let mut handles = Vec::new();
    for i in 0..num_tasks {
        let log = log.clone();
        handles.push(tokio::spawn(async move {
            log.append(
                "concurrent-session",
                make_entry(None, &format!("hash_{}", i), &format!("code_{}", i)),
            )
            .await
            .unwrap()
        }));
    }

    let mut seqs: Vec<u64> = Vec::new();
    for handle in handles {
        seqs.push(handle.await.unwrap());
    }

    // All sequence numbers should be unique
    seqs.sort();
    seqs.dedup();
    assert_eq!(seqs.len(), num_tasks);

    // All entries should be present
    let entries = log.list_entries("concurrent-session", None).await.unwrap();
    assert_eq!(entries.len(), num_tasks);

    // Entries should be in sequence order (sled iterates by key order = big-endian u64)
    let indices: Vec<u64> = entries
        .iter()
        .map(|e| e["index"].as_u64().unwrap())
        .collect();
    for i in 1..indices.len() {
        assert!(indices[i] > indices[i - 1]);
    }
}

#[tokio::test]
async fn test_concurrent_writes_different_sessions() {
    let log = Arc::new(temp_session_log());
    let num_sessions = 10;
    let entries_per_session = 20;

    let mut handles = Vec::new();
    for s in 0..num_sessions {
        for i in 0..entries_per_session {
            let log = log.clone();
            let session_name = format!("session_{}", s);
            handles.push(tokio::spawn(async move {
                log.append(
                    &session_name,
                    make_entry(None, &format!("h_{}_{}", s, i), &format!("c_{}_{}", s, i)),
                )
                .await
                .unwrap()
            }));
        }
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Each session should have exactly entries_per_session entries
    for s in 0..num_sessions {
        let entries = log
            .list_entries(&format!("session_{}", s), None)
            .await
            .unwrap();
        assert_eq!(
            entries.len(),
            entries_per_session,
            "session_{} should have {} entries, got {}",
            s,
            entries_per_session,
            entries.len()
        );
    }

    let mut sessions = log.list_sessions().await.unwrap();
    sessions.sort();
    assert_eq!(sessions.len(), num_sessions);
}

#[tokio::test]
async fn test_read_write_atomicity() {
    // One task writes entries while another reads them.
    // Reads should never see partial/corrupted entries.
    let log = Arc::new(temp_session_log());
    let num_writes = 100;

    let writer_log = log.clone();
    let writer = tokio::spawn(async move {
        for i in 0..num_writes {
            writer_log
                .append(
                    "atomic-session",
                    make_entry(
                        Some(&format!("input_{}", i)),
                        &format!("output_{}", i),
                        &format!("code_{}", i),
                    ),
                )
                .await
                .unwrap();
        }
    });

    let reader_log = log.clone();
    let reader = tokio::spawn(async move {
        let mut max_seen = 0;
        for _ in 0..200 {
            let entries = reader_log
                .list_entries("atomic-session", None)
                .await
                .unwrap();

            // Entry count should only increase
            assert!(
                entries.len() >= max_seen,
                "entry count decreased from {} to {}",
                max_seen,
                entries.len()
            );
            max_seen = entries.len();

            // Each entry should be complete and valid
            for entry in &entries {
                let obj = entry.as_object().unwrap();
                assert!(obj.contains_key("index"));
                assert!(obj.contains_key("input_heap"));
                assert!(obj.contains_key("output_heap"));
                assert!(obj.contains_key("code"));
                assert!(obj.contains_key("timestamp"));

                // output_heap and code should be non-empty strings
                assert!(obj["output_heap"].is_string());
                assert!(!obj["output_heap"].as_str().unwrap().is_empty());
                assert!(obj["code"].is_string());
                assert!(!obj["code"].as_str().unwrap().is_empty());
            }

            tokio::task::yield_now().await;
        }
    });

    writer.await.unwrap();
    reader.await.unwrap();

    // Final state: all entries present
    let entries = log.list_entries("atomic-session", None).await.unwrap();
    assert_eq!(entries.len(), num_writes);
}
