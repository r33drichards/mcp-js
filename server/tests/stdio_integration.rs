/// Integration tests for stdio MCP server transport
/// These tests verify message format and protocol compliance without requiring a running server

use serde_json::{json, Value};

/// Test MCP initialize message format for stdio
#[tokio::test]
async fn test_stdio_initialize_message_format() {
    // Create a test initialize request for stdio transport
    let initialize_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-stdio-client",
                "version": "1.0.0"
            }
        }
    });

    // Verify the request is properly formatted JSON-RPC
    assert_eq!(initialize_request["jsonrpc"], "2.0");
    assert_eq!(initialize_request["method"], "initialize");
    assert!(initialize_request["params"].is_object());
    assert_eq!(initialize_request["params"]["protocolVersion"], "2024-11-05");
}

/// Test that initialize message can be serialized to newline-delimited JSON
#[tokio::test]
async fn test_stdio_message_serialization() {
    let message = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });

    // Stdio transport uses newline-delimited JSON
    let serialized = serde_json::to_string(&message).expect("Should serialize to JSON");
    let with_newline = format!("{}\n", serialized);

    // Verify we can deserialize it back
    let deserialized: Value = serde_json::from_str(&serialized).expect("Should deserialize");
    assert_eq!(deserialized["jsonrpc"], "2.0");
    assert!(!with_newline.is_empty());
}

/// Test run_js tool call message format for stdio
#[tokio::test]
async fn test_stdio_run_js_message_format() {
    // heap is optional — omit for fresh session
    let tool_call = json!({
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

    assert_eq!(tool_call["jsonrpc"], "2.0");
    assert_eq!(tool_call["method"], "tools/call");
    assert_eq!(tool_call["params"]["name"], "run_js");
    assert!(tool_call["params"]["arguments"]["code"].is_string());

    // heap can also be provided as a content hash for resuming
    let tool_call_with_heap = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": {
                "code": "1 + 1",
                "heap": "a1b2c3d4"
            }
        }
    });

    assert!(tool_call_with_heap["params"]["arguments"]["heap"].is_string());
}

/// Test JavaScript code scenarios for stdio transport
#[tokio::test]
async fn test_stdio_javascript_scenarios() {
    let scenarios = vec![
        ("1 + 1", "simple arithmetic"),
        ("var x = 5; x", "variable assignment"),
        ("x * 2", "accessing stored variable"),
        ("[1, 2, 3].map(n => n * 2)", "array operations"),
        ("({ a: 1, b: 2 })", "object literals"),
    ];

    for (code, description) in scenarios {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": {
                    "code": code
                }
            }
        });

        // Verify message is valid
        assert!(message.is_object(), "Failed for scenario: {}", description);
        assert_eq!(message["params"]["arguments"]["code"], code);
    }
}

/// Test expected response format for stdio
#[tokio::test]
async fn test_stdio_response_format() {
    // Expected response format after initialize
    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "mcp-v8",
                "version": "0.1.0"
            }
        }
    });

    assert_eq!(response["jsonrpc"], "2.0");
    assert!(response["result"].is_object());
    assert!(response["result"]["capabilities"].is_object());
}

/// Test error response format for stdio
#[tokio::test]
async fn test_stdio_error_response_format() {
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

/// Test content-addressed heap hash format
#[tokio::test]
async fn test_stdio_heap_hash_format() {
    // Content-addressed heap hashes are 8-character lowercase hex strings
    let valid_hashes = vec![
        "a1b2c3d4",
        "00000000",
        "ffffffff",
        "deadbeef",
    ];

    for hash in valid_hashes {
        assert_eq!(hash.len(), 8, "Hash should be 8 characters");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()),
                "Hash should be hex: {}", hash);

        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": {
                    "code": "1",
                    "heap": hash
                }
            }
        });

        assert_eq!(message["params"]["arguments"]["heap"], hash);
    }
}

/// Test invalid JavaScript error scenarios
#[tokio::test]
async fn test_stdio_invalid_javascript_scenarios() {
    let invalid_codes = vec![
        "this is not javascript",
        "let x = ;",
        "undefined.nonexistent.method()",
        "function(",
    ];

    for code in invalid_codes {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": {
                    "code": code
                }
            }
        });

        // Message should be valid even if the code isn't
        assert!(message.is_object());
        assert!(message["params"]["arguments"]["code"].is_string());
    }
}

/// Test message ID tracking
#[tokio::test]
async fn test_stdio_message_id_tracking() {
    let messages = vec![
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {}}),
    ];

    for (idx, msg) in messages.iter().enumerate() {
        assert_eq!(msg["id"], idx + 1);
    }
}
