# Run SQLite in WebAssembly

In this tutorial you'll build the SQLite WebAssembly module bundled with this
repo, load it into `mcp-v8`, and run SQL against an in-memory database from
JavaScript — end to end.

SQLite is compiled to a WASI-targeted `.wasm` module. Because it has WASI
imports, `mcp-v8` does **not** auto-instantiate it; instead the server exposes
the compiled module as the global `__wasm_sqlite`, and the JavaScript wrapper
creates the `WebAssembly.Instance` with the imports SQLite needs. See
[WebAssembly modules](../concepts/wasm-modules.md) for the why behind this.

## Prerequisites

- The Emscripten SDK with `emcc` on your `PATH` (used to compile SQLite).
- `mcp-v8` built or installed (see [Install](../install/overview.md)).
- `curl` and `jq` for sending the example over HTTP.

## Step 1 — Get the repo

```bash
git clone --depth 1 https://github.com/r33drichards/mcp-js
cd mcp-js
```

## Step 2 — Build the module

```bash
./examples/sqlite-wasm/build.sh
```

This downloads the SQLite amalgamation, compiles it with `emcc`, and writes:

```text
examples/sqlite-wasm/sqlite3.wasm
```

## Step 3 — Start the server with the module loaded

Pre-load the `.wasm` file under the global name `sqlite` with `--wasm-module`.
Run over HTTP so we can submit the example with `curl`:

```bash
mcp-v8 --stateless --http-port 8080 \
  --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

The `--wasm-module` value is `name=/path/to/module.wasm` — see
[CLI flags](../reference/cli-flags.md) for the full syntax (including per-module
memory caps).

## Step 4 — Run the example

The repo ships a complete JavaScript example at
`examples/sqlite-wasm/example.js`. Submit it to the async execution API:

```bash
curl -s http://localhost:8080/api/exec \
  -H 'Content-Type: application/json' \
  -d "$(jq -Rs '{code: .}' examples/sqlite-wasm/example.js)"
```

That returns an `execution_id`; poll it and read the output (see
[Asynchronous execution & output](../how-to/async-execution.md)):

```bash
curl -s http://localhost:8080/api/executions/<execution_id>/output
```

You'll see the query results the example prints, e.g.:

```json
{"users":[{"id":2,"name":"Bob","email":"bob@example.com","age":25}, ...],
 "stats":{"count":3,"avg_age":30}}
```

## Discover the module from an MCP client

You loaded `sqlite` with `--wasm-module`, and the server automatically advertises
it on its MCP surface as a stub tool named `runjs__wasm__sqlite`. A downstream MCP
client finds it via `tools/list` or tool search — no need to read server config.
The stub isn't an executable proxy: calling it returns instructions to drive the
module from JavaScript via `run_js` (it's the `__wasm_sqlite` global, exactly as
below). Add a human description so agents know what it's for:

```bash
mcp-v8 --stateless --http-port 8080 \
  --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm \
  --wasm-stub-description sqlite="In-memory SQLite database (exec/query SQL)."
```

Use `--wasm-stubs false` to hide stubs or `--wasm-stub-prefix` to change the
`runjs__` prefix. See [WebAssembly modules — how-to](../how-to/wasm-modules.md).

## What the wrapper does

The example performs three steps.

**1. Provide WASI-style import stubs** (`wasi_snapshot_preview1`) plus an `env`
import for Emscripten's memory-growth notification.

**2. Instantiate the exposed module:**

```javascript
var instance = new WebAssembly.Instance(__wasm_sqlite, {
    wasi_snapshot_preview1: wasiStubs,
    env: { emscripten_notify_memory_growth: function () {} },
});
```

**3. Drive SQLite through the wrapper's class:**

```javascript
var db = new SQLite();
db.exec("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT, age INTEGER)");
db.exec("INSERT INTO users (name, email, age) VALUES ('Alice', 'alice@example.com', 30)");
var result = db.query("SELECT * FROM users ORDER BY age");
db.close();
JSON.stringify(result.rows);
```

## Persisting the database (stateful mode)

In stateless mode the database lives only for that one execution. To carry it
across calls, run statefully and pass the returned `heap` key to the next
`run_js`:

```bash
mcp-v8 --directory-path /tmp/mcp-v8-heaps \
  --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

The initialized SQLite runtime and its in-memory data are captured in the V8
heap snapshot, so a later run that restores that heap continues with the same
database. See [Stateful sessions & heap snapshots](../how-to/sessions-and-heaps.md).

## Things to keep in mind

- The example uses an in-memory database.
- SQLite calls here are synchronous.
- WASM linear memory is separate from the V8 heap limit; large datasets may need
  a higher per-module cap (`--wasm-module name=...:64m`) and/or a higher
  `--heap-memory-max` for the snapshot.

For the full source, see `examples/sqlite-wasm/example.js` and
`examples/sqlite-wasm/build.sh`.

## See also

- [WebAssembly modules — how-to](../how-to/wasm-modules.md)
- [WebAssembly modules — concepts](../concepts/wasm-modules.md)
- [WebAssembly modules — reference](../reference/wasm-modules.md)
- [Asynchronous execution & output](../how-to/async-execution.md)
