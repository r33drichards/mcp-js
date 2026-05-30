# MCP Tools

The MCP tool surface depends on execution mode.

Stateful mode exposes:

- `run_js`
- `get_execution`
- `get_execution_output`
- `cancel_execution`
- `list_executions`
- `list_sessions`
- `list_session_snapshots`
- `get_heap_tags`
- `set_heap_tags`
- `delete_heap_tags`
- `query_heaps_by_tags`

Stateless mode exposes a simplified `run_js` that waits internally and returns
output directly.

This MCP tool surface is the primary integration surface for `mcp-v8`. The
HTTP API and generated clients expose a fallback path for environments that are
not speaking MCP directly.
