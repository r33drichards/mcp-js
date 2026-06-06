# Stateful sessions & heap snapshots

Complete parameter and response reference for the session and heap MCP tools, session log entry fields, tag operations, and the heap key format. All items on this page are stateful mode only.

## Heap key format

A heap key is the lowercase hex-encoded SHA-256 of the raw V8 snapshot payload (before the mcp-v8 framing header is applied). It is always 64 hexadecimal characters.

Example: `a3f8b2c1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1`

The key doubles as the storage object name. On the filesystem backend the file at `<directory-path>/<heap-key>` contains the full wrapped snapshot. On S3 it is the object key in the configured bucket.

### Snapshot file layout

| Bytes | Content |
|-------|---------|
| 0 – 9 | ASCII magic `MCPV8SNAP` followed by a null byte (`\x00`) |
| 10 – 41 | SHA-256 of the V8 payload (raw 32 bytes) |
| 42 – end | V8 snapshot payload; minimum size 100 KiB |

The checksum is verified on every read. A file that fails verification is rejected and the execution fails.

## `run_js` — heap-related parameters

The following parameters of `run_js` (stateful mode) relate to sessions and heaps. For the complete `run_js` reference see [js-execution](../reference/js-execution.md).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `heap` | string | No | Content hash of an existing snapshot to restore before running `code`. If omitted or empty, a fresh isolate is used. If the hash is not found in storage, a fresh isolate is used. |
| `tags` | object (string→string) | No | Key-value tags to set on the **output** heap after execution. Replaces any existing tags on that content hash. |

The `get_execution` response for a completed stateful execution includes:

| Field | Type | Description |
|-------|------|-------------|
| `heap` | string | Content hash of the snapshot produced by this execution. |

## Session log entry fields

Each entry returned by `list_session_snapshots` is a JSON object with the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `index` | integer | Monotonically increasing sequence number within the session. |
| `input_heap` | string \| null | Content hash of the heap passed to this execution. `null` if no heap was provided (fresh isolate). |
| `output_heap` | string | Content hash of the heap produced by this execution. |
| `code` | string | The JavaScript source that was executed (after TypeScript stripping). |
| `timestamp` | string | RFC 3339 UTC timestamp of when the entry was written. |

## MCP tools

### `list_sessions`

Returns the names of all sessions that have at least one log entry.

**Parameters:** none

**Response:**

```json
{ "sessions": ["my-agent-session", "other-session"] }
```

---

### `list_session_snapshots`

Returns the ordered log entries for the session associated with the current MCP connection. The session is identified by the `X-MCP-Session-Id` header sent on the MCP `initialize` request. If no session ID was captured, the response contains a single error entry.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `fields` | string | No | Comma-separated list of fields to include in each entry. Valid values: `index`, `input_heap`, `output_heap`, `code`, `timestamp`. When omitted, all fields are returned. |

**Response:**

```json
{
  "entries": [
    {
      "index": 0,
      "input_heap": null,
      "output_heap": "a3f8b2c1...",
      "code": "globalThis.x = 1;",
      "timestamp": "2024-11-05T12:00:00Z"
    }
  ]
}
```

Error (no session ID):

```json
{ "entries": [{ "error": "no session ID available (send X-MCP-Session-Id header)" }] }
```

---

### `get_heap_tags`

Retrieves all tags associated with a heap content hash.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `heap` | string | Yes | 64-character hex content hash. |

**Response:**

```json
{ "tags": { "env": "production", "model": "v2" } }
```

Returns `{ "tags": {} }` if no tags are set.

---

### `set_heap_tags`

Sets (replaces) all tags for a heap content hash. Any previously stored tags are removed and replaced by the provided map.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `heap` | string | Yes | 64-character hex content hash. |
| `tags` | object (string→string) | Yes | New tag map. To clear all tags, pass an empty object. |

**Response:**

```json
{ "ok": true }
```

On error: `{ "ok": false, "error": "<message>" }`

---

### `delete_heap_tags`

Removes tags from a heap content hash. When `keys` is provided, only the named keys are removed and the rest remain. When `keys` is omitted, all tags are deleted (equivalent to `set_heap_tags` with an empty map).

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `heap` | string | Yes | 64-character hex content hash. |
| `keys` | string | No | Comma-separated list of tag key names to remove. Omit to delete all tags. |

**Response:**

```json
{ "ok": true }
```

On error: `{ "ok": false, "error": "<message>" }`

---

### `query_heaps_by_tags`

Searches all tagged heaps. A heap is included in the results only if its stored tags contain **every** key-value pair in the filter. Heaps with additional tags beyond the filter are included.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `tags` | object (string→string) | Yes | Filter map. All entries must match. |

**Response:**

```json
{
  "results": [
    { "heap": "a3f8b2c1...", "tags": { "env": "production", "model": "v2" } },
    { "heap": "b9e2d4f1...", "tags": { "env": "production", "model": "v3" } }
  ]
}
```

Returns `{ "results": [] }` if no heaps match. Only heaps that have at least one tag stored are considered; untagged heaps are never returned.

## X-MCP-Session-Id header

The `X-MCP-Session-Id` header is read from the HTTP `initialize` request (Streamable HTTP and SSE transports only). The value becomes the session name used in session log writes and in `list_session_snapshots`. Any `X-MCP-*` header sent on `initialize` is also captured and made available to the engine as MCP headers.

The header is processed once per MCP connection at `initialize` time. Subsequent requests on the same connection inherit the session name set at initialization.

## Storage backend for snapshots

Snapshots are persisted by the configured heap storage backend:

- `--directory-path=DIR` (default `/tmp/mcp-v8-heaps`) — local filesystem; each snapshot is a file named by its content hash.
- `--s3-bucket=NAME` — AWS S3; each snapshot is an object whose key is the content hash.
- `--s3-bucket=NAME --cache-dir=DIR` — S3 with a local filesystem write-through cache.

Session log entries and heap tag metadata are stored in the sled embedded database at `--session-db-path` (default `/tmp/mcp-v8-sessions`).

## See also

- [Tutorial: sessions & heap snapshots](../tutorials/sessions-and-heaps.md)
- [How-to: sessions & heap snapshots](../how-to/sessions-and-heaps.md)
- [Concepts: sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Storage backends](../reference/storage-backends.md)
- [Running JavaScript & TypeScript](../reference/js-execution.md)
- [Asynchronous execution & output](../reference/async-execution.md)
