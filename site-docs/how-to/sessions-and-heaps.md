# Stateful sessions & heap snapshots

Recipes for resuming a heap, tagging snapshots, querying by tag, naming a session, and inspecting session history. All operations in this guide require stateful mode (the default).

## Resume a heap from a previous execution

Pass the `heap` content hash returned by `get_execution` as the `heap` parameter of `run_js`. The V8 isolate is restored to the exact state that produced that snapshot before the new code runs.

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "console.log(globalThis.myData);",
    "heap": "a3f8b2c1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1"
  }
}
```

If the given hash is not found in storage the server starts a fresh isolate instead of returning an error.

## Tag a snapshot at execution time

Pass `tags` in the `run_js` call. Tags are applied to the **output** heap after the execution completes. This is a replace operation — any previous tags on that content hash are overwritten.

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "globalThis.model = loadModel();",
    "heap": "<previous-heap>",
    "tags": { "env": "production", "model": "v2" }
  }
}
```

## Tag a snapshot after the fact

Use `set_heap_tags` to set (replace all) tags on any heap at any time:

```json
{
  "tool": "set_heap_tags",
  "arguments": {
    "heap": "a3f8b2c1d4e5f6a7...",
    "tags": { "env": "staging", "approved": "true" }
  }
}
```

A successful call returns `{ "ok": true }`.

## Query heaps by tags

`query_heaps_by_tags` performs an exact-match filter: every key-value pair in `tags` must appear in the stored tags for a heap to be returned. Extra tags on the heap are ignored.

```json
{
  "tool": "query_heaps_by_tags",
  "arguments": { "tags": { "env": "production" } }
}
```

Response:

```json
{
  "results": [
    { "heap": "a3f8b2c1...", "tags": { "env": "production", "model": "v2" } },
    { "heap": "b9e2d4f1...", "tags": { "env": "production", "model": "v3" } }
  ]
}
```

## Delete specific tag keys from a heap

Pass a comma-separated list of key names in the `keys` parameter:

```json
{
  "tool": "delete_heap_tags",
  "arguments": {
    "heap": "a3f8b2c1d4e5f6a7...",
    "keys": "approved,model"
  }
}
```

Only the named keys are removed; all other tags remain.

## Delete all tags from a heap

Omit the `keys` parameter entirely:

```json
{
  "tool": "delete_heap_tags",
  "arguments": {
    "heap": "a3f8b2c1d4e5f6a7..."
  }
}
```

## Read the current tags on a heap

```json
{
  "tool": "get_heap_tags",
  "arguments": { "heap": "a3f8b2c1d4e5f6a7..." }
}
```

Returns `{ "tags": { "env": "production", "model": "v2" } }`. Returns an empty object if no tags are set.

## Name a session via X-MCP-Session-Id

Send the header `X-MCP-Session-Id: <name>` on the MCP `initialize` request (HTTP or SSE transport only). The server reads the header value and associates every subsequent `run_js` execution with that session name, writing a log entry for each execution.

Example using curl against the Streamable HTTP transport:

```bash
curl -s -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -H "X-MCP-Session-Id: my-agent-session" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"curl","version":"0"}}}'
```

All `run_js` calls made on this MCP connection are logged under the session name `my-agent-session`.

## Inspect session history

### List all sessions

```json
{ "tool": "list_sessions", "arguments": {} }
```

Returns `{ "sessions": ["my-agent-session", "other-session"] }`.

### List log entries for the current session

`list_session_snapshots` returns entries for the session identified by the `X-MCP-Session-Id` header sent on `initialize`. It accepts an optional `fields` parameter (comma-separated) to limit which fields are returned:

```json
{
  "tool": "list_session_snapshots",
  "arguments": { "fields": "index,input_heap,output_heap,timestamp" }
}
```

Response:

```json
{
  "entries": [
    {
      "index": 0,
      "input_heap": null,
      "output_heap": "a3f8b2c1...",
      "timestamp": "2024-11-05T12:00:00Z"
    },
    {
      "index": 1,
      "input_heap": "a3f8b2c1...",
      "output_heap": "b9e2d4f1...",
      "timestamp": "2024-11-05T12:00:05Z"
    }
  ]
}
```

Entries are ordered chronologically. If no `X-MCP-Session-Id` was sent during `initialize`, the tool returns an error object in the `entries` array.

### List all fields

Omit the `fields` parameter to return all fields: `index`, `input_heap`, `output_heap`, `code`, `timestamp`.

## See also

- [Tutorial: sessions & heap snapshots](../tutorials/sessions-and-heaps.md)
- [Concepts: sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Reference: sessions & heap snapshots](../reference/sessions-and-heaps.md)
- [Storage backends](../how-to/storage-backends.md)
- [Running JavaScript & TypeScript](../how-to/js-execution.md)
- [Asynchronous execution & output](../how-to/async-execution.md)
