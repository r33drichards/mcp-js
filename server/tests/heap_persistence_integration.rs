/// Integration tests for heap persistence issues in production
///
/// This test suite reproduces bugs found in production:
/// 1. "Heap already exists" error even when heap doesn't exist
/// 2. "Is a directory (os error 21)" error when trying to use a heap
///
/// These issues are related to URI parsing and file system path handling.

use std::path::PathBuf;
use mcp_server::mcp::heap_storage::{AnyHeapStorage, MultiHeapStorage, FileHeapStorage};

mod common;

/// Test helper to create a fresh temporary directory for heap storage
fn create_fresh_heap_dir() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    let unique_name = format!("heap-test-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let heap_dir = temp_dir.join(unique_name);

    // Clean up if it exists
    let _ = std::fs::remove_dir_all(&heap_dir);

    // Create fresh directory
    std::fs::create_dir_all(&heap_dir).expect("Failed to create test heap directory");

    heap_dir
}

/// Test reproducing: "Heap 'file://my-demo-heap.snapshot' already exists."
/// when the heap doesn't actually exist.
#[tokio::test]
async fn test_create_heap_already_exists_false_positive() {
    mcp_server::initialize_v8();
    let heap_dir = create_fresh_heap_dir();
    let heap_dir_str = heap_dir.to_string_lossy().to_string();

    // Create storage with fresh directory
    let file_storage = Some(FileHeapStorage::new(&heap_dir));
    let heap_storage = AnyHeapStorage::Multi(MultiHeapStorage::new(file_storage, None));

    // Test case 1: Simple heap name
    let heap_uri_1 = "file://my-demo-heap.snapshot";

    // Verify the heap doesn't exist
    let exists = heap_storage.exists_by_uri(heap_uri_1).await
        .expect("Should be able to check existence");
    assert!(!exists, "Heap should not exist initially");

    // Try to create the heap - should succeed
    // This simulates the create_heap tool call
    let v8_result = tokio::task::spawn_blocking(|| {
        mcp_server::mcp::execute_stateful_for_test("undefined".to_string(), None)
    }).await;

    match v8_result {
        Ok(Ok((_output, startup_data))) => {
            let put_result = heap_storage.put_by_uri(heap_uri_1, &startup_data).await;
            assert!(put_result.is_ok(), "Should be able to create heap, got error: {:?}", put_result.err());
        }
        Ok(Err(e)) => panic!("V8 error creating heap: {}", e),
        Err(e) => panic!("Task error creating heap: {}", e),
    }

    // Verify the heap now exists
    let exists = heap_storage.exists_by_uri(heap_uri_1).await
        .expect("Should be able to check existence");
    assert!(exists, "Heap should exist after creation");

    // Cleanup
    let _ = std::fs::remove_dir_all(&heap_dir);
}

/// Test reproducing: "Error saving heap: Is a directory (os error 21)"
/// when trying to use a heap after creation.
#[tokio::test]
async fn test_heap_is_directory_error() {
    mcp_server::initialize_v8();
    let heap_dir = create_fresh_heap_dir();

    // Create storage with fresh directory
    let file_storage = Some(FileHeapStorage::new(&heap_dir));
    let heap_storage = AnyHeapStorage::Multi(MultiHeapStorage::new(file_storage, None));

    let heap_uri = "file://my-test-heap.snapshot";

    // Step 1: Create a heap (simulating create_heap tool)
    let v8_result = tokio::task::spawn_blocking(|| {
        mcp_server::mcp::execute_stateful_for_test("undefined".to_string(), None)
    }).await;

    match v8_result {
        Ok(Ok((_output, startup_data))) => {
            let put_result = heap_storage.put_by_uri(heap_uri, &startup_data).await;
            assert!(put_result.is_ok(), "Should be able to create heap initially, got: {:?}", put_result.err());
        }
        Ok(Err(e)) => panic!("V8 error creating heap: {}", e),
        Err(e) => panic!("Task error creating heap: {}", e),
    }

    // Step 2: Load and use the heap (simulating run_js tool)
    let snapshot = heap_storage.get_by_uri(heap_uri).await
        .expect("Should be able to load heap");

    let code = r#"const myData = { counter: 1, message: "Hello from the heap!" }; myData;"#;
    let v8_result = tokio::task::spawn_blocking(move || {
        mcp_server::mcp::execute_stateful_for_test(code.to_string(), Some(snapshot))
    }).await;

    match v8_result {
        Ok(Ok((output, startup_data))) => {
            // Step 3: Save the heap state after execution
            let put_result = heap_storage.put_by_uri(heap_uri, &startup_data).await;
            assert!(put_result.is_ok(),
                "Should be able to save heap after execution, got error: {:?}",
                put_result.err());

            println!("Execution output: {}", output);
        }
        Ok(Err(e)) => panic!("V8 error executing code: {}", e),
        Err(e) => panic!("Task error executing code: {}", e),
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&heap_dir);
}

