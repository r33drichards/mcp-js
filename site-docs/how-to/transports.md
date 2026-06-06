# Transports: stdio, HTTP, SSE

Recipes for starting mcp-v8 on each of its three transports and for locating the bundled OpenAPI spec.

## Configure stdio in an MCP client

stdio is the default. No port flag is needed. Your MCP client spawns the process and communicates over stdin/stdout.

Minimal configuration (Claude Desktop format):

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": []
    }
  }
}
```

Pass additional flags via `args`. For example, to set a custom session database path:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": ["--session-db-path", "/var/lib/mcp-v8/sessions"]
    }
  }
}
```

Tracing output goes to stderr; the MCP client typically does not display it.

## Expose Streamable HTTP

Use `--http-port` to serve MCP over HTTP. The flag accepts any available port number.

```bash
mcp-v8 --http-port=8080
```

The server binds on `0.0.0.0:8080`. The MCP endpoint is `POST /mcp`. The REST sidecar endpoints (`/api/*`) and the OpenAPI spec (`/api-doc/openapi.json`) are also available on the same port.

To restrict the heap size and set a custom storage directory:

```bash
mcp-v8 --http-port=8080 \
        --heap-memory-max=32 \
        --directory-path=/var/lib/mcp-v8/heaps
```

`--http-port` and `--sse-port` are mutually exclusive. Passing both is an error.

## Expose SSE

Use `--sse-port` to serve MCP over the older SSE transport. This is compatible with MCP clients that do not support Streamable HTTP.

```bash
mcp-v8 --sse-port=8081
```

The server binds on `0.0.0.0:8081`. Two endpoints are registered:

- `GET /sse` — the long-lived SSE stream the client subscribes to.
- `POST /message` — the endpoint the client posts MCP messages to.

The REST sidecar (`/api/*`) and `GET /api-doc/openapi.json` are also available on the same port.

`--sse-port` and `--http-port` are mutually exclusive.

## Find the OpenAPI doc endpoint

When the server is running with `--http-port` or `--sse-port`, the OpenAPI 3 spec for the REST sidecar is served at:

```
GET /api-doc/openapi.json
```

Example:

```bash
curl -s http://localhost:8080/api-doc/openapi.json | python3 -m json.tool | head -30
```

### Print the spec without starting the server

To generate the spec offline (for import into an API tool):

```bash
mcp-v8 --print-openapi > openapi.json
```

The process prints the spec to stdout and exits immediately — no port is opened.

## See also

- [Concepts](../concepts/transports.md)
- [Reference](../reference/transports.md)
- [How-to: async execution](../how-to/async-execution.md)
- [How-to: authentication](../how-to/authentication.md)
- [How-to: clustering](../how-to/clustering.md)
- [CLI flags reference](../reference/cli-flags.md)
