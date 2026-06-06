# Stateful sessions & heap snapshots

In this tutorial we'll run a `run_js` call that sets a variable, capture the returned heap key, then restore that state in a second call — demonstrating how mcp-v8 preserves JavaScript state across executions.

## Prerequisites

- mcp-v8 running in stateful mode (the default — no `--stateless` flag). Heap snapshots are written to `--directory-path` (default `/tmp/mcp-v8-heaps`). See [storage backends](../concepts/storage-backends.md) for other backend options.
- An MCP client connected to the server over stdio, HTTP, or SSE.

## Step 1: Run code that builds state

Call `run_js` with code that writes to a global variable:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "globalThis.counter = (globalThis.counter ?? 0) + 1;\nconsole.log('counter:', globalThis.counter);"
  }
}
```

The tool responds immediately with an execution ID — the execution runs asynchronously:

```json
{ "execution_id": "7f3a9c1d-4e2b-4a80-8f1c-0d9e3b2a1c5f" }
```

## Step 2: Poll for the heap key

Call `get_execution` until `status` is `completed`. The response includes a `heap` field containing the content hash of the saved snapshot:

```json
{
  "tool": "get_execution",
  "arguments": { "execution_id": "7f3a9c1d-4e2b-4a80-8f1c-0d9e3b2a1c5f" }
}
```

Expected response when complete:

```json
{
  "execution_id": "7f3a9c1d-4e2b-4a80-8f1c-0d9e3b2a1c5f",
  "status": "completed",
  "heap": "a3f8b2c1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1",
  "result": null,
  "error": null
}
```

The `heap` value is a 64-character hex string — the SHA-256 of the V8 snapshot payload. Save it; you will pass it to the next call.

## Step 3: Resume state in a second call

Pass the `heap` value as the `heap` parameter:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "globalThis.counter = (globalThis.counter ?? 0) + 1;\nconsole.log('counter:', globalThis.counter);",
    "heap": "a3f8b2c1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1"
  }
}
```

Poll `get_execution` again. Fetch the console output:

```json
{
  "tool": "get_execution_output",
  "arguments": { "execution_id": "<new-execution-id>" }
}
```

The `data` field will contain `counter: 2`, confirming the heap was restored and the variable survived across calls.

## Step 4: Tag the snapshot for later retrieval

We can attach key-value metadata to any heap so it can be found without storing the hash separately:

```json
{
  "tool": "set_heap_tags",
  "arguments": {
    "heap": "a3f8b2c1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1",
    "tags": { "stage": "tutorial", "step": "after-increment" }
  }
}
```

Later, find it by tag:

```json
{
  "tool": "query_heaps_by_tags",
  "arguments": { "tags": { "stage": "tutorial" } }
}
```

Response:

```json
{
  "results": [
    {
      "heap": "a3f8b2c1d4e5f6a7...",
      "tags": { "stage": "tutorial", "step": "after-increment" }
    }
  ]
}
```

## What we covered

- `run_js` (stateful) always produces a new heap snapshot and returns its content hash via `get_execution`.
- Passing `heap` to a subsequent `run_js` call restores the V8 isolate to that exact state.
- `set_heap_tags` / `query_heaps_by_tags` let you label and search snapshots without tracking hashes manually.

## See also

- [How-to: sessions & heap snapshots](../how-to/sessions-and-heaps.md)
- [Concepts: sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Reference: sessions & heap snapshots](../reference/sessions-and-heaps.md)
- [Storage backends](../concepts/storage-backends.md)
- [Running JavaScript & TypeScript](../tutorials/js-execution.md)
- [Asynchronous execution & output](../tutorials/async-execution.md)
