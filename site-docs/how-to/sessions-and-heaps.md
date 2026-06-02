# How to Manage Sessions and Resume from Heap Snapshots

Use heap snapshots to persist V8 state between executions and sessions to organize execution history.

## Resume from a heap snapshot

1. Run code. The response includes a `heap` field (SHA-256 content hash):

    ```json
    { "code": "var counter = 1; console.log(counter);" }
    ```

    After completion, `get_execution` returns:

    ```json
    { "status": "completed", "heap": "a1b2c3d4..." }
    ```

2. Pass the `heap` hash to the next call to resume that V8 state:

    ```json
    { "code": "counter += 1; console.log(counter);", "heap": "a1b2c3d4..." }
    ```

    The second execution sees `counter` as `2`.

3. Omit `heap` to start with a fresh isolate.

## Use named sessions

Pass a `session` parameter to group executions under a human-readable name:

```json
{ "code": "var x = 10;", "session": "my-analysis" }
```

List all sessions:

```
list_sessions()
```

Browse a session's execution history:

```
list_session_snapshots({ session: "my-analysis" })
```

Optionally filter returned fields:

```
list_session_snapshots({ session: "my-analysis", fields: ["heap", "status", "started_at"] })
```

## MCP session identity

When connecting via MCP, session identity comes from the `X-MCP-Session-Id` header during initialization, not from the `session` tool parameter. The `session` parameter is for tagging/logging only.

## Configure the session database

The session database path defaults to `/tmp/mcp-v8-sessions`. Override it with:

```bash
mcp-v8 --session-db-path /var/lib/mcp-v8/sessions
```
