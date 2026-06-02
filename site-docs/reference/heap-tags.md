# Heap Tags Reference

Heap tags are arbitrary key-value string pairs associated with heap snapshot hashes. Available in stateful mode only.

## MCP Tools

### get_heap_tags

Get all tags for a heap snapshot.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `heap` | `string` | Yes | 64-char SHA-256 hex hash of the heap snapshot |

**Response:**

```json
{
  "tags": {
    "key1": "value1",
    "key2": "value2"
  }
}
```

Returns an empty map `{}` if no tags exist for the heap.

### set_heap_tags

Set or replace all tags on a heap snapshot. This replaces all existing tags.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `heap` | `string` | Yes | 64-char SHA-256 hex hash |
| `tags` | `object` | Yes | Map of string key-value pairs |

**Response:**

```json
{
  "ok": true
}
```

### delete_heap_tags

Delete tags from a heap snapshot.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `heap` | `string` | Yes | 64-char SHA-256 hex hash |
| `keys` | `string?` | No | Comma-separated list of tag keys to remove. If omitted, all tags are deleted. |

**Response:**

```json
{
  "ok": true
}
```

Behavior:

- `keys` omitted: all tags are deleted (sets an empty map as tombstone)
- `keys` provided: only the specified keys are removed, other tags are preserved

### query_heaps_by_tags

Find heap snapshots whose tags contain all the specified key-value pairs.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `tags` | `object` | Yes | Map of key-value pairs to match |

**Response:**

```json
{
  "results": [
    {
      "heap": "a3f2b8c1d4e5...",
      "tags": {
        "env": "prod",
        "version": "1.0"
      }
    }
  ]
}
```

**Query Semantics:** AND matching -- a heap matches only if its tags contain **all** of the specified key-value pairs. Extra tags on the heap are ignored. Heaps with empty tag maps are excluded.

## Storage

Tags are stored in a sled database at `{session-db-path}/heap-tags/`.

- sled tree name: `heap_tags`
- Key format: `ht:{heap_hash}` (bytes)
- Value format: JSON-serialized `HashMap<String, String>`

## Cluster Behavior

In cluster mode, tag operations are replicated via Raft consensus:

- `set_heap_tags` and `delete_heap_tags` write through the Raft leader
- `get_heap_tags` reads from the local cluster node (prefix scan for exact key match)
- `query_heaps_by_tags` scans all entries with the `ht:` prefix from the local node

## Tagging at Execution Time

Tags can be set atomically during execution by passing the `tags` parameter to `run_js`:

```json
{
  "code": "console.log('hello')",
  "heap": "my-heap",
  "tags": {
    "env": "prod",
    "version": "1.0"
  }
}
```

Tags are stored after the heap snapshot is saved successfully.
