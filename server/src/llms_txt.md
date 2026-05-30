# mcp-v8

> A Rust-based MCP (Model Context Protocol) server that runs JavaScript and TypeScript inside a V8 engine. Designed for AI agents that need a sandboxed, stateful JS runtime as a tool.

mcp-v8 exposes a V8 JavaScript runtime as MCP tools. Agents can run JS/TS code, persist V8 heap state between calls (stateful mode), import npm/JSR packages at runtime, and optionally use OPA-gated fetch and filesystem access.

## Source

- GitHub: https://github.com/r33drichards/mcp-js
- Install: `curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash`

## Connecting to this server

- MCP (Streamable HTTP): `POST /mcp`
- MCP (SSE): `GET /sse` + `POST /message`
- REST API: `POST /api/exec`, `GET /api/executions/{id}`, etc.
- OpenAPI spec: `GET /api-doc/openapi.json`
- Full docs: `GET /docs`

## MCP Tools

### Core tools

- Stateful MCP: `run_js(code, [heap], [heap_memory_max_mb], [execution_timeout_secs], [tags])` submits async execution and returns `execution_id`; use `get_execution(execution_id)` and `get_execution_output(execution_id, ...)` to poll/read output.
- Stateless MCP: `run_js(code, [heap_memory_max_mb], [execution_timeout_secs])` waits internally and returns `{output, error}` directly.
- Stateful MCP only: `cancel_execution(execution_id)` and `list_executions()`.

### Additional tools (stateful mode only)

- `run_js` gains extra params: `heap` (SHA-256 to resume from), `session` (human-readable session name), `tags` (key-value metadata for the heap snapshot).
- `list_sessions()` — List all named sessions.
- `list_session_snapshots(session, [fields])` — Browse execution history for a session.
- `get_heap_tags(heap)` — Get tags for a heap snapshot.
- `set_heap_tags(heap, tags)` — Set tags on a heap snapshot.
- `delete_heap_tags(heap, keys)` — Delete tag keys from a heap snapshot.
- `query_heaps_by_tags(tags)` — Find heap snapshots matching tag criteria.

## Typical agent workflow

```
Stateful mode:

1. run_js({ code: "console.log(1 + 1);" })
   → { execution_id: "abc-123" }

2. get_execution({ execution_id: "abc-123" })
   → { status: "completed", result: null, heap: "sha256..." }

3. get_execution_output({ execution_id: "abc-123" })
   → { data: "2\n", total_lines: 1 }

Stateless mode:

1. run_js({ code: "console.log(1 + 1);" })
   → { output: "2\n" }
```

In stateful mode, pass the returned `heap` hash back to `run_js` to resume that V8 state. For MCP session history, send `X-MCP-Session-Id` instead of a `session` tool parameter.

## JavaScript features

- TypeScript (type removal via SWC — not type-checked)
- ES modules, top-level `await`
- `console.log/info/warn/error` → readable via `get_execution_output`
- npm/JSR/URL imports via esm.sh (requires `--allow-external-modules`)
- WebAssembly (`WebAssembly.Module`, `WebAssembly.Instance`)
- Optional `fetch()` (OPA-gated, web-standard Fetch API)
- Optional fetch header injection via `--fetch-header` / `--fetch-header-config` (static headers or OAuth client-credentials bearer tokens)
- Optional `fs` module (OPA-gated, Node.js-compatible)
- Optional pre-loaded WASM globals

## Limitations

- No `setTimeout` / `setInterval`
- No DOM / browser APIs
- No environment variable access
- External imports disabled by default (enable with `--allow-external-modules`)
- `fetch()` requires `--policies-json`
- `fs` requires `--policies-json`

## REST API quick reference

| Method | Path | Description |
|--------|------|-------------|
| POST | /api/exec | Submit JS code for async execution |
| GET | /api/executions | List all executions |
| GET | /api/executions/{id} | Get execution status + result |
| GET | /api/executions/{id}/output | Read paginated console output |
| POST | /api/executions/{id}/cancel | Cancel a running execution |
| GET | /api-doc/openapi.json | OpenAPI 3.0 spec |
| GET | /docs | Full documentation |
| GET | /llms.txt | This file |

## MCP Resources

When connecting via MCP, the following resources are available via `resources/list` and `resources/read`:

| URI | Description |
|-----|-------------|
| `docs://readme` | Full README (Markdown) |
| `docs://llms-txt` | This llms.txt (Markdown) |
| `docs://openapi` | OpenAPI 3.0 spec (JSON) |
| `docs://tools` | MCP tool list (JSON) |
