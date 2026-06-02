# Sessions and Heaps Reference

## Heap Hash Format

Heap snapshot keys are 64-character lowercase hexadecimal SHA-256 hashes of the raw V8 snapshot data.

Format: `[0-9a-f]{64}`

Example: `a3f2b8c1d4e5f67890abcdef1234567890abcdef1234567890abcdef12345678`

The hash is computed as:

```rust
let hash = sha256_hash(raw_snapshot_data);
let content_hash = hash.iter().map(|b| format!("{:02x}", b)).collect::<String>();
```

## Snapshot Envelope Format

V8 snapshots are wrapped in an envelope before storage. The envelope provides defense-in-depth against invalid data reaching V8 (which would call `abort()`).

```
┌────────────────────┬───────────────────────────┬──────────────────────┐
│ Magic (10 bytes)   │ SHA-256 Checksum (32 bytes)│ V8 Snapshot Payload  │
└────────────────────┴───────────────────────────┴──────────────────────┘
```

| Field | Offset | Size | Value |
|-------|--------|------|-------|
| Magic header | 0 | 10 bytes | `MCPV8SNAP\0` (literal bytes `4d 43 50 56 38 53 4e 41 50 00`) |
| SHA-256 checksum | 10 | 32 bytes | SHA-256 hash of the payload |
| V8 snapshot payload | 42 | variable | Raw V8 snapshot data |

### Validation Rules

1. Total envelope size must be at least 42 bytes (header length)
2. First 10 bytes must match the magic header `MCPV8SNAP\0`
3. Payload must be at least 100 KB (100 * 1024 bytes) -- valid V8 snapshots are always larger
4. SHA-256 of payload must match the stored checksum

### Constants

```
SNAPSHOT_MAGIC = b"MCPV8SNAP\x00"   // 10 bytes
SNAPSHOT_HEADER_LEN = 42             // 10 (magic) + 32 (SHA-256)
MIN_SNAPSHOT_PAYLOAD = 102400        // 100 KB
```

## Session Database

Session logs and heap tags are stored in sled embedded databases.

### Default Path

```
/tmp/mcp-v8-sessions
```

Configurable via `--session-db-path`.

### Database Layout

| Path | Contents |
|------|----------|
| `{session-db-path}/` | Session log sled database (tree per session) |
| `{session-db-path}/heap-tags/` | Heap tag store sled database |
| `{session-db-path}/executions/` | Execution registry sled database (stdio mode) |
| `{session-db-path}/executions-{port}/` | Execution registry sled database (HTTP/SSE mode, port-suffixed) |
| `{session-db-path}/cluster-{node-id}/` | Cluster consensus sled database (cluster mode) |

### Session Log Entries

Each session log entry is stored in a sled tree named by the session identifier. Entries are appended sequentially.

| Field | Type | Description |
|-------|------|-------------|
| `input_heap` | `string?` | Heap hash used as input (null for first execution) |
| `output_heap` | `string` | Heap hash produced by execution |
| `code` | `string` | JavaScript/TypeScript source that was executed |
| `timestamp` | `string` | ISO-8601 timestamp |

### Session Log sled Tree Naming

- Tree prefix for session entries: `session:{session_name}`
- Each entry key is a sequential u64 in big-endian byte order

## Heap Storage

Heap snapshots (wrapped in the envelope format) are stored using one of three backends:

| Backend | Selection | Key Format |
|---------|-----------|------------|
| Filesystem | Default, or `--directory-path` | File named by 64-char hex hash |
| S3 | `--s3-bucket` | S3 object key = 64-char hex hash |
| S3 + FS cache | `--s3-bucket` + `--cache-dir` | S3 primary + local filesystem cache |

Default filesystem path: `/tmp/mcp-v8-heaps`

## Stateless Mode

When `--stateless` is set:

- No heap snapshots are saved or loaded
- No `heap` parameter in `run_js`
- No session log
- No heap tags
- The `run_js` tool returns console output directly instead of an execution ID
- Only the `run_js` MCP tool is exposed (no `get_execution`, `list_executions`, etc.)
