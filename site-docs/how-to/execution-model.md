# How to Choose Between Stateful and Stateless Mode

Decide whether executions persist V8 heap state or start fresh every time.

## Stateful mode (default)

Stateful mode saves a V8 heap snapshot after each execution. You can resume from any previous snapshot by passing its `heap` hash.

Start the server with a storage backend:

```bash
# Local filesystem storage
mcp-v8 --directory-path /var/lib/mcp-v8/heaps

# S3 storage
mcp-v8 --s3-bucket my-heap-bucket

# S3 with local cache
mcp-v8 --s3-bucket my-heap-bucket --cache-dir /tmp/heap-cache
```

The `run_js` tool accepts:

- `heap` -- SHA-256 hash from a previous execution to resume from
- `session` -- human-readable session name for tracking
- `tags` -- key/value metadata for the heap snapshot

Additional tools available in stateful mode:

- `get_execution` / `get_execution_output` -- poll status and read output
- `cancel_execution` -- stop a running execution
- `list_executions` -- list all executions
- `list_sessions` / `list_session_snapshots` -- browse session history
- `get_heap_tags` / `set_heap_tags` / `delete_heap_tags` / `query_heaps_by_tags` -- manage heap metadata

## Stateless mode

Stateless mode runs each execution in a fresh V8 isolate with no persistence. The `run_js` tool waits for completion and returns output directly.

```bash
mcp-v8 --stateless
```

The response includes `output` and `error` fields -- no need to poll.

Stateless mode is best for:

- Ephemeral computation (no state needed between calls)
- Load testing and benchmarks
- Environments where storage is unavailable

## When to use which

| Scenario | Mode |
|----------|------|
| Multi-step agent workflows | Stateful |
| Building up state across calls | Stateful |
| One-shot calculations | Stateless |
| Maximum throughput | Stateless |
| No storage available | Stateless |
