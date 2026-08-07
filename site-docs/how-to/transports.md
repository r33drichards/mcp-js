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
        --heap-store=dir \
        --heap-dir=/var/lib/mcp-v8/heaps
```

`--http-port` and `--sse-port` are mutually exclusive. Passing both is an error.

## Take the port from `PORT`

Hosted platforms (Railway, Render, Heroku, Fly, Cloud Run, …) assign a port at
runtime and inject it as `$PORT` rather than a project-specific variable, then
route their health checks to it. When no port is configured any other way,
`mcp-v8` folds `$PORT` into `--http-port`:

```bash
PORT=52341 mcp-v8   # equivalent to: mcp-v8 --http-port=52341
```

This makes the published Docker image deployable on those platforms with no
start-command or argument overrides.

`$PORT` is the lowest-precedence way to choose a port. Highest first:

1. An explicit `--http-port`/`--sse-port` argument.
2. `MCP_V8_HTTP_PORT` / `MCP_V8_SSE_PORT`.
3. `http_port` / `sse_port` in a [config file](../reference/config-file.md).
4. `$PORT` — always selects Streamable HTTP, never the legacy SSE transport.
5. Nothing set — the stdio transport.

An empty or whitespace-only `$PORT` counts as unset, so `PORT=` returns the
process to stdio. A value that is not a number in `0..=65535` is rejected at
startup rather than silently ignored. Because `$PORT` selects a transport and
not merely a number, [`--metadata-only`](clustering.md#run-a-metadata-only-node)
nodes — which serve no MCP transport — ignore it entirely.

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
- [How-to: async execution](../how-to/async-execution.md)
- [How-to: authentication](../how-to/authentication.md)
- [How-to: clustering](../how-to/clustering.md)
- [CLI flags reference](../reference/cli-flags.md)
