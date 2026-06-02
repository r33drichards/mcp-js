# How to Tag and Query Heap Snapshots

Attach key/value metadata to heap snapshots and query them later.

## Tag a heap at creation time

Pass `tags` when calling `run_js`:

```json
{
  "code": "var model = 'gpt-4'; console.log('initialized');",
  "tags": { "env": "production", "project": "analysis-v2" }
}
```

The tags are stored with the resulting heap snapshot.

## Read tags from a heap

```
get_heap_tags({ heap: "a1b2c3d4..." })
```

Returns:

```json
{ "env": "production", "project": "analysis-v2" }
```

## Update tags on an existing heap

```
set_heap_tags({
  heap: "a1b2c3d4...",
  tags: { "status": "reviewed", "env": "staging" }
})
```

This merges with existing tags. Existing keys are overwritten; new keys are added.

## Delete specific tag keys

```
delete_heap_tags({
  heap: "a1b2c3d4...",
  keys: ["status"]
})
```

## Query heaps by tags

Find all heap snapshots matching tag criteria:

```
query_heaps_by_tags({
  tags: { "env": "production", "project": "analysis-v2" }
})
```

Returns a list of matching heap hashes. All specified tags must match (AND logic).
