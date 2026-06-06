# Heap storage backends

We'll start `mcp-v8` with local filesystem storage, submit a JavaScript execution, and confirm that the resulting heap snapshot lands on disk.

## Prerequisites

- `mcp-v8` is installed and on your `PATH` (see [install overview](../install/overview.md)).
- `curl` and `jq` are available.

## 1. Create a heap directory and start the server

Create a directory for heap snapshots, then start the server pointing at it:

```bash
mkdir -p /tmp/my-heaps
mcp-v8 --http-port=8080 --directory-path=/tmp/my-heaps
```

`--directory-path` sets the directory where the server writes content-addressed V8 heap snapshots. `--http-port` exposes the REST API so we can call it with `curl`. Leave this terminal open.

## 2. Submit a JavaScript execution

Open a second terminal and POST a snippet to the REST API:

```bash
EXEC_ID=$(curl -s -X POST http://localhost:8080/api/exec \
  -H 'Content-Type: application/json' \
  -d '{"code": "const greeting = \"hello, heap\"; greeting"}' \
  | jq -r .execution_id)
echo "Execution: $EXEC_ID"
```

Expected output:

```
Execution: <hex-id>
```

## 3. Wait for completion

Poll until the execution status is `completed`:

```bash
curl -s "http://localhost:8080/api/executions/$EXEC_ID" | jq .status
```

Re-run the command until you see:

```
"completed"
```

## 4. Verify the snapshot on disk

After the execution completes the server has persisted the V8 heap snapshot:

```bash
ls -lh /tmp/my-heaps/
```

You should see one or more files whose names are content-addressed hex hashes, for example:

```
-rw-r--r--  1 user  staff  1.2M Jan  1 00:00 a3f7c8e1d2...
```

Each file is a V8 heap snapshot. A future execution that passes `"heap": "<hash>"` in its request will have its V8 isolate warm-started from that file instead of beginning fresh.

## What we did

We ran `mcp-v8` in stateful mode with a local filesystem backend (`--directory-path`). Every execution in stateful mode saves a heap snapshot after it completes. Without `--directory-path` the server uses `/tmp/mcp-v8-heaps` by default.

## See also

- [How-to: Heap storage backends](../how-to/storage-backends.md)
- [Concepts: Heap storage backends](../concepts/storage-backends.md)
- [Reference: Heap storage backends](../reference/storage-backends.md)
- [Concepts: Stateful sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Concepts: Clustering & replication (Raft)](../concepts/clustering.md)
- [CLI flags reference](../reference/cli-flags.md)
