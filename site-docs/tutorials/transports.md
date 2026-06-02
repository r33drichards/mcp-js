# Tutorial: Connecting via Different Transports

In this tutorial you will set up mcp-v8 with each of its three transport options -- stdio, HTTP, and SSE (Server-Sent Events) -- and test each one. You will understand when to use each transport and how they differ.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- `curl` installed (for testing HTTP and SSE)

## Step 1: Understand the three transports

mcp-v8 supports three ways for clients to communicate with it:

| Transport | Flag | Use case |
|---|---|---|
| **stdio** | (default, no flag) | Direct integration with AI clients like Claude Desktop |
| **HTTP** | `--http-port <PORT>` | REST API access, CLI client, programmatic use |
| **SSE** | `--sse-port <PORT>` | MCP protocol over Server-Sent Events for web clients |

You can enable multiple transports simultaneously.

## Step 2: Use stdio transport (default)

Stdio transport is the default. The server reads JSON-RPC messages from stdin and writes responses to stdout. This is how most AI clients (Claude Desktop, Claude Code) communicate with MCP servers.

Start the server with stdio:

```bash
mcp-v8
```

The server is now waiting for JSON-RPC input on stdin. You can send a message manually:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' | mcp-v8
```

This returns a JSON-RPC response listing all available tools. Stdio transport is not meant for interactive use -- it is designed for AI clients to pipe messages back and forth.

Press `Ctrl+C` to stop the server.

## Step 3: Start HTTP transport

HTTP transport exposes a REST API that you can call with curl or any HTTP client:

```bash
mcp-v8 --http-port 3000
```

The server is now listening on port 3000.

## Step 4: Test HTTP transport with curl

Send a request to execute JavaScript:

```bash
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{"code": "1 + 1"}' | jq .
```

Expected output (formatted):

```json
{
  "result": "2"
}
```

The HTTP API is covered in detail in the [REST API tutorial](http-api.md).

Press `Ctrl+C` to stop the server.

## Step 5: Start SSE transport

SSE (Server-Sent Events) transport implements the MCP protocol over HTTP with server-sent events for streaming:

```bash
mcp-v8 --sse-port 8080
```

The server is now listening for SSE connections on port 8080.

## Step 6: Test SSE transport

Connect to the SSE endpoint:

```bash
curl -s http://localhost:8080/sse
```

This opens a streaming connection. The server sends events as they occur. Press `Ctrl+C` to disconnect.

SSE transport is primarily used by web-based MCP clients and tools that understand the SSE-based MCP protocol.

## Step 7: Run all three transports simultaneously

You can enable all transports at once:

```bash
mcp-v8 --http-port 3000 --sse-port 8080
```

This starts:
- stdio on stdin/stdout
- HTTP on port 3000
- SSE on port 8080

All three share the same underlying server state. An execution started via HTTP can be queried via SSE, and vice versa.

## Step 8: Verify all transports are working

In a second terminal, test HTTP:

```bash
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{"code": "\"hello from HTTP\""}' | jq .
```

Test SSE connectivity:

```bash
curl -s -I http://localhost:8080/sse
```

You should see a `200` response with `Content-Type: text/event-stream`.

## Choosing the right transport

- **stdio**: Use when integrating with AI clients (Claude Desktop, Claude Code, Cursor). This is the standard MCP transport and requires no network configuration.
- **HTTP**: Use when you want a REST API, are calling from scripts or the CLI client, or need request/response semantics.
- **SSE**: Use when you need the MCP protocol over HTTP (web-based clients, remote MCP connections).

## What you learned

- mcp-v8 supports three transport options: stdio, HTTP, and SSE
- stdio is the default and is designed for AI client integration
- HTTP provides a REST API accessible via curl or any HTTP client
- SSE implements the MCP protocol over Server-Sent Events
- All three transports can run simultaneously and share server state

Next, learn how to integrate with specific AI clients in [Integrating with AI Clients](mcp-protocol.md).
