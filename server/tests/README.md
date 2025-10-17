# Integration Tests for MCP Server

This directory contains integration tests for both HTTP and stdio transport implementations of the MCP (Model Context Protocol) server.

## Test Files

### http_integration.rs
Basic integration tests that verify:
- HTTP connection establishment
- HTTP upgrade request/response format
- MCP protocol message structure
- JSON-RPC message format
- JavaScript execution scenarios
- Heap naming conventions
- Error handling
- Concurrent connections

These tests **do not** require a running server and test protocol compliance and message formatting.

**Run with:**
```bash
cargo test --test http_integration
```

**Current status:** ✅ All 9 tests passing

### http_e2e.rs
End-to-end integration tests that test the full MCP protocol flow:
- HTTP upgrade to MCP protocol
- MCP initialize handshake
- JavaScript execution via `run_js` tool
- Heap persistence across multiple calls
- Error handling for invalid JavaScript

These tests are marked with `#[ignore]` and require a running HTTP MCP server.

**Run with:**
```bash
# First, start the server in one terminal:
cargo run -- --directory-path /tmp/test-heaps --http-port 8765

# Then, in another terminal, run the e2e tests:
cargo test --test http_e2e -- --ignored
```

**Current status:** ⚠️ Ignored by default (requires running server)

### stdio_integration.rs
Basic integration tests for stdio transport that verify:
- MCP message format for stdio
- Newline-delimited JSON serialization
- run_js tool call message format
- JavaScript execution scenarios
- Heap naming conventions
- Error response format
- Message ID tracking

These tests **do not** require a running server and test protocol compliance and message formatting.

**Run with:**
```bash
cargo test --test stdio_integration
```

**Current status:** ✅ All 10 tests passing

### stdio_e2e.rs
End-to-end integration tests for stdio transport that test:
- MCP initialize handshake via stdin/stdout
- JavaScript execution via `run_js` tool
- Heap persistence across multiple calls
- Error handling for invalid JavaScript
- Sequential operations with state preservation
- Multiple independent heaps
- Complex JavaScript operations (arrays, objects, etc.)
- Graceful server shutdown

These tests spawn the actual server process and communicate via stdin/stdout.

**Run with:**
```bash
cargo test --test stdio_e2e
```

**Current status:** ✅ All 8 E2E tests

### sse_integration.rs
Basic integration tests for SSE (Server-Sent Events) transport that verify:
- SSE endpoint format (/sse and /message)
- SSE connection setup
- MCP message format for SSE
- POST message format to SSE server
- run_js tool call message format
- JavaScript execution scenarios
- Heap naming conventions
- Error response format
- SSE keep-alive configuration
- SSE server configuration

These tests **do not** require a running server and test protocol compliance and message formatting.

**Run with:**
```bash
cargo test --test sse_integration
```

**Current status:** ✅ All tests passing

### sse_e2e.rs
End-to-end integration tests for SSE transport that test:
- SSE server startup and connection acceptance
- HTTP POST messages to SSE server
- MCP initialize handshake via SSE
- JavaScript execution via `run_js` tool
- Heap persistence across multiple calls
- Error handling for invalid JavaScript
- Sequential operations with state preservation
- SSE keep-alive behavior

These tests spawn the actual server process with SSE transport enabled.

**Run with:**
```bash
cargo test --test sse_e2e
```

**Current status:** ✅ E2E tests for SSE transport

## Test Structure

```
tests/
├── README.md              # This file
├── common/
│   └── mod.rs            # Common test utilities
├── http_integration.rs   # HTTP basic integration tests
├── http_e2e.rs           # HTTP end-to-end tests
├── stdio_integration.rs  # Stdio basic integration tests
├── stdio_e2e.rs          # Stdio end-to-end tests
├── sse_integration.rs    # SSE basic integration tests
└── sse_e2e.rs            # SSE end-to-end tests
```

## Running All Tests

### Run all non-ignored tests:
```bash
cargo test
```

### Run specific test file:
```bash
# HTTP tests
cargo test --test http_integration
cargo test --test http_e2e

# Stdio tests
cargo test --test stdio_integration
cargo test --test stdio_e2e

# SSE tests
cargo test --test sse_integration
cargo test --test sse_e2e
```

### Run ignored tests (requires server):
```bash
cargo test -- --ignored
```

### Run all tests including ignored:
```bash
cargo test -- --include-ignored
```

## Test Coverage

The integration tests cover:

1. **HTTP Transport**
   - TCP connection establishment
   - HTTP upgrade mechanism
   - Protocol switching to MCP

2. **Stdio Transport**
   - Stdin/stdout communication
   - Newline-delimited JSON formatting
   - Process spawning and management
   - Graceful shutdown

