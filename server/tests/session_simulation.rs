//! Simulation-style deterministic tests for session log, following sled's
//! simulation testing philosophy. Tests crash/recovery semantics and
//! concurrent workload correctness.

use server::mcp::session_log::{SessionLog, SessionLogEntry};
use std::sync::Arc;

fn make_entry(input: Option<&str>, output: &str, code: &str) -> SessionLogEntry {
    SessionLogEntry {
        input_heap: input.map(|s| s.to_string()),
        output_heap: output.to_string(),
        code: code.to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(), // deterministic timestamp
    }
}

/// Simulate a workload: append N entries to a session, then verify.
async fn run_workload(log: &SessionLog, session: &str, count: usize) -> Vec<u64> {
    let mut seqs = Vec::new();
    let mut prev_hash: Option<String> = None;

    for i in 0..count {
        let output = format!("hash_{}", i);
        let entry = make_entry(prev_hash.as_deref(), &output, &format!("step_{}", i));
        let seq = log.append(session, entry).await.unwrap();
        seqs.push(seq);
        prev_hash = Some(output);
    }

    seqs
}

/// Verify that a session contains the expected chain of entries.
async fn verify_chain(log: &SessionLog, session: &str, expected_count: usize) {
    let entries = log.list_entries(session, None).await.unwrap();
    assert_eq!(
        entries.len(),
        expected_count,
        "expected {} entries, got {}",
        expected_count,
        entries.len()
    );

    for (i, entry) in entries.iter().enumerate() {
        let obj = entry.as_object().unwrap();
        assert_eq!(
            obj["output_heap"].as_str().unwrap(),
            format!("hash_{}", i),
            "entry {} has wrong output_heap",
            i
        );
        assert_eq!(
            obj["code"].as_str().unwrap(),
            format!("step_{}", i),
            "entry {} has wrong code",
            i
        );

        if i == 0 {
            assert!(
                obj["input_heap"].is_null(),
                "first entry should have null input_heap"
            );
        } else {
            assert_eq!(
                obj["input_heap"].as_str().unwrap(),
                format!("hash_{}", i - 1),
                "entry {} has wrong input_heap",
                i
            );
        }
    }
}

#[tokio::test]
async fn test_flush_and_recovery() {
    // Use a real (non-temporary) path so we can reopen after drop.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("session_db");
    let db_path_str = db_path.to_str().unwrap();

    // Phase 1: write entries and flush
    {
        let log = SessionLog::new(db_path_str).unwrap();
        run_workload(&log, "recovery-session", 10).await;
        log.flush().unwrap();
        // Drop triggers close
    }

    // Phase 2: reopen and verify all flushed entries survived
    {
        let log = SessionLog::new(db_path_str).unwrap();
        verify_chain(&log, "recovery-session", 10).await;

        // Sessions list should include our session
        let sessions = log.list_sessions().await.unwrap();
        assert!(sessions.contains(&"recovery-session".to_string()));
    }
}

