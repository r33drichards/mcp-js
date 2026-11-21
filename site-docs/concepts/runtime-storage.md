# Runtime payloads and metadata publication

Artifact and console bytes are immutable objects; database records describe and
reference them. This separation applies to server-configured sled, SQLite, and
DynamoDB modes, including when heap/filesystem persistence is disabled.

## Payload identity and readers

New runtime objects use typed SHA-256 references under
`runtime/v2/artifact/<hash>` or `runtime/v2/console/<hash>`. Artifact metadata is
excluded from payload identity, so changing MIME type or timestamps does not
duplicate identical bytes. Readers verify the digest and size; missing or corrupt
objects fail without silently trying another backend.

The explicit `runtime_refs:v1:<hex tree name>` index distinguishes new records
from arbitrary legacy inline bytes. New readers accept existing inline data and
old DynamoDB outer-tagged `runtime/<canonical UUID>` references. Old binaries do
not understand the new index. Coordinate reader upgrades before new-format writes;
rollback requires compatible metadata and objects. Marker-only formats from
unmerged intermediate PR revisions need explicit export/migration if used.

## Publication and I/O

The blob write completes before its reference is published. Console output uses
256 KiB pages with byte-length/newline descriptors; pagination reads only the
objects needed for the requested window. Partial buffers flush at execution end.
Raw inline constructors keep 4 KiB pages until a blob backend is attached.

Metadata runs on its own reactor. Blob I/O has four workers and a bounded queue
of sixteen jobs, with a default ten-second deadline including queue time. Queue
saturation returns a retryable error. A timeout after a mutation starts may have
an uncertain outcome; do not equate it with proof that nothing was stored.

The separate Rust `MetadataPublication` facade adds revisions, fingerprinted
operation receipts, atomic companion writes, tombstones, and explicit unknown
outcome reconciliation. `ObjectPublication` writes immutable content before
metadata publication. These are opt-in APIs, not automatic migrations of every
existing domain. The optional `RaftMetadataStore` exposes local committed reads;
stronger reads are unsupported and pending CAS conflicts remain provisional.
Raft replication of a pointer does not replicate its blob contents.

## Reclaim local runtime objects

Overwrites and failed publication can leave unreferenced objects. The Rust API
`ExecutionRegistry::collect_runtime_objects(&registries, minimum_age)` can sweep
managed local Unix filesystem roots. It requires every registered metadata owner
sharing that root. Readers/publishers hold shared filesystem leases; collection
requires an exclusive lease and fails as busy while operations are pinned.

It retains reachable objects and applies the minimum age to unreachable ones.
Unmanaged/legacy ownership and symlinked runtime directories cause refusal.
Copied references alone do not pin objects; low-level staged writers must hold a
lease across object creation and metadata publication. All writers must cooperate.
There is no automatic sweep timer or HTTP endpoint. S3, write-through caches,
and shared cross-domain GC are unsupported by this collector.

For ownership manifests and migration boundaries, see the
[maintenance contract](https://github.com/r33drichards/mcp-js/blob/main/docs/runtime-blob-maintenance.md).

## Source

- [Versioned runtime indexes](https://github.com/r33drichards/mcp-js/blob/main/server/metadata-store/src/runtime_tree.rs)
- [Workers and deadlines](https://github.com/r33drichards/mcp-js/blob/main/server/metadata-store/src/runtime_worker.rs)
- [Publication facade](https://github.com/r33drichards/mcp-js/blob/main/server/metadata-store/src/publication.rs)
- [Local leases and collection](https://github.com/r33drichards/mcp-js/blob/main/server/src/engine/heap_storage.rs)
- [Configure metadata](../how-to/metadata-backends.md)
