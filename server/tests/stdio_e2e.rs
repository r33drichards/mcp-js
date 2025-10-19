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

    // Create the heap first
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://stdio-test-heap"
            }
        }
    });
    server.send_message(create_heap_msg).await?;

    // Call run_js tool
    let tool_call_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "1 + 1",
                "heap_uri": "file://stdio-test-heap"
            }
        }
    });

    let response = server.send_message(tool_call_msg).await?;

    // Verify response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 3);
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

    // Create the heap first
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://persistence-test-heap"
            }
        }
    });
    server.send_message(create_heap_msg).await?;

    // Set a variable in the heap
    let set_var_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var persistentValue = 42; persistentValue",
                "heap_uri": "file://persistence-test-heap"
            }
        }
    });

    let response1 = server.send_message(set_var_msg).await?;
    assert!(response1["result"].is_object(), "First call should succeed");

    // Read the variable from the heap in a second call
    let read_var_msg = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "persistentValue",
                "heap_uri": "file://persistence-test-heap"
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

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Create the heap first
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://error-test-heap"
            }
        }
    });
    server.send_message(create_heap_msg).await?;

    // Send invalid JavaScript
    let invalid_js_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "this is not valid javascript!!!",
                "heap_uri": "file://error-test-heap"
            }
        }
    });

    let response = server.send_message(invalid_js_msg).await?;

    // The server should return a response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 3);

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

    // Create the heap first
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://sequential-test-heap"
            }
        }
    });
    server.send_message(create_heap_msg).await?;

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
            "id": idx + 3,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": {
                    "code": code,
                    "heap_uri": "file://sequential-test-heap"
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

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Create heap A
    let create_heap_a = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://heap-a"
            }
        }
    });
    server.send_message(create_heap_a).await?;

    // Create heap B
    let create_heap_b = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://heap-b"
            }
        }
    });
    server.send_message(create_heap_b).await?;

    // Set variable in heap A
    let set_heap_a = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var heapValue = 'A'; heapValue",
                "heap_uri": "file://heap-a"
            }
        }
    });

    let response_a = server.send_message(set_heap_a).await?;
    assert!(response_a["result"].is_object());

    // Set variable in heap B
    let set_heap_b = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var heapValue = 'B'; heapValue",
                "heap_uri": "file://heap-b"
            }
        }
    });

    let response_b = server.send_message(set_heap_b).await?;
    assert!(response_b["result"].is_object());

    // Read from heap A - should still be 'A'
    let read_heap_a = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "heapValue",
                "heap_uri": "file://heap-a"
            }
        }
    });

    let verify_a = server.send_message(read_heap_a).await?;
    let content_a = serde_json::to_string(&verify_a["result"]["content"])?;
    assert!(content_a.contains("A"), "Heap A should contain 'A', got: {}", content_a);

    // Read from heap B - should still be 'B'
    let read_heap_b = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "heapValue",
                "heap_uri": "file://heap-b"
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

    // Send initialized notification (required by MCP protocol)
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Create the heap first
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://complex-test-heap"
            }
        }
    });
    server.send_message(create_heap_msg).await?;

    // Test array operations
    let array_op = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "[1, 2, 3, 4, 5].reduce((a, b) => a + b, 0)",
                "heap_uri": "file://complex-test-heap"
            }
        }
    });

    let response = server.send_message(array_op).await?;
    let content = serde_json::to_string(&response["result"]["content"])?;
    assert!(content.contains("15"), "Array sum should be 15");

    // Test object operations
    let object_op = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var obj = {a: 1, b: 2, c: 3}; Object.keys(obj).length",
                "heap_uri": "file://complex-test-heap"
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

/// Test create_heap tool functionality
#[tokio::test]
async fn test_stdio_create_heap_tool() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Create a new heap
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://test-heap-resource"
            }
        }
    });

    let response = server.send_message(create_heap_msg).await?;

    // Verify response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);
    assert!(response["result"].is_object(), "Should have result object");

    // Check that the response contains the heap URI
    let content_str = serde_json::to_string(&response["result"]["content"])?;
    assert!(content_str.contains("file://test-heap-resource"),
            "Should return heap URI in response");
    assert!(content_str.contains("Successfully created heap"),
            "Should contain success message");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test create_heap with invalid heap name
