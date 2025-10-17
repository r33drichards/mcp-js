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
    /// Start a new stdio MCP server for testing
    async fn start(heap_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new("cargo")
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

    // Call run_js tool
    let tool_call_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "1 + 1",
                "heap": "stdio-test-heap"
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

    // Set a variable in the heap
    let set_var_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var persistentValue = 42; persistentValue",
                "heap": "persistence-test-heap"
            }
        }
    });

    let response1 = server.send_message(set_var_msg).await?;
    assert!(response1["result"].is_object(), "First call should succeed");

    // Read the variable from the heap in a second call
    let read_var_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "persistentValue",
                "heap": "persistence-test-heap"
            }
        }
    });

    let response2 = server.send_message(read_var_msg).await?;
    assert!(response2["result"].is_object(), "Second call should succeed");

    // Verify the value persisted
    let content_str = serde_json::to_string(&response2["result"]["content"])?;
    assert!(content_str.contains("42"), "Persisted value should be 42");

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

    // Send invalid JavaScript
    let invalid_js_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "this is not valid javascript!!!",
                "heap": "error-test-heap"
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

    // Perform multiple sequential operations
    let operations = vec![
        ("var counter = 0; counter", "0"),
        ("counter = counter + 1; counter", "1"),
        ("counter = counter + 1; counter", "2"),
        ("counter = counter + 1; counter", "3"),
    ];

    for (idx, (code, expected)) in operations.iter().enumerate() {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": idx + 2,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": {
                    "code": code,
                    "heap": "sequential-test-heap"
                }
            }
        });

        let response = server.send_message(msg).await?;
        assert!(response["result"].is_object(), "Operation {} should succeed", idx);

        let content_str = serde_json::to_string(&response["result"]["content"])?;
        assert!(content_str.contains(expected),
                "Operation {} should return {}, got: {}", idx, expected, content_str);
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

    // Set variable in heap A
    let set_heap_a = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var heapValue = 'A'; heapValue",
                "heap": "heap-a"
            }
        }
    });

    let response_a = server.send_message(set_heap_a).await?;
    assert!(response_a["result"].is_object());

    // Set variable in heap B
    let set_heap_b = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var heapValue = 'B'; heapValue",
                "heap": "heap-b"
            }
        }
    });

    let response_b = server.send_message(set_heap_b).await?;
    assert!(response_b["result"].is_object());

    // Read from heap A - should still be 'A'
    let read_heap_a = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "heapValue",
                "heap": "heap-a"
            }
        }
    });

    let verify_a = server.send_message(read_heap_a).await?;
    let content_a = serde_json::to_string(&verify_a["result"]["content"])?;
    assert!(content_a.contains("A"), "Heap A should contain 'A', got: {}", content_a);

    // Read from heap B - should still be 'B'
    let read_heap_b = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "heapValue",
                "heap": "heap-b"
            }
        }
    });

    let verify_b = server.send_message(read_heap_b).await?;
    let content_b = serde_json::to_string(&verify_b["result"]["content"])?;
    assert!(content_b.contains("B"), "Heap B should contain 'B', got: {}", content_b);

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

    // Test array operations
    let array_op = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "[1, 2, 3, 4, 5].reduce((a, b) => a + b, 0)",
                "heap": "complex-test-heap"
            }
        }
    });

    let response = server.send_message(array_op).await?;
    let content = serde_json::to_string(&response["result"]["content"])?;
    assert!(content.contains("15"), "Array sum should be 15");

    // Test object operations
    let object_op = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var obj = {a: 1, b: 2, c: 3}; Object.keys(obj).length",
                "heap": "complex-test-heap"
            }
        }
    });

    let response2 = server.send_message(object_op).await?;
    let content2 = serde_json::to_string(&response2["result"]["content"])?;
    assert!(content2.contains("3"), "Object should have 3 keys");

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
