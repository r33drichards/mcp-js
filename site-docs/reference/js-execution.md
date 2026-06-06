# Running JavaScript & TypeScript

Complete reference for the `run_js` MCP tool (stateful and stateless) and the `POST /api/exec` REST endpoint.

## run_js — stateful mode

Available when the server is **not** started with `--stateless`. Dispatches execution asynchronously and returns an `execution_id` immediately.

### Parameters

| Parameter | Type | Required | Description |
|---|---|---|---|
| `code` | string | Yes | JavaScript or TypeScript source. TypeScript types are stripped by SWC before execution. JSX/TSX is not supported (rejected with a parse error). |
| `heap` | string | No | Content hash of a previously saved heap snapshot. Omit or pass empty string to start from a fresh isolate. |
| `heap_memory_max_mb` | integer | No | V8 heap cap in MB for this call. Effective minimum: 8. Overrides `--heap-memory-max`. |
| `execution_timeout_secs` | integer | No | Wall-clock timeout in seconds (1–300). Overrides `--execution-timeout`. |
| `tags` | object | No | Key-value string pairs attached to the output heap snapshot. Queryable via `query_heaps_by_tags`. |

### Return value

```json
{"execution_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6"}
```

The call returns immediately. Poll for the result with `get_execution`. Read console output with `get_execution_output`. See [Asynchronous execution & output](../reference/async-execution.md) for pagination details.

## run_js — stateless mode

Available when the server is started with `--stateless`. Polls completion internally and returns output synchronously from the caller's perspective.

### Parameters

| Parameter | Type | Required | Description |
|---|---|---|---|
| `code` | string | Yes | JavaScript or TypeScript source. TypeScript types are stripped before execution. JSX/TSX is not supported (rejected with a parse error). |
| `heap_memory_max_mb` | integer | No | V8 heap cap in MB. Effective minimum: 8. Overrides `--heap-memory-max`. |
| `execution_timeout_secs` | integer | No | Timeout in seconds (1–300). Overrides `--execution-timeout`. |

`heap` and `tags` are not accepted in stateless mode.

### Return value — success

```json
{"output": "hello, world!\n"}
```

`output` contains all captured console output for the execution.

### Return value — failure / timeout / cancellation

```json
{"output": "partial output here\n", "error": "Execution timed out: script exceeded the time limit."}
```

`output` contains any console lines captured before termination; `error` contains the error message.

## POST /api/exec — REST endpoint

Available on HTTP and SSE transports (`--http-port` or `--sse-port`). Always asynchronous.

### Request body

| Field | Type | Required | Description |
|---|---|---|---|
| `code` | string | Yes | JavaScript or TypeScript source. |
| `heap` | string | No | Input heap content hash (stateful mode). |
| `session` | string | No | Named session identifier. Appended to the session log for `list_session_snapshots`. |
| `tags` | object | No | Key-value tags for the output heap snapshot (stateful mode). |
| `heap_memory_max_mb` | integer | No | Per-call heap cap in MB (minimum 8). |
| `execution_timeout_secs` | integer | No | Per-call timeout in seconds (1–300). |

### Response — 202 Accepted

```json
{"execution_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6"}
```

## ExecutionInfo shape

Returned by `get_execution` (MCP) and `GET /api/executions/{id}` (REST).

| Field | Type | Description |
|---|---|---|
| `execution_id` | string | UUID assigned at dispatch time. |
| `status` | string | One of: `running`, `completed`, `failed`, `cancelled`, `timed_out`. |
| `result` | string or null | Always an empty string on completion. Console output is stored separately — use `get_execution_output`. |
| `heap` | string or null | Content hash of the output heap snapshot (stateful only; `null` in stateless mode and when an execution fails). |
| `error` | string or null | Error message when `status` is `failed`, `timed_out`, or `cancelled`. `null` on success. |
| `started_at` | string | RFC 3339 timestamp of when the execution was registered. |
| `completed_at` | string or null | RFC 3339 timestamp of when the execution reached a terminal status. `null` while `running`. |

## Execution statuses

| Status | Description |
|---|---|
| `running` | Execution is active in a V8 isolate. |
| `completed` | Script finished normally; `heap` is populated in stateful mode. |
| `failed` | Script threw an uncaught error, transpilation failed, or the heap limit was exceeded. |
| `cancelled` | Stopped via `cancel_execution` before completion; V8 isolate was terminated. |
| `timed_out` | Wall-clock timeout (`execution_timeout_secs`) expired; V8 isolate was terminated. |

## Console methods

All six methods route through the internal capture op. Arguments are formatted using `JSON.stringify` for non-string values; multiple arguments are joined with a single space. A newline (`\n`) is appended to every call.

| Method | Prefix prepended to each line |
|---|---|
| `console.log(...)` | _(none)_ |
| `console.debug(...)` | _(none)_ |
| `console.trace(...)` | _(none)_ |
| `console.info(...)` | `[INFO] ` |
| `console.warn(...)` | `[WARN] ` |
| `console.error(...)` | `[ERROR] ` |

## Timer functions

| Function | Signature | Notes |
|---|---|---|
| `setTimeout` | `setTimeout(callback: Function, delayMs?: number): number` | Returns a timer ID (integer ≥ 1). Delay is clamped to 0 ms minimum. |
| `clearTimeout` | `clearTimeout(id: number): void` | Cancels a pending timer. No-op if the ID is unknown or already fired. |

`setInterval` is not available.

## TypeScript transpilation

Input is always parsed with the SWC TypeScript parser (`TsSyntax { tsx: false }`). The transformation strips: type annotations, interface and type alias declarations, enum declarations (replaced with JavaScript equivalents), and type-only imports. The output is plain JavaScript executed as an ES module.

Because JSX is disabled (`tsx: false`), angle-bracket type assertions (`<T>value`) are unambiguous and permitted alongside the `as` form. JSX/TSX source is not supported and is rejected with a parse error.

Transpilation errors are reported as `failed` executions with a message of the form:

```
TypeScript parse error: <SWC diagnostic>
```

## Defaults and limits

| Setting | Default | Server flag to change | Per-call override parameter |
|---|---|---|---|
| Heap memory cap | 8 MB | `--heap-memory-max` | `heap_memory_max_mb` (minimum 8 MB) |
| Execution timeout | 30 s | `--execution-timeout` | `execution_timeout_secs` (1–300 s) |
| Max concurrent executions | CPU count | `--max-concurrent-executions` | — |

## Error messages

| Error text | Cause |
|---|---|
| `Out of memory: V8 heap limit exceeded. Try increasing heap_memory_max_mb.` | The V8 heap or array buffer allocator reached the configured cap. |
| `Execution timed out: script exceeded the time limit. Try increasing execution_timeout_secs.` | The wall-clock timeout expired and the isolate was terminated. |
| `TypeScript parse error: ...` | SWC failed to parse the input as TypeScript (this includes JSX/TSX, which is not supported). |
| `Cancelled by user` | The `cancel_execution` tool or REST cancel endpoint was called. |

## See also

- [Tutorial — run your first script](../tutorials/js-execution.md)
- [How-to — execution recipes](../how-to/js-execution.md)
- [Concepts — execution model](../concepts/js-execution.md)
- [Asynchronous execution & output](../reference/async-execution.md)
- [Stateful sessions & heap snapshots](../reference/sessions-and-heaps.md)
- [MCP tools reference](../reference/mcp-tools.md)
