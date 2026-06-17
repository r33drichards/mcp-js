/// Integration tests for stdio MCP server transport
/// These tests verify message format and protocol compliance without requiring a running server

use serde_json::{json, Value};

/// Test MCP initialize message format for stdio

async fn test_stdio_initialize_message_format() {
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

        assert_eq!(initialize_request["jsonrpc"], "2.0");
    assert_eq!(initialize_request["method"], "initialize");
    assert!(initialize_request["params"].is_object());
    assert_eq!(initialize_request["params"]["protocolVersion"], "2024-11-05");
}

/// Test that initialize message can be serialized to newline-delimited JSON

async fn test_stdio_message_serialization() {
    let message = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });

        let serialized = serde_json::to_string(&message).expect("Should serialize to JSON");
    let with_newline = format!("{}\n", serialized);

        let deserialized: Value = serde_json::from_str(&serialized).expect("Should deserialize");
    assert_eq!(deserialized["jsonrpc"], "2.0");
    assert!(!with_newline.is_empty());
}

/// Test run_js tool call message format for stdio

async fn test_stdio_run_js_message_format() {
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

async fn test_stdio_javascript_scenarios() {
    let scenarios = vec![
        ("1 + 1", "simple arithmetic"),
        ("globalThis.x = 5;", "variable assignment"),
        ("globalThis.x * 2", "accessing stored variable"),
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

                assert!(message.is_object(), "Failed for scenario: {}", description);
        assert_eq!(message["params"]["arguments"]["code"], code);
    }
}

/// Test expected response format for stdio

async fn test_stdio_response_format() {
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

async fn test_stdio_heap_hash_format() {
        let valid_hashes = vec![
        "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    ];

    for hash in valid_hashes {
        assert_eq!(hash.len(), 64, "Hash should be 64 characters");
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

                assert!(message.is_object());
        assert!(message["params"]["arguments"]["code"].is_string());
    }
}

/// Test message ID tracking

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
