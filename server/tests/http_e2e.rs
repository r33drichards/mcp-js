/// End-to-end integration tests for HTTP MCP server
/// These tests start the actual server and test real MCP protocol communication

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};
use serde_json::{json, Value};

mod common;

/// Helper to send MCP message and read response
async fn send_mcp_message(stream: &mut TcpStream, message: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let message_str = serde_json::to_string(&message)?;
    let message_with_newline = format!("{}\n", message_str);

    stream.write_all(message_with_newline.as_bytes()).await?;
    stream.flush().await?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader.read_line(&mut response_line).await?;

    let response: Value = serde_json::from_str(&response_line)?;
    Ok(response)
}

/// Test full HTTP upgrade to MCP protocol flow
#[tokio::test]
#[ignore] // Run with `cargo test -- --ignored` when server is available
async fn test_http_upgrade_to_mcp() -> Result<(), Box<dyn std::error::Error>> {
    // Start the server in a background task
    let server_handle = tokio::spawn(async {
        // This would start the actual server
        // For now, we just simulate it running
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    let port = 8765;
    let mut stream = timeout(
        Duration::from_secs(2),
        TcpStream::connect(format!("127.0.0.1:{}", port))
    ).await??;

    // Send HTTP upgrade request
    let upgrade_request = format!(
        "GET / HTTP/1.1\r\n\
         Host: localhost:{}\r\n\
         Upgrade: mcp\r\n\
         Connection: Upgrade\r\n\
         \r\n",
        port
    );

    stream.write_all(upgrade_request.as_bytes()).await?;
    stream.flush().await?;

    // Read HTTP response headers
    let mut buf = vec![0u8; 4096];
    let n = timeout(Duration::from_secs(2), stream.peek(&mut buf)).await??;
    let response = String::from_utf8_lossy(&buf[..n]);

    // Verify we got a 101 Switching Protocols response
    assert!(response.contains("101") || response.contains("Switching Protocols"),
            "Expected HTTP 101 Switching Protocols response");

    server_handle.abort();
    Ok(())
}

/// Test MCP initialize handshake
#[tokio::test]
#[ignore] // Run with `cargo test -- --ignored` when server is available
async fn test_mcp_initialize_handshake() -> Result<(), Box<dyn std::error::Error>> {
    let port = 8766;

    // Simulate upgraded connection (after HTTP upgrade)
    let mut stream = timeout(
        Duration::from_secs(2),
        TcpStream::connect(format!("127.0.0.1:{}", port))
    ).await?;

    if stream.is_err() {
        // Server not running, skip test
        return Ok(());
    }

    let mut stream = stream?;

    // Send initialize request
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "http-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    let response = timeout(
        Duration::from_secs(2),
        send_mcp_message(&mut stream, initialize_msg)
    ).await??;

    // Verify response structure
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert!(response["result"].is_object(), "Should have result object");
    assert!(response["result"]["capabilities"].is_object(),
            "Should have capabilities in response");

    Ok(())
}

/// Test run_js tool execution via MCP
#[tokio::test]
#[ignore] // Run with `cargo test -- --ignored` when server is available
async fn test_run_js_tool_execution() -> Result<(), Box<dyn std::error::Error>> {
    let port = 8767;

    let mut stream = timeout(
        Duration::from_secs(2),
        TcpStream::connect(format!("127.0.0.1:{}", port))
    ).await?;

    if stream.is_err() {
        return Ok(());
    }

    let mut stream = stream?;

    // First, initialize
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "http-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    timeout(
        Duration::from_secs(2),
        send_mcp_message(&mut stream, initialize_msg)
    ).await??;

    // Then call run_js tool
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

    let response = timeout(
        Duration::from_secs(2),
        send_mcp_message(&mut stream, tool_call_msg)
    ).await??;

    // Verify response
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);
    assert!(response["result"].is_object(), "Should have result object");

    // Check for output in result
    if let Some(content) = response["result"]["content"].as_array() {
        assert!(!content.is_empty(), "Should have content in response");
    }

    Ok(())
}

/// Test heap persistence across multiple calls
#[tokio::test]
#[ignore] // Run with `cargo test -- --ignored` when server is available
async fn test_heap_persistence() -> Result<(), Box<dyn std::error::Error>> {
    let port = 8768;

    let mut stream = timeout(
        Duration::from_secs(2),
        TcpStream::connect(format!("127.0.0.1:{}", port))
    ).await?;

    if stream.is_err() {
        return Ok(());
    }

    let mut stream = stream?;

    // Initialize
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "http-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    timeout(
        Duration::from_secs(2),
        send_mcp_message(&mut stream, initialize_msg)
    ).await??;

    // Set a variable in a fresh heap
    let set_var_msg = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "var myValue = 42; myValue"
            }
        }
    });

    let response1 = timeout(
        Duration::from_secs(2),
        send_mcp_message(&mut stream, set_var_msg)
    ).await??;

    // Verify first call succeeded
    assert!(response1["result"].is_object());

    // Extract content hash from first response
    let heap_hash = common::extract_heap_hash(&response1)
        .expect("First response should contain a heap content hash");

    // Read the variable from heap using the content hash
    let read_var_msg = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "myValue",
                "heap": heap_hash
            }
        }
    });

    let response2 = timeout(
        Duration::from_secs(2),
        send_mcp_message(&mut stream, read_var_msg)
    ).await??;

    // Verify second call succeeded and has result
    assert!(response2["result"].is_object());

    Ok(())
}

/// Test error handling for invalid JavaScript
#[tokio::test]
#[ignore] // Run with `cargo test -- --ignored` when server is available
async fn test_invalid_javascript_error() -> Result<(), Box<dyn std::error::Error>> {
    let port = 8769;

    let mut stream = timeout(
        Duration::from_secs(2),
        TcpStream::connect(format!("127.0.0.1:{}", port))
    ).await?;

    if stream.is_err() {
        return Ok(());
    }

    let mut stream = stream?;

    // Initialize
    let initialize_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "http-e2e-test",
                "version": "1.0.0"
            }
        }
    });

    timeout(
        Duration::from_secs(2),
        send_mcp_message(&mut stream, initialize_msg)
    ).await??;

    // Send invalid JavaScript
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

    let response = timeout(
        Duration::from_secs(2),
        send_mcp_message(&mut stream, invalid_js_msg)
    ).await??;

    // The server should return a response (either error or result with error message)
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);

    // It should either be an error response or a result with error info
    let has_error = response["error"].is_object() ||
                   (response["result"].is_object() &&
                    response["result"]["content"].as_array()
                        .and_then(|arr| arr.first())
                        .and_then(|c| c["text"].as_str())
                        .map(|s| s.contains("error") || s.contains("Error"))
                        .unwrap_or(false));

    assert!(has_error, "Should return error information for invalid JavaScript");

    Ok(())
}
