# SSE Transport Implementation

This document describes the SSE (Server-Sent Events) transport implementation in the MCP server.

## Overview

The server supports three transport methods:
1. **stdio** (default) - Standard input/output transport
2. **Streamable HTTP** (`--http-port`) - Bidirectional HTTP transport (MCP 2025-03-26+), serves MCP at `/mcp` and a plain API at `/api/exec`
3. **SSE** (`--sse-port`) - Server-Sent Events transport

## Implementation Details

### Dependencies

The SSE transport uses the following crates from the MCP Rust SDK:

```toml
rmcp = { ..., features = ["transport-io", "transport-sse-server"] }
tokio-util = { version = "0.7", features = ["sync"] }
```

### Code Structure

The SSE server is implemented in `server/src/main.rs` via the `start_sse_server()` function. It creates an `SseServer` from the `rmcp` crate and merges its router with the plain HTTP API router from `server/src/api.rs`.

#### SSE Server Function

```rust
async fn start_sse_server(engine: Engine, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port).parse()?;

    let config = SseServerConfig {
        bind: addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (sse_server, sse_router) = SseServer::new(config);

    // Merge SSE router with plain HTTP API router
    let app = sse_router.merge(api::api_router(engine.clone()));

    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;

    let ct = sse_server.config.ct.clone();
    let ct_shutdown = ct.child_token();

    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
    });

    sse_server.with_service(move || McpService::new(engine.clone()));

    tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!("SSE server error: {:?}", e);
        }
    });

    tokio::signal::ctrl_c().await?;
    ct.cancel();

    Ok(())
}
```

## Usage

### Starting the SSE Server

```bash
# With local filesystem storage
mcp-v8 --sse-port 8000 --directory-path /tmp/mcp-v8-heaps

# With S3 storage
mcp-v8 --sse-port 8000 --s3-bucket my-bucket

# Stateless mode
mcp-v8 --sse-port 8000 --stateless
```

### SSE Endpoints

When the SSE server is running, it exposes:

1. **SSE endpoint**: `http://localhost:8000/sse` - For receiving server-sent events
2. **Message endpoint**: `http://localhost:8000/message` - For sending messages to the server
3. **API endpoint**: `http://localhost:8000/api/exec` - Plain HTTP API for direct JavaScript execution

### Client Connection

To connect a client to the SSE server using the MCP Rust SDK:

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

    // List available tools
    let tools = client.list_tools(Default::default()).await?;
    println!("Available tools: {:#?}", tools);

    Ok(())
}
```

## Features

- **Keep-alive**: The server sends keep-alive messages every 15 seconds to maintain the connection
- **Graceful shutdown**: Supports Ctrl+C for graceful shutdown
- **Concurrent connections**: Handles multiple concurrent SSE connections
- **Tool support**: Exposes the same MCP tools (`run_js`, `list_sessions`, `list_session_snapshots`) as other transports
- **Plain HTTP API**: Also serves `POST /api/exec` for direct JavaScript execution without MCP framing

## Architecture

The SSE implementation follows the MCP (Model Context Protocol) specification:

1. Client connects to the SSE endpoint (`/sse`)
2. Server establishes a unidirectional SSE stream for server-to-client messages
3. Client sends requests via HTTP POST to the message endpoint (`/message`)
4. Server processes requests and sends responses via the SSE stream
5. Keep-alive events maintain the connection

## Comparison with Other Transports

| Transport | Flag | Use Case | Pros | Cons |
|-----------|------|----------|------|------|
| stdio | (default) | CLI tools, local processes | Simple, low overhead | Single connection only |
| Streamable HTTP | `--http-port` | Web apps, load-balanced deployments | Standard HTTP, scalable, supports `/api/exec` | Newer protocol (MCP 2025-03-26+) |
| SSE | `--sse-port` | Browser clients, streaming | Browser-native, simple setup, supports `/api/exec` | Unidirectional server→client stream |

## References

- MCP Rust SDK: https://github.com/modelcontextprotocol/rust-sdk
