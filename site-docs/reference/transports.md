# Transport Reference

mcp-v8 supports three MCP transport mechanisms. Only one can be active at a time.

## stdio (Default)

JSON-RPC 2.0 messages over standard input/output.

- **Input**: stdin (line-delimited JSON-RPC)
- **Output**: stdout (line-delimited JSON-RPC)
- **Logging**: stderr

Activated when neither `--http-port` nor `--sse-port` is specified.

```
mcp-v8
mcp-v8 --stateless
```

Not supported in cluster mode.

## Streamable HTTP

MCP 2025-03-26 Streamable HTTP transport. Load-balanceable.

| Property | Value |
|----------|-------|
| Flag | `--http-port <PORT>` |
| MCP endpoint | `POST /mcp` |
| Bind address | `0.0.0.0:{PORT}` |
| SSE keep-alive | 15 seconds |
| Conflicts with | `--sse-port` |

Activated with `--http-port`:

```
mcp-v8 --http-port 8080
mcp-v8 --http-port 8080 --stateless
```

The HTTP server also serves:

| Path | Method | Description |
|------|--------|-------------|
| `/mcp` | POST | MCP Streamable HTTP endpoint |
| `/api/exec` | POST | REST API: submit execution |
| `/api/executions` | GET | REST API: list executions |
| `/api/executions/{id}` | GET | REST API: get execution status |
| `/api/executions/{id}/output` | GET | REST API: get console output |
| `/api/executions/{id}/cancel` | POST | REST API: cancel execution |
| `/api/version` | GET | Server version |
| `/api/cli` | GET | CLI download index |
| `/api/cli/{platform}` | GET | CLI binary download |
| `/llms.txt` | GET | Machine-readable agent guide |
| `/docs` | GET | Full README |
| `/api-doc/openapi.json` | GET | OpenAPI 3.0 specification |
| `/` | GET | Redirect to `/llms.txt` |

## SSE (Server-Sent Events)

Older HTTP+SSE transport for MCP.

| Property | Value |
|----------|-------|
| Flag | `--sse-port <PORT>` |
| SSE endpoint | `GET /sse` |
| Message endpoint | `POST /message` |
| Bind address | `0.0.0.0:{PORT}` |
| SSE keep-alive | 15 seconds |
| Conflicts with | `--http-port` |

Activated with `--sse-port`:

```
mcp-v8 --sse-port 8080
mcp-v8 --sse-port 8080 --stateless
```

The SSE server also serves all the same REST API, agent discovery, and CLI download endpoints listed above.

## Port Configuration

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--http-port` | `u16` | (none) | Enable Streamable HTTP transport |
| `--sse-port` | `u16` | (none) | Enable SSE transport |

The two flags conflict with each other. If neither is set, stdio transport is used.
