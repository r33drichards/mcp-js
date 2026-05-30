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