3. **SSE Transport**
   - Server-Sent Events streaming
   - HTTP POST for client messages
   - SSE endpoint (/sse) and message endpoint (/message)
   - Keep-alive messages
   - Concurrent SSE connections

4. **MCP Protocol**
   - Initialize handshake (HTTP, stdio, and SSE)
   - Tool calls (run_js)
   - JSON-RPC message format
   - Error responses
   - Message ID tracking

5. **JavaScript Execution**
   - Simple expressions (1 + 1)
   - Variable assignment and persistence
   - Heap storage and retrieval
   - Error handling for invalid code
   - Complex operations (arrays, objects, reduce, etc.)
   - Sequential operations with state preservation

5. **Heap Management**
   - Heap naming conventions
   - Persistence across calls
   - Multiple independent heaps
   - Isolation between different heaps

## Adding New Tests

To add a new test:

1. For basic protocol/format tests:
   - HTTP: add to `http_integration.rs`
   - Stdio: add to `stdio_integration.rs`
   - SSE: add to `sse_integration.rs`
2. For full e2e tests requiring a server:
   - HTTP: add to `http_e2e.rs` and mark with `#[ignore]`
   - Stdio: add to `stdio_e2e.rs`
   - SSE: add to `sse_e2e.rs`
3. For shared test utilities, add to `common/mod.rs`

### Example Test Structures:

**HTTP Test:**
```rust
#[tokio::test]
async fn test_http_feature() -> Result<(), Box<dyn std::error::Error>> {
    // Setup
    let mut stream = TcpStream::connect("127.0.0.1:8765").await?;

    // Test
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "my_method",
        "params": {}
    });

    // Verify
    assert!(response.is_ok());
    Ok(())
}
```

**Stdio Test:**
```rust
#[tokio::test]
async fn test_stdio_feature() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = StdioServer::start(&heap_dir).await?;

    // Send message to stdio server
    let message = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });

    let response = server.send_message(message).await?;

    // Verify
    assert_eq!(response["jsonrpc"], "2.0");

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}
```

## CI/CD Integration

For CI/CD pipelines, run the non-ignored tests:
```bash
cargo test
```

This will run:
- All HTTP integration tests (`http_integration.rs`)
- All stdio integration tests (`stdio_integration.rs`)
- All stdio E2E tests (`stdio_e2e.rs`) - these spawn their own server process
- All SSE integration tests (`sse_integration.rs`)
- All SSE E2E tests (`sse_e2e.rs`) - these spawn their own server process

The HTTP E2E tests can be run in CI by:
1. Starting the HTTP server in background: `cargo run -- --directory-path /tmp/test-heaps --http-port 8765 &`
2. Running `cargo test --test http_e2e -- --ignored`
3. Stopping the server

The stdio and SSE E2E tests are already included in the default test run since they manage their own server process lifecycle.

## Troubleshooting

### HTTP Tests

**Tests fail to connect:**
- Ensure the server is running on the expected port
- Check firewall settings
- Verify the port is not already in use

**Timeout errors:**
- Increase timeout durations in test code
- Check server logs for errors
- Verify server is responding to requests

### Stdio Tests

**Tests hang or timeout:**
- Ensure cargo is in PATH (stdio tests spawn `cargo run`)
- Check server initialization time (may need to increase sleep duration)
- Verify heap directory is writable
- Check for V8 initialization issues

**Server spawn errors:**
- Ensure project compiles: `cargo build`
- Verify all dependencies are available
- Check that test has permissions to spawn child processes

**Message parsing errors:**
- Verify newline-delimited JSON format
- Check for extra whitespace or formatting issues
- Ensure server output goes to stdout (not stderr)

### SSE Tests

**Tests fail to connect:**
- Ensure the SSE server is running on the expected port
- Check that the port is not already in use
- Verify firewall settings allow HTTP connections

**Timeout errors:**
- Increase timeout durations in test code
- Check server logs for SSE connection errors
- Verify SSE endpoint (/sse) and message endpoint (/message) are accessible

**Server spawn errors:**
- Ensure project compiles: `cargo build`
- Verify all dependencies are available (including reqwest for HTTP client)
- Check that test has permissions to spawn child processes

**SSE stream errors:**
- Verify server is sending proper SSE format (event: message, data: ...)
- Check that keep-alive messages are being sent
- Ensure POST requests to /message endpoint are properly formatted

### General

**Compilation errors:**
- Ensure all dependencies are installed: `cargo build --tests`
- Check Rust version compatibility
- Verify tokio features are enabled

## Dependencies

Test dependencies (in `Cargo.toml`):
```toml
[dev-dependencies]
tokio-test = "0.4"

[dependencies]
tokio = { version = "1.45.0", features = ["rt-multi-thread", "process"] }
```
