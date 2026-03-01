/// End-to-end integration tests for stdio MCP server
/// These tests start the actual server via stdio and test real MCP protocol communication

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{timeout, Duration};
use serde_json::{json, Value};
use std::process::Stdio;

mod common;

/// Helper structure to manage stdio server process and communication
struct StdioServer {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl StdioServer {
    /// Send a run_js tool call and poll get_execution until complete.
    /// Returns the completed execution info as a JSON value with fields:
    /// execution_id, status, result, heap, error, started_at, completed_at.
    async fn run_js_and_wait(
        &mut self,
        id: u64,
        code: &str,
        heap: Option<&str>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let mut arguments = json!({ "code": code });
        if let Some(h) = heap {
            arguments["heap"] = json!(h);
        }

        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": arguments
            }
        });

        let response = self.send_message(msg).await?;
        let exec_id = common::extract_execution_id(&response)
            .ok_or("run_js response should contain execution_id")?;

        // Poll get_execution until completed/failed/timed_out
        for i in 0..120 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let poll_msg = json!({
                "jsonrpc": "2.0",
                "id": 10000 + id * 1000 + i,
                "method": "tools/call",
                "params": {
                    "name": "get_execution",
                    "arguments": { "execution_id": exec_id }
                }
            });

            let poll_resp = self.send_message(poll_msg).await?;
            if let Some(info) = common::extract_execution_info(&poll_resp) {
                match info["status"].as_str() {
                    Some("completed") | Some("failed") | Some("timed_out") | Some("cancelled") => {
                        return Ok(info);
                    }
                    _ => continue,
                }
            }
        }
        Err("Execution did not complete within polling timeout".into())
    }

    /// Start a new stdio MCP server for testing
    async fn start(heap_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new(env!("CARGO"))
            .args(&["run", "--", "--directory-path", heap_dir])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // Suppress server logs during tests
            .spawn()?;

        let stdin = child.stdin.take().expect("Failed to get stdin");
        let stdout = child.stdout.take().expect("Failed to get stdout");
        let stdout = BufReader::new(stdout);

        // Give server time to initialize
        tokio::time::sleep(Duration::from_millis(500)).await;

        Ok(StdioServer {
            child,
            stdin,
            stdout,
        })
    }

    /// Send a message to the server and read the response
    async fn send_message(&mut self, message: Value) -> Result<Value, Box<dyn std::error::Error>> {
        // Serialize message as newline-delimited JSON
        let message_str = serde_json::to_string(&message)?;
        let message_with_newline = format!("{}\n", message_str);

        // Write to stdin
        self.stdin.write_all(message_with_newline.as_bytes()).await?;
        self.stdin.flush().await?;

        // Read response from stdout
        let mut response_line = String::new();
        timeout(Duration::from_secs(5), self.stdout.read_line(&mut response_line)).await??;

        // Parse response
        let response: Value = serde_json::from_str(&response_line)?;
        Ok(response)
    }

    /// Send a notification (no response expected)
    async fn send_notification(&mut self, notification: Value) -> Result<(), Box<dyn std::error::Error>> {
        let notification_str = serde_json::to_string(&notification)?;
        let notification_with_newline = format!("{}\n", notification_str);

        self.stdin.write_all(notification_with_newline.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Stop the server
    async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

/// Test full stdio MCP initialize handshake
#[tokio::test]
async fn test_stdio_initialize_handshake() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = StdioServer::start(&heap_dir).await?;

    // Send initialize request
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "stdio-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    let response = server.send_message(initialize_msg).await?;

    // Verify response structure
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert!(response["result"].is_object(), "Should have result object");
    assert!(response["result"]["capabilities"].is_object(),
            "Should have capabilities in response");
    assert!(response["result"]["protocolVersion"].is_string(),
            "Should have protocol version in response");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test run_js tool execution via stdio
#[tokio::test]
async fn test_stdio_run_js_execution() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = StdioServer::start(&heap_dir).await?;

    // Initialize first
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "stdio-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    server.send_message(initialize_msg).await?;

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Call run_js tool (no heap needed for fresh session)
    let tool_call_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "1 + 1"
            }
        }
    });

    let response = server.send_message(tool_call_msg).await?;

    // Verify response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);
    assert!(response["result"].is_object(), "Should have result object");

    // Check for output in result
    if let Some(content) = response["result"]["content"].as_array() {
        assert!(!content.is_empty(), "Should have content in response");
        // Verify the result contains "2" (result of 1 + 1)
        let content_str = serde_json::to_string(&content)?;
        assert!(content_str.contains("2"), "Response should contain result '2'");
    }

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test heap persistence across multiple stdio calls
#[tokio::test]
async fn test_stdio_heap_persistence() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = StdioServer::start(&heap_dir).await?;

    // Initialize
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "stdio-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    server.send_message(initialize_msg).await?;

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Set a variable in a fresh heap
    let info1 = server.run_js_and_wait(2, "var persistentValue = 42; persistentValue", None).await?;
    assert_eq!(info1["status"], "completed", "First call should succeed");

    // Extract the content hash from the completed execution
    let heap_hash = info1["heap"].as_str()
        .expect("First response should contain a heap content hash");

    // Read the variable from the heap using the content hash
    let info2 = server.run_js_and_wait(3, "persistentValue", Some(heap_hash)).await?;
    assert_eq!(info2["status"], "completed", "Second call should succeed");

    // Verify the value persisted
    let result = info2["result"].as_str().unwrap_or("");
    assert!(result.contains("42"), "Persisted value should be 42, got: {}", result);

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test error handling for invalid JavaScript via stdio
#[tokio::test]
async fn test_stdio_invalid_javascript_error() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = StdioServer::start(&heap_dir).await?;

    // Initialize
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "stdio-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    server.send_message(initialize_msg).await?;

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Send invalid JavaScript (no heap needed for fresh session)
    let invalid_js_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "this is not valid javascript!!!"
            }
        }
    });

    let response = server.send_message(invalid_js_msg).await?;

    // The server should return a response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);

    // It should have error information
    let has_error = response["error"].is_object() ||
                   (response["result"].is_object() &&
                    response["result"]["content"].as_array()
                        .and_then(|arr| arr.first())
                        .and_then(|c| c.get("text"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.contains("error") || s.contains("Error") || s.contains("V8 error"))
                        .unwrap_or(false));

    assert!(has_error, "Should return error information for invalid JavaScript");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test multiple sequential operations via stdio
#[tokio::test]
async fn test_stdio_sequential_operations() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = StdioServer::start(&heap_dir).await?;

    // Initialize
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "stdio-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    server.send_message(initialize_msg).await?;

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Perform multiple sequential operations, threading the content hash
    let operations = vec![
        ("var counter = 0; counter", "0"),
        ("counter = counter + 1; counter", "1"),
        ("counter = counter + 1; counter", "2"),
        ("counter = counter + 1; counter", "3"),
    ];

    let mut current_heap: Option<String> = None;

    for (idx, (code, expected)) in operations.iter().enumerate() {
        let info = server.run_js_and_wait(
            (idx + 2) as u64,
            code,
            current_heap.as_deref(),
        ).await?;
        assert_eq!(info["status"], "completed", "Operation {} should succeed", idx);

        let result = info["result"].as_str().unwrap_or("");
        assert!(result.contains(expected),
                "Operation {} should return {}, got: {}", idx, expected, result);

        // Thread the content hash to the next call
        current_heap = info["heap"].as_str().map(|s| s.to_string());
    }

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test using different heaps concurrently via stdio
#[tokio::test]
async fn test_stdio_multiple_heaps() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = StdioServer::start(&heap_dir).await?;

    // Initialize
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "stdio-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    server.send_message(initialize_msg).await?;

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Set variable in heap A (fresh session)
    let info_a = server.run_js_and_wait(2, "var heapValue = 'A'; heapValue", None).await?;
    assert_eq!(info_a["status"], "completed");
    let hash_a = info_a["heap"].as_str()
        .expect("Should get content hash for heap A");

    // Set variable in heap B (fresh session)
    let info_b = server.run_js_and_wait(3, "var heapValue = 'B'; heapValue", None).await?;
    assert_eq!(info_b["status"], "completed");
    let hash_b = info_b["heap"].as_str()
        .expect("Should get content hash for heap B");

    // Read from heap A using its content hash - should still be 'A'
    let verify_a = server.run_js_and_wait(4, "heapValue", Some(hash_a)).await?;
    assert_eq!(verify_a["status"], "completed");
    let result_a = verify_a["result"].as_str().unwrap_or("");
    assert!(result_a.contains("A"), "Heap A should contain 'A', got: {}", result_a);

    // Read from heap B using its content hash - should still be 'B'
    let verify_b = server.run_js_and_wait(5, "heapValue", Some(hash_b)).await?;
    assert_eq!(verify_b["status"], "completed");
    let result_b = verify_b["result"].as_str().unwrap_or("");
    assert!(result_b.contains("B"), "Heap B should contain 'B', got: {}", result_b);

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test complex JavaScript operations via stdio
#[tokio::test]
async fn test_stdio_complex_javascript() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = StdioServer::start(&heap_dir).await?;

    // Initialize
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "stdio-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    server.send_message(initialize_msg).await?;

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Test array operations (fresh session)
    let info = server.run_js_and_wait(2, "[1, 2, 3, 4, 5].reduce((a, b) => a + b, 0)", None).await?;
    assert_eq!(info["status"], "completed");
    let result = info["result"].as_str().unwrap_or("");
    assert!(result.contains("15"), "Array sum should be 15, got: {}", result);

    // Test object operations (fresh session)
    let info2 = server.run_js_and_wait(3, "var obj = {a: 1, b: 2, c: 3}; Object.keys(obj).length", None).await?;
    assert_eq!(info2["status"], "completed");
    let result2 = info2["result"].as_str().unwrap_or("");
    assert!(result2.contains("3"), "Object should have 3 keys, got: {}", result2);

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test that server handles graceful shutdown
#[tokio::test]
async fn test_stdio_graceful_shutdown() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let server = StdioServer::start(&heap_dir).await?;

    // Just start and stop the server - should not panic or hang
    server.stop().await;

    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}
