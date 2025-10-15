# Integration Tests for HTTP MCP Server

This directory contains integration tests for the HTTP transport implementation of the MCP (Model Context Protocol) server.

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

## Test Structure

```
tests/
├── README.md              # This file
├── common/
│   └── mod.rs            # Common test utilities
├── http_integration.rs   # Basic integration tests
└── http_e2e.rs          # End-to-end tests
```

## Running All Tests

### Run all non-ignored tests:
```bash
cargo test
```

### Run specific test file:
```bash
cargo test --test http_integration
cargo test --test http_e2e
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

2. **MCP Protocol**
   - Initialize handshake
   - Tool calls (run_js)
   - JSON-RPC message format
   - Error responses

3. **JavaScript Execution**
   - Simple expressions (1 + 1)
   - Variable assignment and persistence
   - Heap storage and retrieval
   - Error handling for invalid code

4. **Heap Management**
   - Heap naming
   - Persistence across calls
   - Multiple concurrent heaps

## Adding New Tests

To add a new test:

1. For basic protocol/format tests, add to `http_integration.rs`
2. For full e2e tests requiring a server, add to `http_e2e.rs` and mark with `#[ignore]`
3. For shared test utilities, add to `common/mod.rs`

### Example Test Structure:

```rust
#[tokio::test]
async fn test_my_feature() -> Result<(), Box<dyn std::error::Error>> {
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

## CI/CD Integration

For CI/CD pipelines, run the non-ignored tests:
```bash
cargo test
```

The e2e tests can be run in CI by:
1. Starting the server in background
2. Running `cargo test -- --ignored`
3. Stopping the server

## Troubleshooting

### Tests fail to connect
- Ensure the server is running on the expected port
- Check firewall settings
- Verify the port is not already in use

### Timeout errors
- Increase timeout durations in test code
- Check server logs for errors
- Verify server is responding to requests

### Compilation errors
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
