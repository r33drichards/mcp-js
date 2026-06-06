# Asynchronous execution & output

Complete parameter and response reference for the async execution MCP tools, the status enum, and the output pagination model.

## MCP tools (stateful mode)

The following tools are available when the server is running in stateful mode (default). They are not present in stateless mode.

---

### `run_js`

Submit JavaScript or TypeScript code for asynchronous execution. Returns immediately.

**Parameters**

| Parameter | Type | Required | Description |
|---|---|---|---|
| `code` | string | yes | JavaScript or TypeScript source to execute. TypeScript is transpiled via swc before execution. |
| `heap` | string | no | Heap snapshot key to restore before execution. |
| `heap_memory_max_mb` | integer | no | Per-execution V8 heap memory cap in megabytes. Overrides the server default (`--heap-memory-max`). |
| `execution_timeout_secs` | integer | no | Per-execution wall-clock timeout in seconds (1–300). Overrides the server default (`--execution-timeout`). |
| `tags` | object `{string: string}` | no | Arbitrary key/value tags attached to the resulting heap snapshot. |

**Response**

```json
{ "execution_id": "<string>" }
```

`execution_id` is a unique opaque string. Pass it to `get_execution`, `get_execution_output`, or `cancel_execution`.

---

### `get_execution`

Get the status and result of an execution.

**Parameters**

| Parameter | Type | Required | Description |
|---|---|---|---|
| `execution_id` | string | yes | ID returned by `run_js`. |

**Response**

| Field | Type | Description |
|---|---|---|
| `execution_id` | string | The queried ID. |
| `status` | string | One of the values in the [status enum](#execution-status-enum) below. |
| `result` | string \| null | Serialised return value when `status` is `completed`; otherwise `null`. |
| `heap` | string \| null | Heap snapshot key produced after execution (stateful mode only). |
| `error` | string \| null | Error message when `status` is `failed`, `timed_out`, or `cancelled`; otherwise `null`. |
| `started_at` | string | ISO-8601 timestamp when execution began. |
| `completed_at` | string \| null | ISO-8601 timestamp when execution reached a terminal state; `null` while running. |

---

### `get_execution_output`

Get a paginated page of console output (`console.log` / `console.error` etc.) for an execution. Can be called while the execution is still running.

**Parameters**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `execution_id` | string | yes | — | ID returned by `run_js`. |
| `line_offset` | integer | no | `1` | First line to return (1-based; `1` = first line). Ignored when `byte_offset` is provided. |
| `line_limit` | integer | no | `100` | Maximum number of lines to return. Ignored when `byte_offset` is provided. |
| `byte_offset` | integer | no | — | Return output starting at this byte offset. When present, byte mode takes precedence over line-based parameters. |
| `byte_limit` | integer | no | `4096` | Maximum number of bytes to return. Only used when `byte_offset` is provided. |

**Response**

| Field | Type | Description |
|---|---|---|
| `execution_id` | string | The queried ID. |
| `data` | string | Text content for the requested window. |
| `start_line` | integer | First line number in this page (1-based). |
| `end_line` | integer | Last line number in this page (1-based). |
| `next_line_offset` | integer | Pass as `line_offset` to fetch the next page in line mode. |
| `total_lines` | integer | Total lines written so far. |
| `start_byte` | integer | First byte offset in this page. |
| `end_byte` | integer | Last byte offset in this page (exclusive). |
| `next_byte_offset` | integer | Pass as `byte_offset` to fetch the next page in byte mode. |
| `total_bytes` | integer | Total bytes written so far. |
| `has_more` | boolean | `true` if more output exists beyond this page. |
| `status` | string | Execution status at the time of this query. |

Both sets of cursor fields (`start_line`/`end_line`/`next_line_offset` and `start_byte`/`end_byte`/`next_byte_offset`) are always present regardless of which pagination mode was used.

---

### `cancel_execution`

Cancel a running execution. Terminates the V8 isolate immediately.

**Parameters**

| Parameter | Type | Required | Description |
|---|---|---|---|
| `execution_id` | string | yes | ID of the execution to cancel. Must have status `running`. |

**Response**

| Field | Type | Description |
|---|---|---|
| `ok` | boolean | `true` on success. |
| `error` | string | Error message when `ok` is `false` (e.g. execution is not running). Absent when `ok` is `true`. |

---

### `list_executions`

List all executions the server currently tracks (both running and recently completed).

**Parameters:** none.

**Response**

```json
{
  "executions": [
    {
      "execution_id": "<string>",
      "status": "<string>",
      "started_at": "<ISO-8601>",
      "completed_at": "<ISO-8601 | null>"
    }
  ]
}
```

---

## MCP tool (stateless mode only)

In stateless mode (`--stateless`) only `run_js` is available. It polls internally until the execution finishes and returns the full output directly.

### `run_js` (stateless)

**Parameters**

| Parameter | Type | Required | Description |
|---|---|---|---|
| `code` | string | yes | JavaScript or TypeScript source to execute. |
| `heap_memory_max_mb` | integer | no | Per-execution V8 heap memory cap in megabytes. |
| `execution_timeout_secs` | integer | no | Per-execution wall-clock timeout in seconds. |

`heap`, `tags`, and session association are not available in stateless mode.

**Response (success)**

```json
{ "output": "<all console output as a string>" }
```

**Response (failure / timeout / cancellation)**

```json
{ "output": "<console output written before failure>", "error": "<message>" }
```

The stateless `run_js` polls every 50 ms for up to 6000 iterations (300 seconds). If the execution does not complete within that window, `error` is `"Execution did not complete within polling timeout"`.

---

## Execution status enum

| Value | Description |
|---|---|
| `running` | The V8 isolate is active. |
| `completed` | The script returned normally. |
| `failed` | The script threw an unhandled exception or a runtime error occurred. |
| `timed_out` | The execution wall-clock timeout was exceeded. |
| `cancelled` | `cancel_execution` was called while the execution was running. |

Cancellation via `cancel_execution` (or `POST /api/executions/{id}/cancel`) calls `IsolateHandle::terminate_execution()` on the V8 isolate and sets the status atomically. Cancellation fails (error returned, status unchanged) if the execution is in any terminal state.

---

## REST API relationship

The MCP tools map directly to REST endpoints available on HTTP and SSE transports. Refer to [Reference: HTTP API](http-api.md) for the full OpenAPI specification.

| MCP tool | REST equivalent |
|---|---|
| `run_js` | `POST /api/exec` |
| `get_execution` | `GET /api/executions/{id}` |
| `get_execution_output` | `GET /api/executions/{id}/output` |
| `cancel_execution` | `POST /api/executions/{id}/cancel` |
| `list_executions` | `GET /api/executions` |

The REST `POST /api/exec` body (`ExecRequest`) also accepts a `session` string field not present in the MCP `run_js` parameters; the MCP layer derives session from the `X-MCP-Session-Id` header set during `initialize` instead.

---

## See also

- [How-to: Asynchronous execution & output](../how-to/async-execution.md)
- [Concepts: Asynchronous execution & output](../concepts/async-execution.md)
- [Reference: Running JavaScript & TypeScript](js-execution.md)
- [Reference: HTTP API](http-api.md)
- [Concepts: Transports](../concepts/transports.md)
