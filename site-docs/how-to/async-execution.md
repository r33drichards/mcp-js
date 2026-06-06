# Asynchronous execution & output

Recipes for submitting code, polling results, streaming output, cancelling runs, and using the synchronous shortcut — all through MCP tools or the REST API.

## Submit and poll via MCP tools

In stateful mode, `run_js` returns an `execution_id`. Poll with `get_execution` until the status leaves `running`.

```json
// 1. Submit
{ "tool": "run_js", "arguments": { "code": "console.log('hello');" } }
// Response: { "execution_id": "01J9W…" }

// 2. Poll
{ "tool": "get_execution", "arguments": { "execution_id": "01J9W…" } }
// Response: { "execution_id": "01J9W…", "status": "completed", "result": null,
//             "heap": "sha256:…", "error": null,
//             "started_at": "…", "completed_at": "…" }
```

Poll until `status` is one of `completed`, `failed`, `cancelled`, or `timed_out`.

## Read console output by line

`get_execution_output` returns paginated `console.log` output. Omitting pagination parameters returns lines 1–100 (default `line_offset = 1`, `line_limit = 100`).

```json
{
  "tool": "get_execution_output",
  "arguments": { "execution_id": "01J9W…" }
}
```

To read the next page, pass the `next_line_offset` value from the previous response as `line_offset`:

```json
{
  "tool": "get_execution_output",
  "arguments": {
    "execution_id": "01J9W…",
    "line_offset": 101,
    "line_limit": 100
  }
}
```

Repeat while `has_more` is `true`.

## Read console output by byte

Byte-based mode is useful when output contains multi-line structured text or when you need exact byte positions. Providing `byte_offset` activates byte mode (overrides line-based parameters). The default `byte_limit` is 4096 bytes.

```json
{
  "tool": "get_execution_output",
  "arguments": {
    "execution_id": "01J9W…",
    "byte_offset": 0,
    "byte_limit": 8192
  }
}
```

Use `next_byte_offset` from the response as `byte_offset` for the next page.

## Read output while execution is still running

Both `get_execution_output` and `GET /api/executions/{id}/output` work while the execution is `running`. The `status` field in the response reflects the execution state at the time of the query, and `total_lines` / `total_bytes` reflect what has been written so far. Call again with `next_line_offset` or `next_byte_offset` to pick up new output.

## Cancel a running execution

Use `cancel_execution` to terminate the V8 isolate immediately. Cancellation only succeeds when the execution is `running`; calling it on an already-finished execution returns an error.

```json
{ "tool": "cancel_execution", "arguments": { "execution_id": "01J9W…" } }
// Success: { "ok": true }
// Already finished: { "ok": false, "error": "Execution '01J9W…' is not running (status: completed)" }
```

Via REST:

```bash
curl -s -X POST http://localhost:8080/api/executions/$EID/cancel
```

`200 { "ok": true }` on success; `400 { "ok": false, "error": "…" }` when the execution is not in `running` state.

## List all executions

`list_executions` returns a summary of every execution the server currently tracks (both in-flight and recently finished):

```json
{ "tool": "list_executions", "arguments": {} }
// Response: { "executions": [ { "execution_id": "…", "status": "…",
//                                "started_at": "…", "completed_at": "…" }, … ] }
```

Via REST:

```bash
curl -s http://localhost:8080/api/executions
```

## Use the stateless synchronous shortcut

When the server is running in stateless mode (`--stateless`), `run_js` is the only available tool and it blocks until the execution completes (polling internally every 50 ms, up to 300 seconds). It returns the full console output directly — no separate `get_execution_output` call is needed.

```json
{ "tool": "run_js", "arguments": { "code": "console.log(1 + 1);" } }
// Success: { "output": "2" }
// Failure: { "output": "…", "error": "ReferenceError: x is not defined" }
```

The stateless `run_js` accepts only `code`, `heap_memory_max_mb`, and `execution_timeout_secs`. The `heap`, `tags`, and `session` parameters are not available in stateless mode.

## Pass execution options

Both MCP `run_js` and REST `POST /api/exec` accept optional per-execution limits:

| Parameter | Type | Effect |
|---|---|---|
| `heap_memory_max_mb` | integer | Override the server's heap memory cap for this execution |
| `execution_timeout_secs` | integer (1–300) | Override the server's timeout for this execution |

```bash
curl -s -X POST http://localhost:8080/api/exec \
  -H 'Content-Type: application/json' \
  -d '{"code":"/* heavy work */","heap_memory_max_mb":32,"execution_timeout_secs":60}'
```

## See also

- [Quick-start: Asynchronous execution & output](../tutorials/async-execution.md)
- [Concepts: Asynchronous execution & output](../concepts/async-execution.md)
- [Reference: Asynchronous execution & output](../reference/async-execution.md)
- [How-to: Running JavaScript & TypeScript](js-execution.md)
- [Reference: HTTP API](../reference/http-api.md)
- [Concepts: Transports](../concepts/transports.md)
