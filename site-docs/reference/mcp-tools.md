# MCP Tools Reference

## Stateful Mode Tools

Available when not running with `--stateless`.

### run_js

Execute JavaScript or TypeScript code. Returns an execution ID immediately.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `code` | `string` | Yes | -- | JavaScript or TypeScript source code |
| `heap` | `string?` | No | `null` | Heap snapshot hash to restore before execution |
| `heap_memory_max_mb` | `integer?` | No | Server default (8) | V8 heap memory cap in megabytes (minimum 8) |
| `execution_timeout_secs` | `integer?` | No | Server default (30) | Execution timeout in seconds (1-300) |
| `tags` | `object?` | No | `null` | Key-value string pairs to tag the resulting heap snapshot |

**Response:**

```json
{
  "execution_id": "a1b2c3d4-e5f6-4789-abcd-ef0123456789"
}
```

### get_execution

Get the status and result of an execution.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `execution_id` | `string` | Yes | UUID returned by `run_js` |

**Response:**

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

### get_execution_output

Get paginated console output for an execution.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `execution_id` | `string` | Yes | -- | Execution UUID |
| `line_offset` | `integer?` | No | `1` | Start line (1-indexed) |
| `line_limit` | `integer?` | No | `100` | Max lines to return |
| `byte_offset` | `integer?` | No | (none) | Start byte offset (0-indexed). If set, byte mode takes precedence. |
| `byte_limit` | `integer?` | No | `4096` | Max bytes to return |

**Response:** See [Console Output Reference](console-output.md).

### cancel_execution

Cancel a running execution. Terminates the V8 isolate.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `execution_id` | `string` | Yes | Execution UUID |

**Response:**

```json
{
  "ok": true
}
```

Error response (execution not running):

```json
{
  "ok": false,
  "error": "Execution '...' is not running (status: completed)"
}
```

### list_executions

List all executions with their status. No parameters.

**Response:**

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

### list_sessions

List all named sessions. No parameters.

**Response:**

```json
{
  "sessions": ["session-1", "session-2"]
}
```

### list_session_snapshots

List all log entries for the current session.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `fields` | `string?` | No | Comma-separated field names to include: `index`, `input_heap`, `output_heap`, `code`, `timestamp`. If omitted, all fields are returned. |

**Response:**

```json
{
  "entries": [
    {
      "index": 0,
      "input_heap": null,
      "output_heap": "a3f2b8c1...",
      "code": "console.log('hello')",
      "timestamp": "2024-01-01T00:00:00Z"
    }
  ]
}
```

### get_heap_tags

See [Heap Tags Reference](heap-tags.md).

### set_heap_tags

See [Heap Tags Reference](heap-tags.md).

### delete_heap_tags

See [Heap Tags Reference](heap-tags.md).

### query_heaps_by_tags

See [Heap Tags Reference](heap-tags.md).

## Stateless Mode Tools

Available when running with `--stateless`.

### run_js (Stateless)

Execute JavaScript or TypeScript code. Blocks until completion and returns console output directly.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `code` | `string` | Yes | -- | JavaScript or TypeScript source code |
| `heap_memory_max_mb` | `integer?` | No | Server default (8) | V8 heap memory cap in megabytes |
| `execution_timeout_secs` | `integer?` | No | Server default (30) | Execution timeout in seconds |

**Response (success):**

```json
{
  "output": "hello world\n"
}
```

**Response (failure):**

```json
{
  "output": "partial output...\n",
  "error": "ReferenceError: x is not defined"
}
```

The stateless `run_js` polls internally at 50ms intervals for up to 5 minutes.

## MCP Resources

Both stateful and stateless modes expose these documentation resources via `resources/list` and `resources/read`:

| URI | Name | MIME Type | Description |
|-----|------|-----------|-------------|
| `docs://readme` | README | `text/markdown` | Full mcp-v8 README |
| `docs://llms-txt` | llms.txt | `text/markdown` | Machine-readable agent guide |
| `docs://openapi` | OpenAPI spec | `application/json` | OpenAPI 3.0 JSON spec for the REST API |
| `docs://tools` | MCP tool list | `application/json` | JSON list of available MCP tools with descriptions |
