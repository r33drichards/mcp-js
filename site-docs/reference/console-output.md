# Console Output Reference

## Console Methods

| Method | Level | Prefix | Output Format |
|--------|-------|--------|--------------|
| `console.log(...)` | 0 | (none) | `{message}\n` |
| `console.info(...)` | 1 | `[INFO]` | `[INFO] {message}\n` |
| `console.warn(...)` | 2 | `[WARN]` | `[WARN] {message}\n` |
| `console.error(...)` | 3 | `[ERROR]` | `[ERROR] {message}\n` |

All methods append a newline character after the message.

## Storage Mechanism

Console output is buffered in memory and flushed to a per-execution sled tree in fixed-size pages (WAL-style writes).

- Page size: 4096 bytes
- Tree name: `ex:{execution_id}`
- Key: sequential `u64` in big-endian byte order
- Value: raw bytes (up to 4096 per page)
- Remaining bytes are flushed on execution completion

## Pagination Modes

The `get_execution_output` tool and `GET /api/executions/{id}/output` endpoint support two pagination modes.

### Line-Based Pagination (Default)

Used when `byte_offset` is not provided.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `line_offset` | `u64?` | `1` | Start reading from this line number (1-indexed) |
| `line_limit` | `u64?` | `100` | Maximum number of lines to return |

### Byte-Based Pagination

Used when `byte_offset` is provided. Takes precedence over line-based parameters.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `byte_offset` | `u64?` | (none) | Start reading from this byte offset (0-indexed) |
| `byte_limit` | `u64?` | `4096` | Maximum number of bytes to return |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| `execution_id` | `string` | Execution UUID |
| `data` | `string` | Text content for the requested window |
| `start_line` | `u64` | First line number in this page |
| `end_line` | `u64` | Last line number in this page |
| `next_line_offset` | `u64` | Line offset to pass for the next page |
| `total_lines` | `u64` | Total lines written so far |
| `start_byte` | `u64` | First byte offset in this page |
| `end_byte` | `u64` | Last byte offset in this page (exclusive) |
| `next_byte_offset` | `u64` | Byte offset to pass for the next page |
| `total_bytes` | `u64` | Total bytes written so far |
| `has_more` | `bool` | Whether more output is available beyond this page |
| `status` | `string` | Execution status at the time of this query |

## Iteration Pattern

To read all output:

```
1. GET /api/executions/{id}/output
2. If has_more == true, GET /api/executions/{id}/output?line_offset={next_line_offset}
3. Repeat until has_more == false
```

Or using byte-based pagination:

```
1. GET /api/executions/{id}/output?byte_offset=0&byte_limit=8192
2. If has_more == true, GET /api/executions/{id}/output?byte_offset={next_byte_offset}&byte_limit=8192
3. Repeat until has_more == false
```

## Cross-Referencing

Both line and byte coordinates are always returned regardless of which pagination mode is used. This allows switching between modes mid-stream.
