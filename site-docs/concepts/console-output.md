# Console Output Architecture

Console output in mcp-v8 is more than just logging -- it is a structured, streamable, paginated data channel between the executing JavaScript code and the caller. Because executions are asynchronous (the caller gets an execution ID immediately and polls for results), console output must be available for reading while execution is still in progress.

## How Console Output is Captured

JavaScript's `console.log`, `console.info`, `console.warn`, and `console.error` are replaced with custom implementations during sandbox setup. These replacements call a Rust op (`op_console_write`) that writes formatted text into a buffered storage system.

Each log level gets a distinct prefix:

- `console.log(...)` and `console.debug(...)` -- no prefix
- `console.info(...)` -- `[INFO]` prefix
- `console.warn(...)` -- `[WARN]` prefix
- `console.error(...)` -- `[ERROR]` prefix

Arguments are formatted by converting each value to a string (using `JSON.stringify` for objects) and joining them with spaces, mimicking standard browser console behavior.

## WAL-Based Storage in Sled

Console output is stored using a Write-Ahead Log (WAL) pattern in sled, an embedded key-value database. Each execution gets its own sled tree (named `ex:<execution_id>`), and output bytes are appended as sequentially-keyed pages.

The write flow is:

1. The `ConsoleLogState` maintains an in-memory buffer.
2. As `op_console_write` is called, bytes are appended to the buffer.
3. When the buffer reaches the page size (4096 bytes), a full page is flushed to sled with a monotonically increasing sequence number as the key.
4. When execution completes (or the runtime is dropped), any remaining bytes in the buffer are flushed as a final partial page.

This design has several advantages:

- **Streaming** -- Pages are flushed incrementally during execution, so callers can read output before execution finishes.
- **Efficiency** -- Buffering avoids a sled write for every `console.log` call. The 4 KB page size balances write frequency against latency.
- **Ordering** -- Sequence numbers (stored as big-endian u64 keys) preserve the exact order of output.

## Dual Pagination: Lines and Bytes

Console output can be queried using two independent coordinate systems:

### Line-based pagination

The caller specifies `line_offset` (1-based) and `line_limit`. The system splits the full output on newlines, skips to the requested offset, and returns up to `line_limit` lines. This is the default mode and is convenient for human-readable output.

### Byte-based pagination

The caller specifies `byte_offset` and `byte_limit`. The system returns raw bytes from the given offset. This mode is useful for large outputs where the caller wants precise control over how much data to transfer.

Both modes return a response that includes coordinates in both systems:

```json
{
  "data": "...",
  "start_line": 1,
  "end_line": 10,
  "next_line_offset": 11,
  "total_lines": 42,
  "start_byte": 0,
  "end_byte": 4096,
  "next_byte_offset": 4096,
  "total_bytes": 17234,
  "has_more": true
}
```

## Cross-Referencing Coordinates

Every response includes both line and byte coordinates regardless of which pagination mode was used. This means a caller can start with line-based pagination, note the byte offset in the response, and switch to byte-based pagination for subsequent requests (or vice versa).

The `has_more` flag and `next_line_offset` / `next_byte_offset` fields enable simple cursor-based pagination: keep fetching with the "next" offset until `has_more` is false.

## Streaming During Execution

Because pages are flushed to sled as the buffer fills, a caller can poll for console output while the execution status is still `running`. The `total_lines` and `total_bytes` fields reflect the output written so far. On subsequent polls, the caller passes the previous `next_line_offset` or `next_byte_offset` to get only the new output.

This streaming capability is particularly valuable for long-running scripts that produce incremental output -- for example, a script that processes items in a loop and logs progress.

## Design Rationale

Storing console output in sled rather than keeping it purely in memory ensures that output survives even if the caller disconnects and reconnects (within the server's lifetime). The per-execution tree structure means that reading one execution's output never interferes with another's, and cleanup is straightforward -- dropping the sled tree removes all output for that execution.

The dual pagination system exists because different consumers have different needs. AI agents typically want line-based pagination (it maps naturally to text processing). Programmatic clients that transfer large outputs benefit from byte-based pagination for precise chunking.
