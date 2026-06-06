# Use SQLite WASM

Run a full SQLite database inside `mcp-v8` using the existing SQLite WASM
example from this repo.

The SQLite `.wasm` module is pre-loaded at server startup with
`--wasm-module`. Because the binary has WASI imports, it is not
auto-instantiated. Instead, `mcp-v8` exposes the compiled
`WebAssembly.Module` as `__wasm_sqlite`, and the JavaScript wrapper creates
the `WebAssembly.Instance` with the imports it needs.

## Prerequisites

- Emscripten SDK with `emcc` on `PATH`
- `mcp-v8` built from this repo or installed on the machine


## Clone the example repository

```bash
git clone --depth 1 github.com/r33drichard/mcp-js
cd mcp-js
```

## Build the module

```bash
./examples/sqlite-wasm/build.sh
```

This produces:

```text
examples/sqlite-wasm/sqlite3.wasm
```

## Run the server

### Stateless mode

```bash
mcp-v8 --stateless --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

### Stateful mode

```bash
mcp-v8 --directory-path /tmp/mcp-v8-heaps \
  --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

In stateful mode, the SQLite wrapper and initialized runtime can be snapshotted
into the V8 heap so an agent can continue working with the same in-memory
database across later runs.

### HTTP mode

```bash
mcp-v8 --stateless --http-port 8080 \
  --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

## Run the example

The repo already includes a working JavaScript example:

```bash
curl -s http://localhost:8080/api/exec \
  -H 'Content-Type: application/json' \
  -d "$(cat examples/sqlite-wasm/example.js | jq -Rs '{code: .}')"
```

## What the wrapper does

The SQLite example follows three steps:

1. provide WASI-style import stubs
2. instantiate `__wasm_sqlite` with `new WebAssembly.Instance(...)`
3. call the SQLite wrapper methods from JavaScript

The key instantiation step looks like this:

```javascript
var instance = new WebAssembly.Instance(__wasm_sqlite, {
    wasi_snapshot_preview1: wasiStubs,
    env: { emscripten_notify_memory_growth: function () {} },
});
```

Then the wrapper can open an in-memory database and run SQL:

```javascript
var db = new SQLite();
db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)");
db.exec("INSERT INTO t (val) VALUES ('hello')");
var result = db.query("SELECT * FROM t");
db.close();
JSON.stringify(result.rows);
```

## Discover loaded modules via stub tools

By default, every module loaded with `--wasm-module` / `--wasm-config` is
also advertised on the MCPJS MCP surface as a stub tool named
`runjs__wasm__<name>` (so the SQLite module above appears as
`runjs__wasm__sqlite`). A downstream MCP client can discover the module via
`tools/list` or tool search without reading server configuration.

The stub is not an executable proxy: calling it returns instructions telling
the agent to use the module from JavaScript via `run_js` (the compiled module
is available as the `__wasm_<name>` global, exactly as described above).

Control this behaviour with:

- `--wasm-stubs false` to hide the stubs
- `--wasm-stub-prefix <prefix>` to change the `runjs__` prefix
- `--wasm-stub-description <name>=<text>` to add a human description to a
  module's stub (shown to agents alongside the auto-generated usage hint)

The description can also be set inline in `--wasm-config`:

```json
{
  "sqlite": {
    "path": "examples/sqlite-wasm/sqlite3.wasm",
    "description": "In-memory SQLite database (exec/query SQL)."
  }
}
```

This mirrors the [MCP server tool stubs](../concepts/mcp-pass-through.md).

## Limits to keep in mind

- the example is built for in-memory databases only
- SQLite operations are synchronous
- large databases may require increasing `--heap-memory-max`
- WASM linear memory growth is separate from the V8 heap limit

For the full source material, see `examples/sqlite-wasm/README.md` and
`examples/sqlite-wasm/example.js` in the repository.