/// Test URI parsing with various formats
#[tokio::test]
async fn test_uri_parsing_edge_cases() {
    mcp_server::initialize_v8();
    let heap_dir = create_fresh_heap_dir();

    let file_storage = Some(FileHeapStorage::new(&heap_dir));
    let heap_storage = AnyHeapStorage::Multi(MultiHeapStorage::new(file_storage, None));

    // Test various URI formats
    let test_cases = vec![
        ("file://simple", "Should handle simple filename"),
        ("file://with-dash.ext", "Should handle dashes and extensions"),
        ("file://my-demo-heap.snapshot", "Should handle production case"),
        ("file:///absolute/path/heap", "Should handle absolute paths"),
    ];

    for (uri, description) in test_cases {
        println!("Testing: {} - {}", uri, description);

        // Check if exists (should be false)
        let exists_result = heap_storage.exists_by_uri(uri).await;
        assert!(exists_result.is_ok(), "exists_by_uri should not error for {}: {:?}", uri, exists_result.err());

        let exists = exists_result.unwrap();
        println!("  - Exists: {}", exists);

        if !exists {
            // Try to create it
            let v8_result = tokio::task::spawn_blocking(|| {
                mcp_server::mcp::execute_stateful_for_test("undefined".to_string(), None)
            }).await;

            if let Ok(Ok((_output, startup_data))) = v8_result {
                let put_result = heap_storage.put_by_uri(uri, &startup_data).await;
                println!("  - Create result: {:?}", put_result.is_ok());

                if put_result.is_ok() {
                    // Verify it now exists
                    let exists_after = heap_storage.exists_by_uri(uri).await.unwrap();
                    assert!(exists_after, "Heap should exist after creation: {}", uri);
                    println!("  - Verified exists after creation");
                }
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&heap_dir);
}

/// Test the exact scenario from production logs
#[tokio::test]
async fn test_production_scenario() {
    mcp_server::initialize_v8();
    let heap_dir = create_fresh_heap_dir();

    let file_storage = Some(FileHeapStorage::new(&heap_dir));
    let heap_storage = AnyHeapStorage::Multi(MultiHeapStorage::new(file_storage, None));

    // Exact URI from production logs
    let heap_uri = "file://my-demo-heap.snapshot";

    println!("=== Step 1: Create heap ===");
    // Step 1: Create heap (first request from logs)
    let exists_before = heap_storage.exists_by_uri(heap_uri).await
        .expect("Should be able to check existence");
    println!("Heap exists before create: {}", exists_before);

    if exists_before {
        panic!("BUG REPRODUCED: Heap reported as existing when it shouldn't!");
    }

    // Create the heap
    let v8_result = tokio::task::spawn_blocking(|| {
        mcp_server::mcp::execute_stateful_for_test("undefined".to_string(), None)
    }).await;

    match v8_result {
        Ok(Ok((_output, startup_data))) => {
            let put_result = heap_storage.put_by_uri(heap_uri, &startup_data).await;
            if let Err(e) = put_result {
                panic!("Failed to create heap: {}", e);
            }
            println!("Heap created successfully");
        }
        Ok(Err(e)) => panic!("V8 error: {}", e),
        Err(e) => panic!("Task error: {}", e),
    }

    println!("\n=== Step 2: Use heap with run_js ===");
    // Step 2: Use the heap (second request from logs)
    let snapshot = heap_storage.get_by_uri(heap_uri).await
        .expect("Should be able to load heap");
    println!("Heap loaded, size: {} bytes", snapshot.len());

    let code = r#"const myData = { counter: 1, message: "Hello from the heap!" }; myData;"#;
    let v8_result = tokio::task::spawn_blocking(move || {
        mcp_server::mcp::execute_stateful_for_test(code.to_string(), Some(snapshot))
    }).await;

    match v8_result {
        Ok(Ok((output, startup_data))) => {
            println!("Execution output: {}", output);
            println!("New snapshot size: {} bytes", startup_data.len());

            // Try to save - this is where "Is a directory" error occurs
            let put_result = heap_storage.put_by_uri(heap_uri, &startup_data).await;
            if let Err(e) = put_result {
                if e.contains("Is a directory") {
                    panic!("BUG REPRODUCED: Is a directory error: {}", e);
                } else {
                    panic!("Failed to save heap: {}", e);
                }
            }
            println!("Heap saved successfully");
        }
        Ok(Err(e)) => panic!("V8 error: {}", e),
        Err(e) => panic!("Task error: {}", e),
    }

    println!("\n=== Test passed ===");

    // Cleanup
    let _ = std::fs::remove_dir_all(&heap_dir);
}

/// Test to verify heap file is not created as a directory
#[tokio::test]
async fn test_heap_file_not_directory() {
    mcp_server::initialize_v8();
    let heap_dir = create_fresh_heap_dir();

    let file_storage = Some(FileHeapStorage::new(&heap_dir));
    let heap_storage = AnyHeapStorage::Multi(MultiHeapStorage::new(file_storage, None));

    let heap_uri = "file://test-heap";

    // Create heap
    let v8_result = tokio::task::spawn_blocking(|| {
        mcp_server::mcp::execute_stateful_for_test("undefined".to_string(), None)
    }).await;

    if let Ok(Ok((_output, startup_data))) = v8_result {
        heap_storage.put_by_uri(heap_uri, &startup_data).await
            .expect("Should create heap");

        // Check the actual file on disk
        let heap_file_path = heap_dir.join("test-heap");

        assert!(heap_file_path.exists(), "Heap file should exist on disk");

        let metadata = std::fs::metadata(&heap_file_path)
            .expect("Should be able to get metadata");

        assert!(metadata.is_file(),
            "BUG: Heap should be a FILE, not a directory! Path: {:?}",
            heap_file_path);

        assert!(!metadata.is_dir(),
            "BUG: Heap should NOT be a directory! Path: {:?}",
            heap_file_path);

        println!("Heap is correctly stored as a file");
        println!("File size: {} bytes", metadata.len());
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&heap_dir);
}
