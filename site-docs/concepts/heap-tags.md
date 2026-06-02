# Heap Tagging System

Heap tags are key-value metadata pairs that can be attached to any heap snapshot. They provide a way to annotate snapshots with application-level meaning -- labeling a snapshot as "production", marking it with a user ID, or categorizing it by purpose -- without modifying the snapshot itself.

## Why Tags Exist

Content-addressed heap snapshots are identified by SHA-256 hashes: opaque strings like `a3f8c2...`. While this is excellent for integrity and deduplication, it tells a human (or an agent) nothing about what a snapshot contains or what it is for. Tags bridge this gap by associating meaningful metadata with snapshot hashes.

Common use cases include:

- **Environment tagging** -- `env=production`, `env=staging`
- **Ownership** -- `owner=agent-42`, `team=data-pipeline`
- **Versioning** -- `version=3`, `checkpoint=after-training`
- **Workflow state** -- `step=data-loaded`, `status=validated`

## Data Model

Each heap snapshot can have zero or more tags. Tags are a flat `HashMap<String, String>` -- both keys and values are strings. Setting tags replaces the entire tag map for that snapshot; there is no partial update at the API level (though the engine does support atomic merge operations internally via sled's `fetch_and_update`).

Tags are stored separately from the snapshot data itself. A snapshot in S3 or on the filesystem has no knowledge of its tags. Tags live in sled, keyed by `ht:<heap_hash>`, with the value being a JSON-serialized map.

## Query Semantics

Tags can be queried with AND-matching semantics: given a set of filter tags, the system returns all snapshots whose tags contain every specified key-value pair. For example, querying with `{"env": "production", "owner": "agent-42"}` returns only snapshots that have both tags with those exact values.

This is a full scan over the sled tree's `ht:` prefix. There are no secondary indexes. For deployments with a small number of tagged snapshots (typical for agent workflows), this is fast. For very large tag stores, the scan cost scales linearly with the number of tagged snapshots.

## Persistence in Sled

Tags are stored in a dedicated sled database opened at `<session_db_path>/heap-tags`. The sled tree named `heap_tags` holds all entries. Each entry's key is `ht:<heap_hash>` and its value is the JSON-serialized tag map.

Sled provides:

- **Atomic writes** -- Tag updates are atomic at the sled level. The `merge_tags` operation uses `fetch_and_update` to atomically read-modify-write, preventing lost updates from concurrent writes (in standalone mode).
- **Durability** -- Tags survive server restarts because sled persists to disk.
- **Prefix scanning** -- The `ht:` prefix enables efficient iteration over all tagged snapshots.

## Cluster Replication

In cluster mode, tag writes are routed through Raft consensus. The `HeapTagStore` checks for a cluster node and, if present, uses `put_or_forward` to write through the Raft leader. Reads in cluster mode use `scan_prefix` on the replicated data tree.

This means tags are consistent across cluster nodes (subject to Raft replication lag). The trade-off is that `merge_tags` in cluster mode performs a non-atomic read-modify-write -- if two concurrent merges target the same snapshot, the last writer wins and the earlier writer's changes to different keys may be lost. This is documented as a known limitation.

## Tag Lifecycle

Tags are independent of snapshot lifecycle:

- Setting tags on a snapshot that does not exist in storage is permitted. The tag store does not verify that the referenced heap hash corresponds to a real snapshot.
- Deleting all tags for a snapshot writes an empty map as a tombstone, rather than removing the sled entry. This ensures that a heap that was explicitly "untagged" is distinguishable from one that was never tagged (though in practice the difference is rarely meaningful).
- Deleting specific tag keys performs a read-modify-write: the existing tag map is loaded, the specified keys are removed, and the result is written back.
