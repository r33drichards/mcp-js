# Tutorial: Organizing Heap Snapshots with Tags

In this tutorial you will learn how to attach tags to heap snapshots, query snapshots by their tags, and manage the tag lifecycle. Tags help you organize and find specific states across sessions.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Familiarity with sessions and heaps ([Sessions and Heaps](sessions-and-heaps.md))

## Step 1: Start the server

```bash
mcp-v8 --http-port 3000 --directory-path /tmp/mcp-v8-tags-tutorial
```

## Step 2: Create some heap snapshots

Create a few snapshots to work with:

```bash
# Snapshot 1: a data processing setup
mcp-v8-cli --http-port 3000 exec --code '
var config = { mode: "production", version: "1.0" };
var data = [1, 2, 3, 4, 5];
"setup complete";
'
```

Note the heap hash. Then create a second snapshot by resuming:

```bash
# Snapshot 2: processed data
mcp-v8-cli --http-port 3000 exec --code '
var results = data.map(x => x * 2);
var status = "processed";
results;
' --heap-id <HEAP_HASH_1>
```

Note this heap hash too. Create a third independent snapshot:

```bash
# Snapshot 3: a different experiment
mcp-v8-cli --http-port 3000 exec --code '
var config = { mode: "experimental", version: "2.0" };
var data = [10, 20, 30];
"experiment setup";
'
```

Note this third heap hash.

## Step 3: Set tags on the first snapshot

Tags are key-value pairs you attach to a heap snapshot. Set tags on the first snapshot:

```bash
mcp-v8-cli --http-port 3000 set-heap-tags \
  --heap-id <HEAP_HASH_1> \
  --tags '{"environment": "production", "stage": "setup", "project": "data-pipeline"}'
```

## Step 4: Set tags on the second snapshot

```bash
mcp-v8-cli --http-port 3000 set-heap-tags \
  --heap-id <HEAP_HASH_2> \
  --tags '{"environment": "production", "stage": "processed", "project": "data-pipeline"}'
```

## Step 5: Set tags on the third snapshot

```bash
mcp-v8-cli --http-port 3000 set-heap-tags \
  --heap-id <HEAP_HASH_3> \
  --tags '{"environment": "experimental", "stage": "setup", "project": "data-pipeline"}'
```

## Step 6: Get tags for a specific snapshot

Retrieve the tags attached to any snapshot:

```bash
mcp-v8-cli --http-port 3000 get-heap-tags --heap-id <HEAP_HASH_1>
```

Expected output:

```json
{
  "environment": "production",
  "stage": "setup",
  "project": "data-pipeline"
}
```

## Step 7: Query snapshots by tags

Find all snapshots tagged with a specific environment:

```bash
mcp-v8-cli --http-port 3000 query-heaps-by-tags \
  --tags '{"environment": "production"}'
```

This returns the heap hashes for Snapshot 1 and Snapshot 2, since both are tagged `environment: production`.

## Step 8: Query with multiple tag filters

Narrow your search by specifying multiple tags:

```bash
mcp-v8-cli --http-port 3000 query-heaps-by-tags \
  --tags '{"environment": "production", "stage": "processed"}'
```

This returns only Snapshot 2, which matches both tags.

## Step 9: Find all setup snapshots across environments

```bash
mcp-v8-cli --http-port 3000 query-heaps-by-tags \
  --tags '{"stage": "setup"}'
```

This returns Snapshot 1 and Snapshot 3, both of which are in the "setup" stage but in different environments.

## Step 10: Update tags on a snapshot

You can update tags by setting new values. This merges with existing tags:

```bash
mcp-v8-cli --http-port 3000 set-heap-tags \
  --heap-id <HEAP_HASH_2> \
  --tags '{"stage": "verified", "reviewer": "alice"}'
```

Now Snapshot 2 has updated tags:

```json
{
  "environment": "production",
  "stage": "verified",
  "project": "data-pipeline",
  "reviewer": "alice"
}
```

## Step 11: Delete tags from a snapshot

Remove specific tags when they are no longer needed:

```bash
mcp-v8-cli --http-port 3000 delete-heap-tags \
  --heap-id <HEAP_HASH_2> \
  --tags '["reviewer"]'
```

The `reviewer` tag is removed, leaving the other tags intact.

## Step 12: Verify the deletion

```bash
mcp-v8-cli --http-port 3000 get-heap-tags --heap-id <HEAP_HASH_2>
```

Expected output:

```json
{
  "environment": "production",
  "stage": "verified",
  "project": "data-pipeline"
}
```

## Practical use cases for tags

Tags are especially useful for AI agents that need to:

- **Track experiment branches**: Tag snapshots with `experiment: A` vs `experiment: B`
- **Mark checkpoints**: Tag known-good states with `status: stable`
- **Organize by purpose**: Tag with `purpose: data-cleaning` or `purpose: analysis`
- **Find states to resume from**: Query by tags to locate the right starting point

## What you learned

- How to set key-value tags on heap snapshots with `set_heap_tags`
- How to retrieve tags for a specific snapshot with `get_heap_tags`
- How to query across all snapshots by tag values with `query_heaps_by_tags`
- How to update existing tags by setting new values
- How to delete specific tags with `delete_heap_tags`
- That tags enable organized, searchable state management across sessions

Next, learn about transport options in [Connecting via Different Transports](transports.md).