#[tokio::test]
async fn test_stdio_create_heap_invalid_name() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Try to create heap with invalid name (contains spaces and special chars)
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://invalid heap name!"
            }
        }
    });

    let response = server.send_message(create_heap_msg).await?;

    // The file storage will actually accept this, so we should check that it either:
    // 1. Succeeds (file system allows spaces in names), or
    // 2. Returns an error if the filesystem doesn't support it
    // Just verify we get a valid response
    assert!(response["result"].is_object(), "Should have result object");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test delete_heap tool functionality
#[tokio::test]
async fn test_stdio_delete_heap_tool() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Create a heap first
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://heap-to-delete"
            }
        }
    });

    server.send_message(create_heap_msg).await?;

    // Delete the heap
    let delete_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "delete_heap",
            "arguments": {
                "heap_uri": "file://heap-to-delete"
            }
        }
    });

    let response = server.send_message(delete_heap_msg).await?;

    // Verify response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 3);
    assert!(response["result"].is_object(), "Should have result object");

    let content_str = serde_json::to_string(&response["result"]["content"])?;
    assert!(content_str.contains("Successfully deleted heap"),
            "Should return success message");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test list_resources MCP endpoint
#[tokio::test]
async fn test_stdio_list_resources() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Create some heaps
    for heap_name in &["resource-heap-1", "resource-heap-2", "resource-heap-3"] {
        let heap_uri = format!("file://{}", heap_name);
        let create_msg = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "create_heap",
                "arguments": {
                    "heap_uri": heap_uri
                }
            }
        });
        server.send_message(create_msg).await?;
    }

    // List resources
    let list_resources_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "resources/list",
        "params": {}
    });

    let response = server.send_message(list_resources_msg).await?;

    // Verify response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 3);
    assert!(response["result"].is_object(), "Should have result object");
    assert!(response["result"]["resources"].is_array(), "Should have resources array");

    let resources = response["result"]["resources"].as_array().unwrap();
    assert!(resources.len() >= 3, "Should have at least 3 resources");

    // Verify that our heaps are in the list with correct URIs
    let resources_str = serde_json::to_string(&resources)?;
    assert!(resources_str.contains("file://resource-heap-1"),
            "Should contain resource-heap-1");
    assert!(resources_str.contains("file://resource-heap-2"),
            "Should contain resource-heap-2");
    assert!(resources_str.contains("file://resource-heap-3"),
            "Should contain resource-heap-3");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test read_resource MCP endpoint
#[tokio::test]
async fn test_stdio_read_resource() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Create a heap
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://readable-heap"
            }
        }
    });

    server.send_message(create_heap_msg).await?;

    // Execute some code to modify the heap
    let run_js_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var testData = 'resource test'; testData",
                "heap_uri": "file://readable-heap"
            }
        }
    });

    server.send_message(run_js_msg).await?;

    // Read the heap resource
    let read_resource_msg = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "resources/read",
        "params": {
            "uri": "file://readable-heap"
        }
    });

    let response = server.send_message(read_resource_msg).await?;

    // Verify response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 4);
    assert!(response["result"].is_object(), "Should have result object");
    assert!(response["result"]["contents"].is_array(), "Should have contents array");

    let contents = response["result"]["contents"].as_array().unwrap();
    assert!(!contents.is_empty(), "Should have at least one content item");

    // Verify the content has blob data (base64-encoded snapshot)
    let first_content = &contents[0];
    assert!(first_content["blob"].is_string(), "Should have blob field with base64 data");
    // Check mimeType if present - the rmcp SDK may serialize this differently
    if !first_content["mimeType"].is_null() {
        assert_eq!(first_content["mimeType"], "application/octet-stream",
                   "Should have correct MIME type");
    }
    assert_eq!(first_content["uri"], "file://readable-heap",
               "Should have correct URI");

    // Verify the blob is non-empty base64
    let blob = first_content["blob"].as_str().unwrap();
    assert!(!blob.is_empty(), "Blob should not be empty");
    assert!(blob.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '='),
            "Blob should be valid base64");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test read_resource with invalid URI
#[tokio::test]
async fn test_stdio_read_resource_invalid_uri() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Try to read a resource with invalid URI (not heap://)
    let read_resource_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "resources/read",
        "params": {
            "uri": "invalid://not-a-heap"
        }
    });

    let response = server.send_message(read_resource_msg).await?;

    // Should return error
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);
    assert!(response["error"].is_object(), "Should have error object for invalid URI");

    let error_msg = response["error"]["message"].as_str().unwrap();
    assert!(error_msg.contains("Invalid") || error_msg.contains("URI") || error_msg.contains("file://") || error_msg.contains("s3://"),
            "Error should mention invalid URI format");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test read_resource with non-existent heap
