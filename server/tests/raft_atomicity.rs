/// Tests for Raft WAL atomicity and crash safety.
///
/// These tests verify that:
/// 1. Concurrent writes to the same heap are serialized (mutual exclusion)
/// 2. Read/write operations are atomic
/// 3. Crash recovery cleans up staged files

use std::sync::Once;
use std::sync::Arc;

use sha2::{Sha256, Digest};

use server::mcp::heap_storage::{FileHeapStorage, AnyHeapStorage, HeapStorage};
use server::raft;
use server::raft::types::{Command, SnapshotRef};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::mcp::initialize_v8();
    });
}

fn create_temp_heap_dir() -> String {
    let dir = format!("/tmp/mcp-test-heap-{}", uuid::Uuid::new_v4());
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Helper: execute V8 and return (output, snapshot_bytes, sha256).
/// V8 snapshot operations must be serialized — we use spawn_blocking
/// and run them one at a time through a tokio mutex.
async fn v8_execute(
    code: String,
    snapshot: Option<Vec<u8>>,
    v8_lock: Arc<tokio::sync::Mutex<()>>,
) -> (String, Vec<u8>, [u8; 32]) {
    let _guard = v8_lock.lock().await;
    let result = tokio::task::spawn_blocking(move || {
        server::mcp::execute_stateful(code, snapshot, 8 * 1024 * 1024, 30)
    })
    .await
    .unwrap()
    .unwrap();

    let (output, startup_data) = result;
    let sha256: [u8; 32] = Sha256::digest(&startup_data).into();
    (output, startup_data, sha256)
}

/// Test: Two concurrent writers trying to write the same heap.
/// Assert that writes are serialized through Raft — only one SnapshotRef
/// exists at any time, and the final state is consistent (not corrupted).
#[tokio::test]
async fn test_concurrent_heap_writes_are_serialized() {
    ensure_v8();

    let heap_dir = create_temp_heap_dir();
    let storage = Arc::new(AnyHeapStorage::File(FileHeapStorage::new(&heap_dir)));

    // Start Raft in temporary mode
    let (raft, store) = raft::start_raft_node_temp().await.unwrap();

    // Give Raft a moment to elect leader
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let raft = Arc::new(raft);
    let v8_lock = Arc::new(tokio::sync::Mutex::new(()));

    // Spawn two tasks that both try to write to the SAME heap simultaneously.
    // V8 calls are serialized via mutex (V8 snapshot_creator is not thread-safe),
    // but the Raft proposals race concurrently — the point is to test Raft serialization.
    let mut handles = Vec::new();
    for i in 0..2 {
        let raft = raft.clone();
        let storage = storage.clone();
        let v8_lock = v8_lock.clone();
        let code = format!("var x = {}; x", i);
        let heap_name = "shared".to_string();

        let handle = tokio::spawn(async move {
            // Execute V8 (serialized through mutex)
            let (output, startup_data, sha256) = v8_execute(code, None, v8_lock).await;

            let staged_key = format!("{}_staged_{}", heap_name, uuid::Uuid::new_v4());

            // Write staged file
            storage.put(&staged_key, &startup_data).await.unwrap();

            // Commit through Raft (concurrent with other task)
            let cmd = Command::Execute {
                session_id: heap_name,
                result_ref: SnapshotRef {
                    key: staged_key,
                    sha256,
                },
            };

            raft.client_write(cmd).await.unwrap();

            output
        });
        handles.push(handle);
    }

    // Wait for both to complete
    let results: Vec<String> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // Both tasks completed successfully
    assert_eq!(results.len(), 2);

    // Read the committed state from the Raft state machine
    let state = store.get_state().await;

    // Exactly ONE SnapshotRef exists for "shared" session
    assert!(
        state.sessions.contains_key("shared"),
        "shared session should exist in committed state"
    );
    let snap_ref = &state.sessions["shared"];

    // The snapshot ref's sha256 matches the actual bytes on disk
    let stored_bytes = storage.get(&snap_ref.key).await.unwrap();
    let actual_sha256: [u8; 32] = Sha256::digest(&stored_bytes).into();
    assert_eq!(
        snap_ref.sha256, actual_sha256,
        "sha256 of stored bytes should match committed ref"
    );

    // Loading the snapshot and running "x" returns either "0" or "1" (not corrupted)
    let v8_lock = v8_lock.clone();
    let (output, _, _) = v8_execute("x".to_string(), Some(stored_bytes), v8_lock).await;
    assert!(
        output == "0" || output == "1",
        "expected '0' or '1', got '{}'",
        output
    );

    // Cleanup
    std::fs::remove_dir_all(&heap_dir).ok();
}

/// Test: Read-write atomicity — concurrent tasks all writing to the same heap
/// produce a consistent final state with no corruption.
#[tokio::test]
async fn test_read_write_atomicity() {
    ensure_v8();

    let heap_dir = create_temp_heap_dir();
    let storage = Arc::new(AnyHeapStorage::File(FileHeapStorage::new(&heap_dir)));

    let (raft, store) = raft::start_raft_node_temp().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let raft = Arc::new(raft);
    let store = Arc::new(store);
    let v8_lock = Arc::new(tokio::sync::Mutex::new(()));

    // Write initial state: var counter = 0
    {
        let (_, startup_data, sha256) = v8_execute(
            "var counter = 0; counter".to_string(),
            None,
            v8_lock.clone(),
        )
        .await;

        let key = "atomic_init".to_string();
        storage.put(&key, &startup_data).await.unwrap();

        let cmd = Command::Execute {
            session_id: "atomic".to_string(),
            result_ref: SnapshotRef { key, sha256 },
        };
        raft.client_write(cmd).await.unwrap();
    }

    // Spawn N concurrent tasks that all increment counter
    let n = 5usize;
    let mut handles = Vec::new();
    for i in 0..n {
        let raft = raft.clone();
        let storage = storage.clone();
        let store_clone = store.clone();
        let v8_lock = v8_lock.clone();

        let handle = tokio::spawn(async move {
            // Read current snapshot from state machine
            let state = store_clone.get_state().await;
            let snap_ref = state.sessions.get("atomic").unwrap();
            let snapshot_bytes = storage.get(&snap_ref.key).await.unwrap();

            // Execute: increment counter (serialized via V8 mutex)
            let (_output, startup_data, sha256) = v8_execute(
                "counter += 1; counter".to_string(),
                Some(snapshot_bytes),
                v8_lock,
            )
            .await;

            let staged_key = format!("atomic_staged_{}", i);
            storage.put(&staged_key, &startup_data).await.unwrap();

            let cmd = Command::Execute {
                session_id: "atomic".to_string(),
                result_ref: SnapshotRef {
                    key: staged_key,
                    sha256,
                },
            };
            raft.client_write(cmd).await.unwrap();
        });
        handles.push(handle);
    }

    futures::future::join_all(handles).await;

    // Read final state
    let state = store.get_state().await;
    let snap_ref = state.sessions.get("atomic").unwrap();
    let final_bytes = storage.get(&snap_ref.key).await.unwrap();

    // Verify the snapshot is valid (not corrupted)
    let actual_sha256: [u8; 32] = Sha256::digest(&final_bytes).into();
    assert_eq!(
        snap_ref.sha256, actual_sha256,
        "final snapshot sha256 should match committed ref"
    );

    // Load the snapshot and check counter value is consistent
    let (output, _, _) = v8_execute("counter".to_string(), Some(final_bytes), v8_lock).await;

    // Counter should be a valid number (serialized execution)
    let counter_val: i64 = output.parse().expect("counter should be a number");
    assert!(
        counter_val >= 1 && counter_val <= n as i64,
        "counter should be between 1 and {}, got {}",
        n,
        counter_val
    );

    std::fs::remove_dir_all(&heap_dir).ok();
}

/// Test: Crash recovery — staged files left by a crash are cleaned up by GC,
/// and the state machine has no entry for the "crashed" session.
#[tokio::test]
async fn test_crash_leaves_no_partial_state() {
    let heap_dir = create_temp_heap_dir();
    let storage = AnyHeapStorage::File(FileHeapStorage::new(&heap_dir));

    // Start a Raft node (temporary)
    let (_raft, store) = raft::start_raft_node_temp().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Simulate a crash: write a staged file but do NOT commit through Raft
    let staged_key = "crashed_session_staged_fake-uuid";
    storage
        .put(staged_key, b"fake snapshot data")
        .await
        .unwrap();

    // Verify the staged file exists
    let staged = storage.list_staged().await.unwrap();
    assert!(
        staged.contains(&staged_key.to_string()),
        "staged file should exist before GC"
    );

    // Run GC sweep
    raft::gc::gc_sweep(&storage, &store).await.unwrap();

    // Assert: staged file is deleted
    let staged_after = storage.list_staged().await.unwrap();
    assert!(
        !staged_after.contains(&staged_key.to_string()),
        "staged file should be deleted after GC"
    );

    // Assert: state machine has no entry for the "crashed" session
    let state = store.get_state().await;
    assert!(
        !state.sessions.contains_key("crashed_session"),
        "crashed session should not exist in state machine"
    );

    std::fs::remove_dir_all(&heap_dir).ok();
}
