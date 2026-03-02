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
            if let Some(mut info) = common::extract_execution_info(&poll_resp) {
                match info["status"].as_str() {
                    Some("completed") | Some("failed") | Some("timed_out") | Some("cancelled") => {
                        // Include the execution_id so callers can fetch console output
                        info["execution_id"] = json!(exec_id);
                        return Ok(info);
                    }
                    _ => continue,
                }
            }
        }
        Err("Execution did not complete within polling timeout".into())
    }

    /// Get console output for a completed execution.
    /// Returns the console output text.
    async fn get_console_output(
        &mut self,
        id: u64,
        execution_id: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "get_execution_output",
                "arguments": {
                    "execution_id": execution_id,
                    "line_limit": 10000
                }
            }
        });

        let response = self.send_message(msg).await?;
        if let Some(info) = common::extract_execution_info(&response) {
            Ok(info["data"].as_str().unwrap_or("").to_string())
        } else {
            Ok(String::new())
        }
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

    // Call run_js tool and poll until completed
    let info = server.run_js_and_wait(2, "console.log(1 + 1)", None).await?;
    assert_eq!(info["status"], "completed", "Execution should complete successfully");
    let exec_id = info["execution_id"].as_str().unwrap();
    let output = server.get_console_output(100, exec_id).await?;
    assert!(output.contains("2"), "Console output should contain '2', got: {}", output);

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

    // Set a variable on globalThis so it persists in the heap snapshot
    let info1 = server.run_js_and_wait(2, "globalThis.persistentValue = 42; console.log(globalThis.persistentValue)", None).await?;
    assert_eq!(info1["status"], "completed", "First call should succeed");

    // Extract the content hash from the completed execution
    let heap_hash = info1["heap"].as_str()
        .expect("First response should contain a heap content hash");

    // Read the variable from the heap using the content hash
    let info2 = server.run_js_and_wait(3, "console.log(globalThis.persistentValue)", Some(heap_hash)).await?;
    assert_eq!(info2["status"], "completed", "Second call should succeed");

    // Verify the value persisted via console output
    let exec_id = info2["execution_id"].as_str().unwrap();
    let output = server.get_console_output(200, exec_id).await?;
    assert!(output.contains("42"), "Persisted value should be 42, got: {}", output);

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

    // Perform multiple sequential operations, threading the content hash.
    // Use globalThis for persistence across module executions and console.log for output.
    let operations = vec![
        ("globalThis.counter = 0; console.log(globalThis.counter)", "0"),
        ("globalThis.counter = globalThis.counter + 1; console.log(globalThis.counter)", "1"),
        ("globalThis.counter = globalThis.counter + 1; console.log(globalThis.counter)", "2"),
        ("globalThis.counter = globalThis.counter + 1; console.log(globalThis.counter)", "3"),
    ];

    let mut current_heap: Option<String> = None;

    for (idx, (code, expected)) in operations.iter().enumerate() {
        let info = server.run_js_and_wait(
            (idx + 2) as u64,
            code,
            current_heap.as_deref(),
        ).await?;
        assert_eq!(info["status"], "completed", "Operation {} should succeed", idx);

        let exec_id = info["execution_id"].as_str().unwrap();
        let output = server.get_console_output((300 + idx) as u64, exec_id).await?;
        assert!(output.trim() == *expected,
                "Operation {} should return {}, got: {}", idx, expected, output.trim());

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

    // Set variable in heap A (fresh session) using globalThis for persistence
    let info_a = server.run_js_and_wait(2, "globalThis.heapValue = 'A'; console.log(globalThis.heapValue)", None).await?;
    assert_eq!(info_a["status"], "completed");
    let hash_a = info_a["heap"].as_str()
        .expect("Should get content hash for heap A");

    // Set variable in heap B (fresh session) using globalThis for persistence
    let info_b = server.run_js_and_wait(3, "globalThis.heapValue = 'B'; console.log(globalThis.heapValue)", None).await?;
    assert_eq!(info_b["status"], "completed");
    let hash_b = info_b["heap"].as_str()
        .expect("Should get content hash for heap B");

    // Read from heap A using its content hash - should still be 'A'
    let verify_a = server.run_js_and_wait(4, "console.log(globalThis.heapValue)", Some(hash_a)).await?;
    assert_eq!(verify_a["status"], "completed");
    let exec_id_a = verify_a["execution_id"].as_str().unwrap();
    let output_a = server.get_console_output(400, exec_id_a).await?;
    assert!(output_a.contains("A"), "Heap A should contain 'A', got: {}", output_a);

    // Read from heap B using its content hash - should still be 'B'
    let verify_b = server.run_js_and_wait(5, "console.log(globalThis.heapValue)", Some(hash_b)).await?;
    assert_eq!(verify_b["status"], "completed");
    let exec_id_b = verify_b["execution_id"].as_str().unwrap();
    let output_b = server.get_console_output(401, exec_id_b).await?;
    assert!(output_b.contains("B"), "Heap B should contain 'B', got: {}", output_b);

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

    // Test array operations (fresh session) - use console.log for output
    let info = server.run_js_and_wait(2, "console.log([1, 2, 3, 4, 5].reduce((a, b) => a + b, 0))", None).await?;
    assert_eq!(info["status"], "completed");
    let exec_id = info["execution_id"].as_str().unwrap();
    let output = server.get_console_output(500, exec_id).await?;
    assert!(output.contains("15"), "Array sum should be 15, got: {}", output);

    // Test object operations (fresh session) - use console.log for output
    let info2 = server.run_js_and_wait(3, "const obj = {a: 1, b: 2, c: 3}; console.log(Object.keys(obj).length)", None).await?;
    assert_eq!(info2["status"], "completed");
    let exec_id2 = info2["execution_id"].as_str().unwrap();
    let output2 = server.get_console_output(501, exec_id2).await?;
    assert!(output2.contains("3"), "Object should have 3 keys, got: {}", output2);

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
