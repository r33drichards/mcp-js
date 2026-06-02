# WebAssembly Support

mcp-v8 can load and execute WebAssembly (WASM) modules alongside JavaScript. WASM modules are compiled and injected as global variables before user code runs, making them available for computation-intensive tasks, data processing, and even running full database engines (like SQLite) inside the sandbox.

## Two Injection Modes

WASM modules in mcp-v8 come in two flavors, determined by whether the module has imports:

### Auto-instantiated modules (no imports)

If a WASM module has no import declarations -- it is entirely self-contained -- mcp-v8 automatically instantiates it and exposes its exports as a global variable with the given name.

```
--wasm-module math=/path/to/math.wasm
```

In JavaScript:

```js
// math.add and math.multiply are directly available
const result = math.add(2, 3);
```

### Manual instantiation modules (has imports)

If a WASM module declares imports (like WASI interfaces or memory imports), mcp-v8 cannot auto-instantiate it because it does not know what imports object to provide. Instead, the compiled `WebAssembly.Module` is exposed as a global with a `__wasm_` prefix.

```
--wasm-module sqlite=/path/to/sqlite.wasm
```

In JavaScript:

```js
// The compiled module is available as __wasm_sqlite
// Instantiate it with the required imports:
const instance = new WebAssembly.Instance(__wasm_sqlite, {
  wasi_snapshot_preview1: { /* ... */ }
});
```

This two-tier approach handles the common case (simple WASM modules) automatically while still supporting complex modules that need custom import objects.

## Pre-Loading with --wasm-module

WASM modules are loaded at server startup, not at execution time. The `--wasm-module` flag specifies a name and a file path:

```
--wasm-module <name>=<path>[:<max_memory>]
```

The name must be a valid JavaScript identifier (starts with a letter, underscore, or dollar sign; contains only alphanumerics, underscores, and dollar signs). The optional memory suffix caps the module's native memory allocation.

For multiple modules or complex configurations, use `--wasm-config` with a JSON file:

```json
{
  "math": "/path/to/math.wasm",
  "sqlite": {
    "path": "/path/to/sqlite.wasm",
    "max_memory_bytes": 16777216
  }
}
```

## Validation with wasmparser

Before any WASM bytes reach V8, they pass through two validation stages:

1. **Structural validation** -- The `wasmparser` crate validates the binary format: section headers, type definitions, function bodies, and all other structural elements. Invalid or malformed WASM is rejected with a clear error message.

2. **Resource validation** -- mcp-v8 inspects the module's resource declarations (memory sections, table sections, and imported memories/tables) to verify they fit within the allocated budget. This prevents a WASM module from declaring, for example, 4 GB of linear memory when the budget is 16 MB.

Both checks happen before `WebAssembly.compile()` is called. This is critical because V8's WASM compiler allocates native memory (outside the managed JS heap) during compilation. Without pre-validation, a malicious or misconfigured module could allocate unbounded native memory and crash the process.

## Per-Module Memory Limits

WASM linear memory and table memory are allocated as native memory, separate from the V8 JavaScript heap. The `--heap-memory-max` flag does not cover WASM allocations. Instead, mcp-v8 provides per-module memory limits:

- **Per-module limit** -- Set via the `:max_memory` suffix on `--wasm-module` or the `max_memory_bytes` field in `--wasm-config`.
- **Default limit** -- Set via `--wasm-default-max-memory` (default: 16 MB). Applied to modules that do not specify a per-module limit.

The resource validator checks the module's declared initial memory pages against this limit:

```
pages * 64 KiB <= max_memory_bytes
```

Table elements are checked similarly, with an estimated 8 bytes per element. Both direct declarations and imported memories/tables are checked.

## Inline Compilation

In addition to pre-loaded modules, JavaScript code can compile WASM inline using the standard WebAssembly API:

```js
const bytes = new Uint8Array([0x00, 0x61, 0x73, 0x6d, ...]);
const module = new WebAssembly.Module(bytes);
const instance = new WebAssembly.Instance(module);
```

Inline compilation is subject to the V8 heap limit (for the `Uint8Array` holding the bytes) and the bounded ArrayBuffer allocator. Very large WASM modules compiled inline may hit these limits.

## The SQLite WASM Example

A practical demonstration of mcp-v8's WASM support is running SQLite compiled to WASM. The SQLite WASM build has WASI imports (for file I/O), so it uses the manual instantiation path:

1. The server is started with `--wasm-module sqlite=/path/to/sqlite3.wasm`.
2. JavaScript code receives `__wasm_sqlite` as a global.
3. The code provides a WASI shim as the imports object and instantiates the module.
4. SQL queries are executed through the SQLite C API exposed as WASM exports.

This demonstrates mcp-v8's ability to run substantial native-code libraries inside the sandbox, giving agents access to a full relational database without any filesystem or network access.
