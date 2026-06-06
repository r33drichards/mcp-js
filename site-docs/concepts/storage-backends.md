# Heap storage backends

An explanation of how `mcp-v8` stores V8 heap snapshots, the trade-offs between the available backends, how the heap store relates to the sled session database, and what each choice means for clustering.

## The HeapStorage abstraction

In stateful mode every execution ends by serialising the V8 isolate's heap into a binary snapshot and handing it to the `HeapStorage` implementation. The trait has two operations:

- `put(name, bytes)` — persist a snapshot under a content-addressed name.
- `get(name) -> bytes` — retrieve a snapshot by name so a future isolate can warm-start from it.

At runtime one of three concrete backends is wired in as `AnyHeapStorage`. In stateless mode no backend is created at all.

```mermaid
flowchart TD
    call["run_js call"] --> mode{Mode}
    mode -->|"--stateless"| sl["Stateless Engine\nNo heap saved"]
    mode -->|"Stateful (default)"| sel{Storage backend selected}
    sel -->|"--directory-path DIR\nor default /tmp/mcp-v8-heaps"| fs["FileHeapStorage\n(local directory)"]
    sel -->|"--s3-bucket NAME"| s3["S3HeapStorage\n(S3 bucket)"]
    sel -->|"--s3-bucket NAME\n--cache-dir DIR"| wt["WriteThroughCacheHeapStorage\n(FS cache + S3)"]
    fs --> snap["Heap snapshot\n(content-addressed file)"]
    s3 --> snap
    wt --> snap
    snap -.->|"heap= on next call"| call
```

## The four backends

### FileHeapStorage — local filesystem

Each snapshot is a file in the configured directory. Reads and writes are synchronous filesystem operations. This backend has the lowest latency and zero external dependencies, making it the right default for a single-node setup.

The directory is created automatically if it does not exist. The default path when no storage flag is given is `/tmp/mcp-v8-heaps`; note that `/tmp` is typically cleared on reboot.

### S3HeapStorage — object storage

Each snapshot is stored as an S3 object keyed by its content hash. The AWS SDK client is initialised from the environment at startup. Reads and writes go over the network to S3.

S3 storage is the appropriate choice when multiple nodes need access to the same snapshot corpus — for example, in a horizontally-scaled deployment behind a load balancer, or in a Raft cluster where any node may receive any execution.

### WriteThroughCacheHeapStorage — FS cache in front of S3

A write-through cache that wraps `S3HeapStorage` with a local `FileHeapStorage`:

- **put**: the snapshot is written to the local cache directory first and then to S3. Both writes must succeed; if either fails the error is returned.
- **get**: the local cache is checked first. On a hit the data is returned immediately. On a miss the snapshot is fetched from S3, written into the local cache (a warning is logged if that write fails), and returned.

This backend provides near-local read latency for recently-used heaps while keeping S3 as the durable, shared source of truth. It is most useful when executions on a single node tend to reference the same heaps repeatedly.

### Stateless — no heap storage

When `--stateless` is passed the engine does not create any `HeapStorage` at all. Each `run_js` call starts a cold V8 isolate, runs the code synchronously, discards the isolate, and returns the output inline. No snapshot is ever written or read. The session log and heap tag index are not opened; the execution registry sled database (at `{session-db-path}/executions`) is still opened regardless of this flag.

## Durability and sharing trade-offs

| Backend | Durability across restarts | Shared across nodes | Read latency |
|---|---|---|---|
| FileHeapStorage (`/tmp/...` default) | No — `/tmp` cleared on reboot | No — single node only | Lowest |
| FileHeapStorage (persistent path) | Yes — survives reboots | No — single node only | Lowest |
| S3HeapStorage | Yes | Yes | Network round-trip |
| WriteThroughCacheHeapStorage | Yes (S3 copy) | Yes (shared via S3) | Local on cache hit |
| Stateless | N/A | N/A | N/A — no state |

For a single node with no clustering requirement, a persistent `--directory-path` on durable storage is simplest. For multi-node or clustered deployments, S3 (with or without a cache) is required so that all nodes see the same snapshots.

## The session DB vs the heap store

The heap store and the sled session DB are separate components that serve different purposes:

- **Heap store** (`--directory-path` / `--s3-bucket`): stores the raw binary V8 heap snapshots. Accessed only during execution. Content-addressed; the same bytes always produce the same key.
- **Session DB** (`--session-db-path`, default `/tmp/mcp-v8-sessions`): a sled embedded database that holds the session log, the heap tag index (at the `heap-tags` sub-path), the execution registry, and (when clustering is enabled) per-node Raft state.

The session DB tracks metadata — which heaps have been tagged, which executions belong to which session — but does not store the snapshot bytes themselves. Losing the session DB means losing session history and tags, but the snapshots in the heap store are unaffected and can still be referenced directly by hash. Losing the heap store means snapshots cannot be replayed even if session metadata survives.

## Relationship to clustering

A Raft cluster replicates the session log and heap tag index (both stored in the session DB) across all nodes. It does not replicate the raw snapshot bytes in the heap store.

For a Raft cluster to work correctly every node must be able to resolve any content-addressed heap name it receives. This means all nodes in the cluster must share the same heap store. FileHeapStorage on a local disk does not satisfy this requirement unless the directory is backed by a shared network filesystem. S3HeapStorage (with or without a local cache) is the recommended heap store for clustered deployments.

## See also

- [Quick-start: Heap storage backends](../tutorials/storage-backends.md)
- [How-to: Heap storage backends](../how-to/storage-backends.md)
- [Reference: Heap storage backends](../reference/storage-backends.md)
- [Concepts: Stateful sessions & heap snapshots](sessions-and-heaps.md)
- [Concepts: Clustering & replication (Raft)](clustering.md)
- [CLI flags reference](../reference/cli-flags.md)
