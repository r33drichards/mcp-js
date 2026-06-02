# SQLite WASM Example

Run a full [SQLite](https://sqlite.org/) database inside mcp-v8 using [WebAssembly](https://github.com/sqlite/sqlite-wasm).

The SQLite `.wasm` module is pre-loaded at server startup with `--wasm-module`. Because the WASM binary has WASI imports, it cannot be auto-instantiated — the compiled `WebAssembly.Module` is exposed as `__wasm_sqlite`, and the JavaScript code provides the necessary import stubs and instantiates it manually.

## Prerequisites

- [Emscripten SDK](https://emscripten.org/docs/getting_started/downloads.html) (`emcc` on PATH)
- mcp-v8 binary (built from this repo or installed via `install.sh`)

## Build

```bash
./examples/sqlite-wasm/build.sh
```

This downloads the [SQLite amalgamation](https://sqlite.org/amalgamation.html) and compiles it to WASM. The output is `examples/sqlite-wasm/sqlite3.wasm`.

### Compile flags

Key flags used during compilation:

| Flag | Purpose |
|------|---------|
| `STANDALONE_WASM=1` | Produce a WASI-compatible standalone `.wasm` file |
| `SQLITE_OS_OTHER=1` | Disable platform-specific VFS (only `:memory:` databases) |
| `SQLITE_THREADSAFE=0` | Single-threaded (no mutex overhead) |
| `FILESYSTEM=0` | No Emscripten filesystem emulation |
| `--no-entry` | No `main()` — library-only |
| `ALLOW_MEMORY_GROWTH=1` | WASM linear memory can grow as needed |

## Run

### Stateless mode

```bash
mcp-v8 --stateless --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

### Stateful mode (persistent sessions)

```bash
mcp-v8 --directory-path /tmp/mcp-v8-heaps --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

In stateful mode the SQLite wrapper code is snapshotted into the V8 heap, so you only need to initialize it once and can continue using it across subsequent calls by passing the `heap` hash.

### HTTP transport

```bash
mcp-v8 --stateless --http-port 8080 --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

Then test with curl:

```bash
curl -s http://localhost:8080/api/exec \
  -H 'Content-Type: application/json' \
  -d "$(cat examples/sqlite-wasm/example.js | jq -Rs '{code: .}')"
```

## Example code

See [`example.js`](example.js) for a complete working example. The key steps are:

### 1. Provide WASI import stubs

```javascript
var wasiStubs = {
    fd_close: function () { return 0; },
    fd_write: function (fd, iovs, iovsLen, nwrittenPtr) {
        // discard output
        var view = new DataView(memory.buffer);
        var total = 0;
        for (var i = 0; i < iovsLen; i++)
            total += view.getUint32(iovs + i * 8 + 4, true);
        view.setUint32(nwrittenPtr, total, true);
        return 0;
    },
    fd_read: function () { return 0; },
    fd_seek: function () { return 0; },
    fd_fdstat_get: function () { return 0; },
    environ_get: function () { return 0; },
    environ_sizes_get: function (countPtr, bufSizePtr) {
        var view = new DataView(memory.buffer);
        view.setUint32(countPtr, 0, true);
        view.setUint32(bufSizePtr, 0, true);
        return 0;
    },
    proc_exit: function () {},
    clock_time_get: function (id, precLo, precHi, timePtr) {
        var view = new DataView(memory.buffer);
        view.setBigUint64(timePtr, BigInt(0), true);
        return 0;
    },
};
```

### 2. Instantiate the module

```javascript
// __wasm_sqlite is the compiled WebAssembly.Module (set by --wasm-module)
var instance = new WebAssembly.Instance(__wasm_sqlite, {
    wasi_snapshot_preview1: wasiStubs,
    env: { emscripten_notify_memory_growth: function () {} },
});
var exports = instance.exports;
var memory = exports.memory;
```

### 3. Use SQLite

```javascript
var db = new SQLite();                         // opens :memory: database
db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)");
db.exec("INSERT INTO t (val) VALUES ('hello')");
var result = db.query("SELECT * FROM t");
db.close();

JSON.stringify(result.rows);
// → [{"id":1,"val":"hello"}]
```

## How it works

```
┌──────────────────────────────────────────────────────────┐
│  mcp-v8  (Rust)                                          │
│                                                          │
│  --wasm-module sqlite=sqlite3.wasm                       │
│      │                                                   │
│      ├─ wasmparser: validates .wasm bytes                │
│      ├─ v8::WasmModuleObject::compile(bytes)             │
│      ├─ detects imports → skips auto-instantiation       │
│      └─ sets global: __wasm_sqlite = WebAssembly.Module  │
│                                                          │
│  run_js("... example.js ...")                            │
│      │                                                   │
│      ├─ JS provides WASI stubs                           │
│      ├─ new WebAssembly.Instance(__wasm_sqlite, imports) │
│      ├─ SQLite C API via WASM exports                    │
│      └─ returns JSON result                              │
└──────────────────────────────────────────────────────────┘
```

## Limitations

- **In-memory databases only** — compiled with `SQLITE_OS_OTHER=1`, so there is no filesystem VFS. Only `:memory:` databases work.
- **No async** — all SQLite operations are synchronous (which is fine since mcp-v8's runtime is synchronous).
- **No extensions** — `SQLITE_OMIT_LOAD_EXTENSION` is set.
- **Heap size** — for large databases, you may need to increase `--heap-memory-max` (default 8 MB). The WASM linear memory also grows independently.

## Claude Desktop / Cursor configuration

```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": [
        "--stateless",
        "--wasm-module", "sqlite=/path/to/sqlite3.wasm",
        "--heap-memory-max", "32"
      ]
    }
  }
}
```