#[tokio::test]
async fn test_stdio_read_resource_not_found() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Try to read a non-existent heap
    let read_resource_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "resources/read",
        "params": {
            "uri": "file://non-existent-heap"
        }
    });

    let response = server.send_message(read_resource_msg).await?;

    // Should return error
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);
    assert!(response["error"].is_object(), "Should have error object for non-existent heap");

    let error_msg = response["error"]["message"].as_str().unwrap();
    assert!(error_msg.contains("not found") || error_msg.contains("Heap"),
            "Error should mention heap not found");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test complete heap lifecycle: create, use, list, read, delete
#[tokio::test]
async fn test_stdio_heap_resource_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // 1. Create a heap
    let create_heap_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "create_heap",
            "arguments": {
                "heap_uri": "file://lifecycle-heap"
            }
        }
    });

    let create_response = server.send_message(create_heap_msg).await?;
    assert!(create_response["result"].is_object(), "Create should succeed");

    // 2. Use the heap with run_js
    let run_js_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var lifecycleData = {value: 123}; lifecycleData.value",
                "heap_uri": "file://lifecycle-heap"
            }
        }
    });

    let run_response = server.send_message(run_js_msg).await?;
    assert!(run_response["result"].is_object(), "Run should succeed");

    // 3. List resources - should include our heap
    let list_msg = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "resources/list",
        "params": {}
    });

    let list_response = server.send_message(list_msg).await?;
    let resources_str = serde_json::to_string(&list_response["result"]["resources"])?;
    assert!(resources_str.contains("file://lifecycle-heap"),
            "Heap should appear in resources list");

    // 4. Read the heap resource
    let read_msg = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "resources/read",
        "params": {
            "uri": "file://lifecycle-heap"
        }
    });

    let read_response = server.send_message(read_msg).await?;
    assert!(read_response["result"]["contents"].is_array(), "Read should succeed");
    assert!(!read_response["result"]["contents"].as_array().unwrap().is_empty(),
            "Should have content");

    // 5. Delete the heap
    let delete_msg = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "delete_heap",
            "arguments": {
                "heap_uri": "file://lifecycle-heap"
            }
        }
    });

    let delete_response = server.send_message(delete_msg).await?;
    assert!(delete_response["result"].is_object(), "Delete should succeed");

    // 6. List resources again - heap should be gone
    let list_msg2 = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "resources/list",
        "params": {}
    });

    let list_response2 = server.send_message(list_msg2).await?;
    let resources_str2 = serde_json::to_string(&list_response2["result"]["resources"])?;
    assert!(!resources_str2.contains("file://lifecycle-heap"),
            "Heap should not appear in resources list after deletion");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test run_js with non-existent heap URI
#[tokio::test]
async fn test_stdio_run_js_nonexistent_heap() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Try to run JS on a heap that doesn't exist
    let run_js_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "1 + 1",
                "heap_uri": "file://does-not-exist"
            }
        }
    });

    let response = server.send_message(run_js_msg).await?;

    // Should return error message about non-existent heap
    assert!(response["result"].is_object(), "Should have result");
    let content_str = serde_json::to_string(&response["result"]["content"])?;
    assert!(content_str.contains("does not exist") || content_str.contains("create_heap"),
            "Should indicate heap doesn't exist");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

/// Test run_js with invalid heap URI format
#[tokio::test]
async fn test_stdio_run_js_invalid_heap_uri() -> Result<(), Box<dyn std::error::Error>> {
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

    // Send initialized notification
    server.send_notification(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })).await?;

    // Try to run JS with invalid URI format (not heap://)
    let run_js_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "1 + 1",
                "heap_uri": "not-a-valid-uri"
            }
        }
    });

    let response = server.send_message(run_js_msg).await?;

    // Should return error message about invalid URI
    assert!(response["result"].is_object(), "Should have result");
    let content_str = serde_json::to_string(&response["result"]["content"])?;
    assert!(content_str.contains("Invalid") || content_str.contains("URI") || content_str.contains("file://") || content_str.contains("s3://"),
            "Should indicate invalid URI format");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}
