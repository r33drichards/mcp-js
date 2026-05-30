# HTTP API

When `mcp-v8` runs with `--http-port` or `--sse-port`, it exposes a plain HTTP
API alongside MCP transport.

Core endpoints:

- `POST /api/exec`
- `GET /api/executions`
- `GET /api/executions/{id}`
- `GET /api/executions/{id}/output`
- `POST /api/executions/{id}/cancel`
- `GET /api/version`
- `GET /api/cli`
- `GET /api/cli/{platform}`
- `GET /api-doc/openapi.json`

Use the OpenAPI document as the source of truth for request and response
shapes.

The HTTP API is a fallback and tooling surface, not the primary product
integration model. The main integration story for `mcp-v8` is still the MCP
server and tool surface.
