# Asynchronous execution & output

This page explains why execution in mcp-v8 is asynchronous, how the submit-poll model works, how output is stored and paginated, and how execution status transitions through its lifecycle.

## Why execution is asynchronous

V8 JavaScript execution can be long-running — a script might loop, await network I/O, or process large datasets over several seconds or minutes. Blocking an MCP tool call or an HTTP request for that duration is impractical: MCP clients have their own timeouts, and HTTP connections may close.

mcp-v8 therefore separates submission from observation. Execution is submitted to the engine and run on a dedicated Tokio task. The session-capable surface normally returns an `execution_id` for polling;
`run_js` with `wait: true` instead waits for execution and persistence to stop.

## The submit-poll model

```
caller                   mcp-v8
  │                        │
  │── run_js / POST /api/exec ──▶│  register id, spawn task
  │◀── { execution_id } ──────── │
  │                        │ (V8 isolate running)
  │── get_execution ───────▶│
  │◀── { status: "running" }──── │
  │                        │ (V8 finishes)
  │── get_execution ───────▶│
  │◀── { status: "completed" }── │
  │── get_execution_output ─▶│
  │◀── { data, has_more } ───────│
```

The caller is free to poll as frequently or infrequently as it likes. Output can be fetched at any point — even before the execution finishes — because console output is written to persistent storage incrementally.

## The execution registry

All execution state is managed by `ExecutionRegistry` (in `server/src/engine/execution.rs`). It combines two storage layers:

- **In-memory `DashMap`** — holds the `ExecutionRecord` struct for each execution (status, isolate handle, result, timestamps). Lookups are O(1) and lock-free.
- **Runtime store** — console descriptors and artifact metadata use the selected
  sled, SQLite, or DynamoDB backend; their bytes are immutable S3/filesystem
  objects. SQLite/DynamoDB also persist execution records. The default sled
  registry retains live status records in memory, so console/artifact persistence
  does not imply that old status IDs can be queried after restart.

Registration opens an ordered console tree and creates the live `running` record.
Terminal handling updates status/timestamps and, for the configured remote
metadata adapter (SQLite/DynamoDB), persists the record. Isolate handles remain
process-local; reading durable metadata never resumes an interrupted execution.

## Max-concurrent executions

The server limits the number of V8 isolates that may run simultaneously via `--max-concurrent-executions` (default: CPU count). Submissions beyond this limit are queued. This prevents uncontrolled memory growth because each active isolate holds its own heap up to `heap_memory_max_mb` MB.

## Lifecycle and status transitions

An execution moves through the following states:

```mermaid
stateDiagram-v2
    [*] --> running : submit (run_js / POST /api/exec)
    running --> completed : execution returns normally
    running --> failed : unhandled exception / runtime error
    running --> timed_out : wall-clock timeout exceeded
    running --> cancelled : cancel_execution called
    completed --> [*]
    failed --> [*]
    timed_out --> [*]
    cancelled --> [*]
```

| Status | Meaning |
|---|---|
| `running` | The V8 isolate is active. |
| `completed` | The script returned without throwing. `result` holds the serialised return value (may be `null`). |
| `failed` | The script threw an unhandled exception or a runtime error occurred. `error` holds the message. |
| `timed_out` | The `execution_timeout_secs` wall-clock limit was reached. `error` is `"Execution timed out"`. |
| `cancelled` | `cancel_execution` was called while the execution was running. The V8 isolate was terminated immediately via its `IsolateHandle`. |

Cancellation is the only transition that is externally triggered. The other three terminal states are set by the execution task itself.

Once an execution reaches a terminal state it cannot transition again. Calling `cancel_execution` on a completed, failed, timed-out, or already-cancelled execution returns an error.

## Output pagination model

Server console output is stored in 256 KiB blob pages with ordered size/newline
descriptors in the metadata store. Pagination scans descriptors and fetches only
objects needed for the requested byte/line window; a missing later object need
not prevent reading an intact early window. Raw inline constructors retain 4 KiB
pages. Output becomes visible as full pages are published or the tail is flushed.

Two pagination modes are available:

### Line-based pagination

- Parameters: `line_offset` (1-based, default 1), `line_limit` (default 100).
- The response includes `start_line`, `end_line`, `next_line_offset`, and `total_lines`.
- Use `next_line_offset` as `line_offset` on the next call.

Line numbers are 1-based: `line_offset = 1` returns the first line. The internal algorithm computes `start_idx = line_offset - 1` so the two modes (line and byte) share the same underlying byte slice.

### Byte-based pagination

- Parameters: `byte_offset` (default 0), `byte_limit` (default 4096).
- Takes precedence over line-based parameters when `byte_offset` is present.
- The response includes `start_byte`, `end_byte`, `next_byte_offset`, and `total_bytes`.
- Use `next_byte_offset` as `byte_offset` on the next call.

### Cross-referencing

Both modes return all six cursor fields (`start_line`, `end_line`, `next_line_offset`, `start_byte`, `end_byte`, `next_byte_offset`) regardless of which mode was requested. This lets callers switch between line and byte windows without losing position.

The `has_more` flag is `true` when more output exists beyond the current page. The `status` field in the output response reflects the execution status at query time, allowing a caller to detect completion in a single round trip.

## Stateless mode and the synchronous shortcut

When both `--heap-store none` and `--fs-store none` are selected (the default) the `run_js` MCP tool internally submits the code, polls every 50 ms for up to 6000 iterations (300 seconds), collects all console output in a single pass, and returns `{ "output": "…" }` directly. This is a convenience wrapper — no separate polling or output-fetching calls are needed, but the caller blocks for the full execution duration. The async/session tools are not advertised by this stateless MCP surface;
artifact tools and capability discovery may still be present. Filesystem-only
persistence uses the session-capable surface without requiring heap snapshots.
REST execution endpoints have their own submit/query interface.

## Wait through persistence

On the session-capable MCP surface, `run_js` with `wait: true` waits for the
background execution activity to stop, including snapshot/runtime persistence.
The bounded wait returns the original ID with `wait_timed_out: true` when it
cannot settle; it never resubmits. Completed output is capped at 1 MiB in this
response, with `output_truncated` indicating that pagination is needed. This is
the appropriate mode for Lambda invocations that must remain open through writes.

Visible `cancelled` status can precede actual task shutdown. It is not proof that
all effects have stopped; the scheduler worker's cancellation protocol separately
waits for quiescence. Streamable HTTP/stdio also support the MCP tasks extension
for clients that negotiate it; the legacy SSE transport does not. A protocol
task ID and an engine execution ID are different identifiers.

## See also

- [How-to: Asynchronous execution & output](../how-to/async-execution.md)
- [Concepts: Running JavaScript & TypeScript](js-execution.md)
- [Concepts: Transports](transports.md)
- [Reference: HTTP API](../reference/http-api.md)
