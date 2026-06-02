# Transport Protocols

mcp-v8 supports three transport protocols for MCP communication. Each serves a different deployment scenario, and the choice of transport affects how clients connect, how sessions are managed, and whether load balancing is possible.

## stdio (Default)

The stdio transport is the simplest: mcp-v8 reads JSON-RPC messages from standard input and writes responses to standard output. Diagnostic logging goes to standard error.

This is the default when neither `--http-port` nor `--sse-port` is specified. It is designed for local MCP clients like Claude Desktop, Cursor, or any MCP host that launches the server as a child process.

Key characteristics:

- **One client, one server** -- stdio is inherently point-to-point. The server handles exactly one client connection.
- **No network exposure** -- The server is not listening on any port. There is no attack surface from the network.
- **Process lifecycle coupling** -- When the client exits, the server's stdin closes, and the server shuts down.
- **No load balancing** -- Not applicable; the server is a child process of the client.

stdio is the right choice for desktop MCP clients and development/testing scenarios.

## Streamable HTTP (MCP 2025-03-26+)

Streamable HTTP is the modern MCP transport, introduced in the MCP specification revision of March 2025. It uses a single HTTP endpoint (`/mcp` by default) that handles both request-response and server-to-client streaming.

Enable it with `--http-port <port>`. The server binds to `0.0.0.0:<port>` and serves:

- `/mcp` -- The MCP endpoint (POST for client-to-server messages, GET for server-initiated streams)
- `/api/*` -- The REST API endpoints (always available alongside MCP)
- `/api-doc/openapi.json` -- The OpenAPI specification

Key characteristics:

- **Bidirectional** -- Clients send requests via POST; the server can stream responses and notifications via SSE on the same connection.
- **Session management** -- The `X-MCP-Session-Id` header identifies MCP sessions. The server generates a session ID during the `initialize` handshake.
- **Load balanceable** -- Because sessions are identified by header, requests from the same session can be routed to the same backend by a load balancer using header-based affinity.
- **Standard HTTP** -- Works with any HTTP client, reverse proxy, or API gateway. No special protocol support needed beyond SSE for streaming.
- **Graceful shutdown** -- The server supports `Ctrl+C` with a cancellation token that triggers graceful connection draining.

Streamable HTTP is the recommended transport for production deployments, multi-client setups, and any scenario involving a network.

## SSE (Legacy)

The SSE transport is the older MCP transport that uses two separate HTTP endpoints:

- `/sse` -- A GET endpoint that establishes a Server-Sent Events stream. The server sends notifications and responses through this stream.
- `/message` -- A POST endpoint where clients send JSON-RPC messages.

Enable it with `--sse-port <port>`. Like Streamable HTTP, the REST API and OpenAPI spec are served alongside the MCP endpoints.

Key characteristics:

- **Two-endpoint design** -- The separation between the event stream (`/sse`) and the message endpoint (`/message`) means clients must manage two connections.
- **SSE keep-alive** -- The server sends periodic keep-alive events (every 15 seconds) to prevent proxies from closing idle connections.
- **Broader client support** -- Some older MCP clients only support the SSE transport. If your client predates the Streamable HTTP specification, use this transport.

SSE is supported for backward compatibility. New deployments should prefer Streamable HTTP.

## When to Use Which

| Scenario | Transport |
|---|---|
| Local MCP client (Claude Desktop, Cursor) | stdio |
| Production server, modern clients | Streamable HTTP (`--http-port`) |
| Production server, legacy clients | SSE (`--sse-port`) |
| Development and testing | stdio or Streamable HTTP |
| Cluster deployment | Streamable HTTP (required) |
| Behind a load balancer | Streamable HTTP |

Cluster mode requires either `--http-port` or `--sse-port` because Raft peer communication uses HTTP. stdio is not supported in cluster mode.

## Shared Infrastructure

Regardless of transport, all three protocols serve the same MCP tools, use the same Engine, and share the REST API when running on an HTTP port. The transport is purely the communication layer -- the MCP service and tool implementations are transport-agnostic.
