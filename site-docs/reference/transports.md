# Transports: stdio, HTTP, SSE

Complete reference for the three transport modes, their flags, endpoints, and constraints.

## Transport summary

| Transport | CLI flag | Default | Bind address | MCP endpoint(s) | Extra endpoints | SSE keep-alive |
|---|---|---|---|---|---|---|
| stdio | (none) | Yes | — | stdin / stdout | — | — |
| Streamable HTTP | `--http-port=N` | No | `0.0.0.0:N` | `POST /mcp` | `GET /api-doc/openapi.json`, `GET/POST /api/*` | 15 s |
| SSE | `--sse-port=N` | No | `0.0.0.0:N` | `GET /sse` (stream), `POST /message` (client→server) | `GET /api-doc/openapi.json`, `GET/POST /api/*` | 15 s |

## Flag details

### `--http-port`

- **Type:** `u16` (1–65535)
- **Default:** none (transport not started)
- **Conflicts with:** `--sse-port`
- **Effect:** Starts the Streamable HTTP server. The MCP endpoint is `POST /mcp`. The same port also serves the REST sidecar (`/api/*`) and `GET /api-doc/openapi.json`.

### `--sse-port`

- **Type:** `u16` (1–65535)
- **Default:** none (transport not started)
- **Conflicts with:** `--http-port`
- **Effect:** Starts the SSE server. Registers `GET /sse` (event stream) and `POST /message` (client messages). The same port also serves the REST sidecar (`/api/*`) and `GET /api-doc/openapi.json`.

### `--print-openapi`

- **Type:** boolean flag
- **Default:** false
- **Effect:** Prints the OpenAPI 3 JSON spec for the REST sidecar to stdout and exits immediately. No server is started and no port is opened. Useful for generating the spec offline.

## Mutual exclusivity

`--http-port` and `--sse-port` are declared with `conflicts_with` in the CLI parser. Passing both causes an immediate startup error.

## Cluster mode constraint

If `--cluster-port` is set, at least one of `--http-port` or `--sse-port` must also be set. Attempting to run a cluster node over stdio produces the error:

```
Cluster mode requires --http-port or --sse-port (stdio transport is not supported in cluster mode)
```

## Endpoint reference

### `POST /mcp` (Streamable HTTP only)

MCP JSON-RPC endpoint. Accepts `Content-Type: application/json`. Supports both single-response and SSE-streamed responses as defined by the MCP Streamable HTTP specification.

### `GET /sse` (SSE only)

Opens a long-lived SSE connection. The server sends keep-alive pings every 15 seconds.

### `POST /message` (SSE only)

Accepts MCP JSON-RPC messages from the client. Must be used in conjunction with an active `GET /sse` connection.

### `GET /api-doc/openapi.json` (HTTP and SSE transports)

Returns the OpenAPI 3 JSON specification for all `/api/*` REST endpoints. `Content-Type: application/json`.

### `/api/*` (HTTP and SSE transports)

REST sidecar. See the [HTTP API reference](../reference/http-api.md) for the full endpoint list.

## See also

- [How-to guide](../how-to/transports.md)
- [Concepts](../concepts/transports.md)
- [Reference: async execution](../reference/async-execution.md)
- [Reference: authentication](../reference/authentication.md)
- [Reference: clustering](../reference/clustering.md)
- [CLI flags reference](../reference/cli-flags.md)