#[tokio::test]
async fn test_crash_recovery_partial_flush() {
    // Simulate: write some entries, flush, write more (unflushed), drop (crash), reopen.
    // Flushed entries must survive. Unflushed entries may or may not survive (sled
    // makes no guarantee for unflushed data on crash), but whatever is present
    // must be valid (no partial/corrupted entries).
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("session_db");
    let db_path_str = db_path.to_str().unwrap();

    let flushed_count = 5;
    let unflushed_count = 5;

    // Phase 1: write and partially flush
    {
        let log = SessionLog::new(db_path_str).unwrap();

        // Write and flush first batch
        run_workload(&log, "partial-session", flushed_count).await;
        log.flush().unwrap();

        // Write more without flushing (simulating crash before flush)
        let mut prev = format!("hash_{}", flushed_count - 1);
        for i in flushed_count..(flushed_count + unflushed_count) {
            let output = format!("hash_{}", i);
            log.append(
                "partial-session",
                make_entry(Some(&prev), &output, &format!("step_{}", i)),
            )
            .await
            .unwrap();
            prev = output;
        }
        // Drop without flush — simulates crash
    }

    // Phase 2: reopen and verify
    {
        let log = SessionLog::new(db_path_str).unwrap();
        let entries = log.list_entries("partial-session", None).await.unwrap();

        // At least the flushed entries must survive
        assert!(
            entries.len() >= flushed_count,
            "expected at least {} flushed entries, got {}",
            flushed_count,
            entries.len()
        );

        // All surviving entries must be valid (deserializable, complete)
        for entry in &entries {
            let obj = entry.as_object().unwrap();
            assert!(obj.contains_key("index"));
            assert!(obj.contains_key("input_heap"));
            assert!(obj.contains_key("output_heap"));
            assert!(obj.contains_key("code"));
            assert!(obj.contains_key("timestamp"));
            assert!(obj["output_heap"].is_string());
            assert!(!obj["output_heap"].as_str().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn test_repeated_open_close_cycles() {
    // Open, write, close, reopen, write more — multiple cycles.
    // Verify cumulative state is correct after each reopen.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("session_db");
    let db_path_str = db_path.to_str().unwrap();

    let entries_per_cycle = 3;
    let num_cycles = 5;

    for cycle in 0..num_cycles {
        let log = SessionLog::new(db_path_str).unwrap();

        let offset = cycle * entries_per_cycle;
        let mut prev = if offset > 0 {
            Some(format!("hash_{}", offset - 1))
        } else {
            None
        };

        for i in 0..entries_per_cycle {
            let idx = offset + i;
            let output = format!("hash_{}", idx);
            log.append(
                "cycle-session",
                make_entry(prev.as_deref(), &output, &format!("step_{}", idx)),
            )
            .await
            .unwrap();
            prev = Some(output);
        }

        log.flush().unwrap();

        // Verify total entries so far
        let entries = log.list_entries("cycle-session", None).await.unwrap();
        assert_eq!(
            entries.len(),
            (cycle + 1) * entries_per_cycle,
            "after cycle {}, expected {} entries",
            cycle,
            (cycle + 1) * entries_per_cycle
        );
    }
}

#[tokio::test]
async fn test_concurrent_session_writers_no_duplicates() {
    // Two "nodes" (tasks) writing to the same session concurrently.
    // Assert no duplicate sequence numbers and all entries are present.
    let log = Arc::new(
        SessionLog::from_config(sled::Config::new().temporary(true))
            .expect("failed to open temp sled"),
    );

    let num_per_writer = 50;

    let log1 = log.clone();
    let writer1 = tokio::spawn(async move {
        let mut seqs = Vec::new();
        for i in 0..num_per_writer {
            let seq = log1
                .append(
                    "shared-session",
                    make_entry(None, &format!("w1_hash_{}", i), &format!("w1_code_{}", i)),
                )
                .await
                .unwrap();
            seqs.push(seq);
        }
        seqs
    });

    let log2 = log.clone();
    let writer2 = tokio::spawn(async move {
        let mut seqs = Vec::new();
        for i in 0..num_per_writer {
            let seq = log2
                .append(
                    "shared-session",
                    make_entry(None, &format!("w2_hash_{}", i), &format!("w2_code_{}", i)),
                )
                .await
                .unwrap();
            seqs.push(seq);
        }
        seqs
    });

    let seqs1 = writer1.await.unwrap();
    let seqs2 = writer2.await.unwrap();

    // All sequence numbers should be globally unique
    let mut all_seqs: Vec<u64> = seqs1.into_iter().chain(seqs2).collect();
    let total = all_seqs.len();
    all_seqs.sort();
    all_seqs.dedup();
    assert_eq!(
        all_seqs.len(),
        total,
        "duplicate sequence numbers detected"
    );

    // Total entries should be 2 * num_per_writer
    let entries = log.list_entries("shared-session", None).await.unwrap();
    assert_eq!(entries.len(), num_per_writer * 2);

    // All entries should be valid
    for entry in &entries {
        let obj = entry.as_object().unwrap();
        let output = obj["output_heap"].as_str().unwrap();
        assert!(
            output.starts_with("w1_hash_") || output.starts_with("w2_hash_"),
            "unexpected output_heap: {}",
            output
        );
    }
}

#[tokio::test]
async fn test_deterministic_workload_replay() {
    // Run the exact same workload twice on two separate DBs.
    // The resulting entries should be identical (excluding sequence numbers,
    // which depend on global sled state).
    let log1 =
        SessionLog::from_config(sled::Config::new().temporary(true)).expect("open temp sled");
    let log2 =
        SessionLog::from_config(sled::Config::new().temporary(true)).expect("open temp sled");

    let count = 20;
    run_workload(&log1, "deterministic", count).await;
    run_workload(&log2, "deterministic", count).await;

    let entries1 = log1.list_entries("deterministic", None).await.unwrap();
    let entries2 = log2.list_entries("deterministic", None).await.unwrap();

    assert_eq!(entries1.len(), entries2.len());

    for (e1, e2) in entries1.iter().zip(entries2.iter()) {
        // Content should be identical (ignoring index which may differ)
        assert_eq!(e1["input_heap"], e2["input_heap"]);
        assert_eq!(e1["output_heap"], e2["output_heap"]);
        assert_eq!(e1["code"], e2["code"]);
        assert_eq!(e1["timestamp"], e2["timestamp"]);
    }
}

#[tokio::test]
async fn test_linearizable_session_operations() {
    // Simulate a sequence of operations and verify the observed results
    // are consistent with a serial execution.
    let log =
        SessionLog::from_config(sled::Config::new().temporary(true)).expect("open temp sled");

    // Operation sequence: append, append, list, append, get_latest, list
    log.append("linear", make_entry(None, "h0", "c0")).await.unwrap();
    log.append("linear", make_entry(Some("h0"), "h1", "c1"))
        .await
        .unwrap();

    let entries = log.list_entries("linear", None).await.unwrap();
    assert_eq!(entries.len(), 2);

    log.append("linear", make_entry(Some("h1"), "h2", "c2"))
        .await
        .unwrap();

    let latest = log.get_latest("linear").await.unwrap().unwrap();
    assert_eq!(latest.output_heap, "h2");

    let entries = log.list_entries("linear", None).await.unwrap();
    assert_eq!(entries.len(), 3);

    // Verify full chain manually (uses h0/h1/h2 naming, not hash_N)
    assert!(entries[0]["input_heap"].is_null());
    assert_eq!(entries[0]["output_heap"], "h0");
    assert_eq!(entries[1]["input_heap"], "h0");
    assert_eq!(entries[1]["output_heap"], "h1");
    assert_eq!(entries[2]["input_heap"], "h1");
    assert_eq!(entries[2]["output_heap"], "h2");
}
