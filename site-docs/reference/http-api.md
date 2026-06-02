# HTTP API Reference

The REST API is available on all HTTP transports (`--http-port` and `--sse-port`).

## Endpoints

### GET /api/version

Return the running server version.

**Response (200):**

```json
{
  "version": "0.1.0"
}
```

### POST /api/exec

Submit JavaScript code for asynchronous execution.

**Request body (`ExecRequest`):**

```json
{
  "code": "console.log('hello')",
  "heap": "a3f2b8c1...",
  "session": "my-session",
  "heap_memory_max_mb": 16,
  "execution_timeout_secs": 60,
  "tags": {"env": "prod"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `code` | `string` | Yes | JavaScript or TypeScript source code |
| `heap` | `string?` | No | Heap snapshot key to restore |
| `session` | `string?` | No | Session identifier for tagging/logging |
| `heap_memory_max_mb` | `integer?` | No | Per-execution V8 heap memory cap (MB) |
| `execution_timeout_secs` | `integer?` | No | Per-execution timeout (seconds) |
| `tags` | `object?` | No | Key-value string tags for the resulting heap |

**Response (202 Accepted):**

```json
{
  "execution_id": "a1b2c3d4-e5f6-4789-abcd-ef0123456789"
}
```

**Response (500 Internal Error):**

```json
{
  "error": "error message"
}
```

### GET /api/executions

List all known executions.

**Response (200):**

```json
{
  "executions": [
    {
      "execution_id": "...",
      "status": "completed",
      "started_at": "2024-01-01T00:00:00Z",
      "completed_at": "2024-01-01T00:00:01Z"
    }
  ]
}
```

### GET /api/executions/{id}

Get the status and result of an execution.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `string` | Execution ID |

**Response (200 - `ExecutionInfo`):**

```json
{
  "execution_id": "...",
  "status": "completed",
  "result": "",
  "heap": "a3f2b8c1...",
  "error": null,
  "started_at": "2024-01-01T00:00:00Z",
  "completed_at": "2024-01-01T00:00:01Z"
}
```

**Response (404):**

```json
{
  "error": "Execution '...' not found"
}
```

### GET /api/executions/{id}/output

Read paginated console output from an execution.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `string` | Execution ID |

**Query parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `line_offset` | `integer?` | `1` | Start line (1-indexed) |
| `line_limit` | `integer?` | `100` | Max lines |
| `byte_offset` | `integer?` | (none) | Start byte (0-indexed). Takes precedence. |
| `byte_limit` | `integer?` | `4096` | Max bytes |

**Response (200 - `ExecutionOutput`):**

```json
{
  "execution_id": "...",
  "data": "hello world\n",
  "start_line": 1,
  "end_line": 1,
  "next_line_offset": 2,
  "total_lines": 1,
  "start_byte": 0,
  "end_byte": 12,
  "next_byte_offset": 12,
  "total_bytes": 12,
  "has_more": false,
  "status": "completed"
}
```

**Response (404):**

```json
{
  "error": "..."
}
```

### POST /api/executions/{id}/cancel

Cancel a running execution.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `string` | Execution ID |

**Response (200):**

```json
{
  "ok": true
}
```

**Response (400):**

```json
{
  "ok": false,
  "error": "Execution '...' is not running (status: completed)"
}
```

### GET /api/cli

List available CLI binary downloads.

**Response (200 - `CliIndex`):**

```json
{
  "version": "0.1.0",
  "assets": [
    {
      "platform": "linux-x86_64",
      "url": "http://host/api/cli/linux-x86_64",
      "available": true
    },
    {
      "platform": "linux-aarch64",
      "url": "http://host/api/cli/linux-aarch64",
      "available": false
    },
    {
      "platform": "macos-aarch64",
      "url": "http://host/api/cli/macos-aarch64",
      "available": false
    }
  ]
}
```

### GET /api/cli/{platform}

Download CLI binary for a platform.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `platform` | `string` | `linux-x86_64`, `linux-aarch64`, or `macos-aarch64` |

**Response (200):** Binary data (`application/octet-stream`)

**Response (404):** Platform not found or binary not embedded

## Agent Discovery Endpoints

| Path | Method | Content-Type | Description |
|------|--------|-------------|-------------|
| `/` | GET | -- | 301 redirect to `/llms.txt` |
| `/llms.txt` | GET | `text/markdown` | Machine-readable agent guide |
| `/docs` | GET | `text/markdown` | Full README |
| `/api-doc/openapi.json` | GET | `application/json` | OpenAPI 3.0 specification |

## Response Schemas Summary

| Schema | Used By |
|--------|---------|
| `ExecRequest` | POST /api/exec request body |
| `ExecAccepted` | POST /api/exec response |
| `ExecutionInfo` | GET /api/executions/{id} response |
| `ExecutionOutput` | GET /api/executions/{id}/output response |
| `ExecutionList` | GET /api/executions response |
| `ExecutionSummary` | Items in ExecutionList |
| `CancelResult` | POST /api/executions/{id}/cancel response |
| `ApiError` | Error responses |
| `CliIndex` | GET /api/cli response |
| `CliAsset` | Items in CliIndex |
