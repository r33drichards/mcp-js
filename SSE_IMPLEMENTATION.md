# SSE Transport Implementation

This document describes the SSE (Server-Sent Events) transport implementation added to the MCP server.

## Overview

The server now supports three transport methods:
1. **stdio** (default) - Standard input/output transport
2. **HTTP** - HTTP upgrade transport
3. **SSE** - Server-Sent Events transport (newly added)

## Implementation Details

### Dependencies Added

The following dependencies were added to `server/Cargo.toml`:

```toml
# SSE transport support
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main", features = ["transport-io", "transport-sse-server"] }
tokio-util = { version = "0.7", features = ["sync"] }
```

### Code Changes

#### Main Imports
Added SSE server imports to `server/src/main.rs`:

```rust
use rmcp::transport::sse_server::{SseServer, SseServerConfig};
use tokio_util::sync::CancellationToken;
```

#### CLI Arguments
Added `--sse-port` option to choose SSE transport:

```rust
/// SSE port to listen on (if not specified, uses stdio transport)
#[arg(long, conflicts_with = "http_port")]
sse_port: Option<u16>,
```

#### SSE Server Function
Implemented `start_sse_server()` function based on the Rust SDK examples:

```rust
async fn start_sse_server(heap_storage: AnyHeapStorage, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port).parse()?;

    let config = SseServerConfig {
        bind: addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (sse_server, router) = SseServer::new(config);

    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    tracing::info!("SSE server listening on {}", sse_server.config.bind);

    let ct = sse_server.config.ct.clone();
    let ct_shutdown = ct.child_token();

    // Start the HTTP server with graceful shutdown
    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("SSE server shutting down");
    });

    // Spawn the server task
    tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!("SSE server error: {:?}", e);
        }
    });

    // Register the service with SSE server
    sse_server.with_service(move || {
        let storage = heap_storage.clone();
        async move { GenericService::new(storage).await }
    });

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    tracing::info!("Received Ctrl+C, shutting down SSE server");
    ct.cancel();

    Ok(())
}
```

## Usage

### Starting the SSE Server

To start the server with SSE transport on port 8000:

```bash
cargo run -p server -- --sse-port 8000
```

With custom heap storage:

```bash
# With filesystem storage
cargo run -p server -- --sse-port 8000 --directory-path /path/to/heaps

# With S3 storage
cargo run -p server -- --sse-port 8000 --s3-bucket my-bucket
```

### SSE Endpoints

When the SSE server is running, it exposes two endpoints:

1. **SSE endpoint**: `http://localhost:8000/sse` - For receiving server-sent events
2. **Message endpoint**: `http://localhost:8000/message` - For sending messages to the server

### Client Connection

To connect a client to the SSE server, use the `SseClientTransport`:

```rust
use rmcp::{ServiceExt, transport::SseClientTransport, model::*};

#[tokio::main]
async fn main() -> Result<()> {
    let transport = SseClientTransport::start("http://localhost:8000/sse").await?;

    let client_info = ClientInfo {
        protocol_version: Default::default(),
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "my-client".to_string(),
            title: None,
            version: "0.1.0".to_string(),
            website_url: None,
            icons: None,
        },
    };

    let client = client_info.serve(transport).await?;

    // Use the client...
    let tools = client.list_tools(Default::default()).await?;
    println!("Available tools: {:#?}", tools);

    Ok(())
}
```

## Features

- **Keep-alive**: The server sends keep-alive messages every 15 seconds to maintain the connection
- **Graceful shutdown**: Supports Ctrl+C for graceful shutdown
- **Concurrent connections**: Handles multiple concurrent SSE connections
- **Tool support**: Exposes the same MCP tools (like `run_js`) as other transports
- **Heap storage**: Supports both file-based and S3-based heap storage

## Architecture

The SSE implementation follows the MCP (Model Context Protocol) specification:

1. Client connects to the SSE endpoint (`/sse`)
2. Server establishes a unidirectional SSE stream for server-to-client messages
3. Client sends requests via HTTP POST to the message endpoint (`/message`)
4. Server processes requests and sends responses via the SSE stream
5. Keep-alive events maintain the connection

## Comparison with Other Transports

| Transport | Use Case | Pros | Cons |
|-----------|----------|------|------|
| stdio | CLI tools, local processes | Simple, low overhead | Single connection only |
| HTTP | Web applications | Standard protocol, firewall-friendly | Requires upgrade negotiation |
| SSE | Browser clients, streaming | Browser-native, simple setup | HTTP-based overhead |

## References

- Implementation based on: https://github.com/modelcontextprotocol/rust-sdk/tree/main/examples
- Key examples used:
  - `examples/servers/src/counter_sse.rs` - Simple SSE server
  - `examples/servers/src/complex_auth_sse.rs` - SSE with authentication
  - `examples/clients/src/sse.rs` - SSE client
