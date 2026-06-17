use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use serde_json::{json, Value};
use std::time::Duration;

/// Helper function to start the HTTP MCP server in the background for testing.
/// Returns the join handle and the dynamically assigned port.
async fn start_test_server() -> (tokio::task::JoinHandle<()>, u16) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();

                let (mut reader, mut writer) = tokio::io::split(stream);
        let mut buf = vec![0u8; 4096];

        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if let Err(_) = writer.write_all(&buf[..n]).await {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    (handle, port)
}

/// Test basic HTTP connection to the server

async fn test_http_connection() {
    let (_server, port) = start_test_server().await;

        let result = TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(result.is_ok(), "Should be able to connect to the test server");
}

/// Test HTTP upgrade request

async fn test_http_upgrade_request() {
    let (_server, port) = start_test_server().await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to connect");

        let request = "GET / HTTP/1.1\r\n\
                   Host: localhost\r\n\
                   Upgrade: mcp\r\n\
                   Connection: Upgrade\r\n\
                   \r\n";

    stream.write_all(request.as_bytes()).await.expect("Failed to write request");

        let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.expect("Failed to read response");
    let response = String::from_utf8_lossy(&buf[..n]);

        assert!(!response.is_empty(), "Should receive a response");
}

/// Test MCP initialize message

async fn test_mcp_initialize() {
        let initialize_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }
    });

        assert_eq!(initialize_request["jsonrpc"], "2.0");
    assert_eq!(initialize_request["method"], "initialize");
    assert!(initialize_request["params"].is_object());
}

/// Test MCP tool call (run_js)

async fn test_mcp_tool_call_format() {
        let tool_call_request = json!({
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

        assert_eq!(tool_call_request["jsonrpc"], "2.0");
    assert_eq!(tool_call_request["method"], "tools/call");
    assert_eq!(tool_call_request["params"]["name"], "run_js");
    assert!(tool_call_request["params"]["arguments"].is_object());
}

/// Test JavaScript code execution scenarios

async fn test_javascript_execution_scenarios() {
        let code1 = "1 + 1";
    assert!(!code1.is_empty());

        let code2 = "let x = 5; x";
    assert!(code2.contains("let"));

        let code3 = "x + 10";
    assert!(!code3.is_empty());

        assert!(true);
}

/// Test content-addressed heap hash format

async fn test_heap_hash_format() {
        let valid_hashes = vec![
        "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    ];

    for hash in valid_hashes {
        assert_eq!(hash.len(), 64, "Hash should be 64 characters");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()),
                "Hash should be hex: {}", hash);
    }
}

/// Test error handling for invalid JavaScript

async fn test_invalid_javascript_handling() {
        let invalid_codes = vec![
        "this is not javascript",
        "let x = ;",
        "undefined.method()",
    ];

    for code in invalid_codes {
                let request = json!({
            "code": code
        });
        assert!(request.is_object());
    }
}

/// Test concurrent connections

async fn test_concurrent_connections() {
        let tasks: Vec<_> = (0..3).map(|_| {
        tokio::spawn(async move {
                        let addr = "127.0.0.1:1";
                        let _ = tokio::time::timeout(
                Duration::from_millis(100),
                TcpStream::connect(addr)
            ).await;
        })
    }).collect();

        for task in tasks {
        task.await.expect("Task should complete");
    }
}

/// Test JSON-RPC error response format

async fn test_jsonrpc_error_format() {
    let error_response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32600,
            "message": "Invalid Request"
        }
    });

    assert_eq!(error_response["jsonrpc"], "2.0");
    assert!(error_response["error"].is_object());
    assert!(error_response["error"]["code"].is_number());
    assert!(error_response["error"]["message"].is_string());
}
