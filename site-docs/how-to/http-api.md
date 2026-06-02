# How to Use the HTTP REST API

Interact with mcp-v8 via its REST API when running with `--http-port` or `--sse-port`.

## Start the server

```bash
mcp-v8 --http-port 3000
```

## Submit code for execution

```bash
curl -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{"code": "console.log(1 + 1);"}'
```

Response:

```json
{ "execution_id": "abc-123" }
```

Optional fields in the request body:

- `heap` -- SHA-256 hash to resume from
- `session` -- session name for logging
- `heap_memory_max_mb` -- per-execution memory cap
- `execution_timeout_secs` -- per-execution timeout
- `tags` -- key/value metadata for the heap snapshot

## Poll execution status

```bash
curl http://localhost:3000/api/executions/abc-123
```

Response:

```json
{
  "execution_id": "abc-123",
  "status": "completed",
  "result": null,
  "heap": "sha256...",
  "error": null,
  "started_at": "2025-01-01T00:00:00Z",
  "completed_at": "2025-01-01T00:00:01Z"
}
```

Status values: `running`, `completed`, `failed`, `cancelled`, `timed_out`.

## Read console output

```bash
curl "http://localhost:3000/api/executions/abc-123/output?line_offset=0&line_limit=100"
```

Query parameters:

| Parameter | Description |
|-----------|-------------|
| `line_offset` | Start from this line (0-indexed) |
| `line_limit` | Max lines to return |
| `byte_offset` | Start from this byte offset |
| `byte_limit` | Max bytes to return |

## List all executions

```bash
curl http://localhost:3000/api/executions
```

## Cancel an execution

```bash
curl -X POST http://localhost:3000/api/executions/abc-123/cancel
```

## API documentation endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /swagger-ui` | Interactive Swagger UI |
| `GET /api-doc/openapi.json` | OpenAPI 3.0 spec |
| `GET /llms.txt` | Machine-readable agent guide |
| `GET /docs` | Full README documentation |

## Generate the OpenAPI spec offline

```bash
mcp-v8 --print-openapi > openapi.json
```
