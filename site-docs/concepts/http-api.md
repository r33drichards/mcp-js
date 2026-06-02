# HTTP REST API Design

When mcp-v8 runs with an HTTP-based transport (`--http-port` or `--sse-port`), it exposes a REST API alongside the MCP protocol. This API provides the same execution capabilities as the MCP tools but through standard HTTP endpoints, making mcp-v8 accessible to any HTTP client without requiring MCP protocol support.

## Endpoints

The REST API is served under the `/api` prefix:

### Execution

- `POST /api/exec` -- Submit code for execution. Accepts a JSON body with `code`, optional `heap`, `session`, `heap_memory_max_mb`, `execution_timeout_secs`, and `tags`. Returns `202 Accepted` with an `execution_id`.

- `GET /api/executions/:id` -- Get the status of an execution. Returns the execution status, result (if completed), error (if failed), heap snapshot hash (if stateful), and timestamps.

- `GET /api/executions/:id/output` -- Get paginated console output. Supports query parameters: `line_offset`, `line_limit`, `byte_offset`, `byte_limit`. Returns a page of console output with dual coordinate system.

- `POST /api/executions/:id/cancel` -- Cancel a running execution. Terminates the V8 isolate.

- `GET /api/executions` -- List all known executions with their status summaries.

### Sessions (stateful mode only)

- `GET /api/sessions` -- List all session names.
- `GET /api/sessions/:name` -- List all entries in a session log.

### Heap Tags (stateful mode only)

- `GET /api/heaps/:id/tags` -- Get tags for a heap snapshot.
- `PUT /api/heaps/:id/tags` -- Set tags for a heap snapshot.
- `DELETE /api/heaps/:id/tags` -- Delete tags (all or specific keys).
- `POST /api/heaps/query` -- Query heaps by tag filter.

### Documentation

- `GET /llms.txt` -- Machine-readable agent guide.
- `GET /docs` -- Full README.
- `GET /api-doc/openapi.json` -- OpenAPI specification.

### CLI Distribution

- `GET /api/cli` -- Index of available CLI binaries for the current server version.
- `GET /api/cli/:platform` -- Download a platform-specific CLI binary.

## OpenAPI Specification

The REST API is fully described by an OpenAPI 3.0 specification, generated at compile time using the `utoipa` crate. The spec documents all endpoints, request/response schemas, and query parameters.

The spec is available at `/api-doc/openapi.json` when the server is running with an HTTP port. It can also be printed to stdout at startup using `--print-openapi` for offline use or CI validation.

## Request and Response Schemas

### ExecRequest (POST /api/exec)

```json
{
  "code": "console.log('hello')",
  "heap": "abc123...",
  "session": "my-session",
  "heap_memory_max_mb": 16,
  "execution_timeout_secs": 60,
  "tags": { "env": "production" }
}
```

Only `code` is required. All other fields are optional and override server defaults.

### ExecutionInfo (GET /api/executions/:id)

```json
{
  "execution_id": "uuid-here",
  "status": "completed",
  "result": "",
  "heap": "sha256-hash",
  "error": null,
  "started_at": "2024-01-15T10:30:00Z",
  "completed_at": "2024-01-15T10:30:01Z"
}
```

### ExecutionOutput (GET /api/executions/:id/output)

```json
{
  "execution_id": "uuid-here",
  "data": "hello\nworld\n",
  "start_line": 1,
  "end_line": 2,
  "next_line_offset": 3,
  "total_lines": 2,
  "start_byte": 0,
  "end_byte": 12,
  "next_byte_offset": 12,
  "total_bytes": 12,
  "has_more": false,
  "status": "completed"
}
```

## Relationship to MCP Tools

The REST API and MCP tools provide equivalent functionality through different interfaces:

| MCP Tool | REST Endpoint |
|---|---|
| `run_js` | `POST /api/exec` |
| `get_execution_status` | `GET /api/executions/:id` |
| `get_execution_output` | `GET /api/executions/:id/output` |
| `cancel_execution` | `POST /api/executions/:id/cancel` |
| `list_sessions` | `GET /api/sessions` |
| `list_session_snapshots` | `GET /api/sessions/:name` |

Both interfaces use the same underlying Engine. An execution started via MCP can be queried via REST, and vice versa. The execution registry is shared.

The REST API exists for several reasons:

- **Non-MCP clients** -- Not all tools and scripts speak MCP. The REST API makes mcp-v8 accessible to curl, Postman, CI scripts, and any HTTP-capable language.
- **Monitoring** -- The `/api/executions` endpoint enables external monitoring of execution status.
- **CLI client** -- The mcp-v8 CLI client is auto-generated from the OpenAPI spec and talks to the REST API.
- **Swagger UI compatibility** -- The OpenAPI spec enables auto-generated API explorers.
