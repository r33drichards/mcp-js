# Storage Architecture

mcp-v8 stores heap snapshots through a pluggable storage layer defined by the `HeapStorage` trait. The trait is simple -- just `put` and `get` -- but the available backends cover a range of deployment scenarios from local development to production cloud infrastructure.

## The HeapStorage Trait

The storage interface is intentionally minimal:

```rust
trait HeapStorage: Send + Sync + 'static {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String>;
    async fn get(&self, name: &str) -> Result<Vec<u8>, String>;
}
```

- `put` stores a snapshot blob under a content-addressed key (the SHA-256 hash of the raw snapshot data).
- `get` retrieves a snapshot blob by its key.

There is no `delete`, `list`, or `update` operation. Snapshots are immutable: once stored, they are never modified. This simplicity makes the storage layer easy to implement and reason about.

## Four Backends

### FileHeapStorage

The default backend. Stores snapshots as files in a directory on the local filesystem. The filename is the content hash.

- **Default path** -- `/tmp/mcp-v8-heaps`
- **Configuration** -- `--directory-path /path/to/heaps`
- **Characteristics** -- Fast, no external dependencies, but limited to a single machine. Does not survive container restarts unless the directory is on a persistent volume.

### S3HeapStorage

Stores snapshots in an Amazon S3 bucket (or S3-compatible service). The S3 key is the content hash.

- **Configuration** -- `--s3-bucket my-bucket-name`
- **Authentication** -- Uses the standard AWS SDK credential chain (environment variables, IAM roles, credential files).
- **Characteristics** -- Durable, accessible from multiple nodes, suitable for production. Latency is higher than local filesystem due to network round-trips.

### WriteThroughCacheHeapStorage

A composite backend that wraps any primary storage with a local filesystem cache. This is designed for S3 deployments where read latency matters.

- **Configuration** -- `--s3-bucket my-bucket --cache-dir /tmp/heap-cache`
- **Write behavior** -- On `put`, the snapshot is written to both the local cache and the primary storage. The local write happens first for speed.
- **Read behavior** -- On `get`, the local cache is checked first. On cache miss, the snapshot is fetched from the primary storage and written to the cache for future reads.
- **Cache population** -- Cache population on miss is best-effort: if the local write fails, the snapshot is still returned from the primary. A warning is logged but the operation succeeds.

This backend provides S3's durability with local-disk read performance for frequently accessed snapshots.

### Stateless (no storage)

When `--stateless` is specified, no `HeapStorage` is created. The Engine operates without snapshot persistence. There is no storage overhead and no storage configuration required.

## Content-Addressed Keys

All storage keys are SHA-256 hashes of the raw V8 snapshot data (before the envelope wrapper is applied). This means:

- **Deduplication is automatic** -- Storing a snapshot that already exists is a no-op (the file/object already exists with that key).
- **Integrity is verifiable** -- The content can be re-hashed and compared to its key at any time.
- **No coordination needed** -- Multiple writers producing the same snapshot state will write the same key with the same content. There are no conflicts.

The envelope (magic header + checksum + payload) is what gets stored. The content hash is computed from the payload alone, so the storage key refers to the V8 data, not the envelope.

## Write-Through Cache Strategy

The write-through cache is specifically designed for the content-addressed storage pattern:

- **Writes go to both stores** -- Since content-addressed writes are idempotent, writing to both the cache and the primary is safe even if one succeeds and the other fails. The data is correct in whichever store it reaches.
- **Cache is not authoritative** -- The cache can be deleted entirely without data loss. It is a performance optimization, not a durability layer.
- **No cache invalidation needed** -- Because snapshots are immutable, cached data never becomes stale. A snapshot with hash X will always have the same content. There is no TTL and no eviction policy (beyond filesystem space limits).

## Default Paths

| Backend | Default path | Configured by |
|---|---|---|
| File | `/tmp/mcp-v8-heaps` | `--directory-path` |
| S3 | (bucket name required) | `--s3-bucket` |
| Cache | (requires `--cache-dir`) | `--cache-dir` |
| Session DB | `/tmp/mcp-v8-sessions` | `--session-db-path` |
| Execution registry | `<session-db>/executions[-port]` | Derived from session-db |
| Heap tag store | `<session-db>/heap-tags` | Derived from session-db |
| Cluster DB | `<session-db>/cluster-<node-id>` | Derived from session-db |

The `/tmp` defaults are convenient for development but inappropriate for production. Persistent volumes or explicit paths should be configured for any deployment where data must survive restarts.
