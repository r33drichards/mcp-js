# How to Configure Transport Protocols

Choose between stdio, Streamable HTTP, and SSE transports for mcp-v8.

## stdio (default)

When no port flags are given, mcp-v8 uses stdio transport. This is the standard mode for MCP clients that launch the server as a subprocess.

```bash
mcp-v8
```

The server reads JSON-RPC from stdin and writes to stdout.

## Streamable HTTP

Use `--http-port` to enable the Streamable HTTP transport (MCP 2025-03-26 spec). This is the recommended network transport -- it supports load balancing and works behind reverse proxies.

```bash
mcp-v8 --http-port 3000
```

Endpoints:

- `POST /mcp` -- MCP JSON-RPC over HTTP
- `POST /api/exec` -- REST API
- `GET /swagger-ui` -- interactive API docs
- `GET /api-doc/openapi.json` -- OpenAPI spec

## SSE (legacy)

Use `--sse-port` for the older HTTP+SSE transport:

```bash
mcp-v8 --sse-port 3000
```

Endpoints:

- `GET /sse` -- SSE event stream
- `POST /message` -- send JSON-RPC messages

## Constraints

- `--http-port` and `--sse-port` are mutually exclusive
- stdio is used when neither port flag is set
- Cluster mode requires `--http-port` (not SSE or stdio)

## Print the OpenAPI spec

Generate the OpenAPI JSON without starting the server:

```bash
mcp-v8 --print-openapi > openapi.json
```
