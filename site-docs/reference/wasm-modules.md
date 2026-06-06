# WebAssembly modules — reference

Complete reference for the `--wasm-module`, `--wasm-config`, and
`--wasm-default-max-memory` flags, module name rules, and the JavaScript-side
global shape.

## --wasm-module

```
--wasm-module <name>=<path>[:<max_memory>]
```

Repeatable. Loads the `.wasm` file at `<path>` and registers it under `<name>`.
`--wasm-module` and `--wasm-config` can be used together; duplicate names across
either source are rejected at startup.

| Component | Description |
|-----------|-------------|
| `name` | JavaScript global name; see [Module name rules](#module-name-rules) |
| `path` | Absolute or relative path to the `.wasm` file |
| `max_memory` | Optional; a size string (see [Size suffixes](#size-suffixes)) that overrides `--wasm-default-max-memory` for this module only |

**Examples:**

```bash
--wasm-module math=/opt/wasm/math.wasm
--wasm-module sqlite=/opt/wasm/sqlite3.wasm:32m
--wasm-module codec=/opt/wasm/codec.wasm:1g
```

## --wasm-config

```
--wasm-config <path>
```

Path to a JSON file containing module definitions. The top-level value must be a
JSON object. Each key is a module name (same rules as `--wasm-module`). Each value
is one of:

**String** — path to the `.wasm` file; uses the default memory cap.

**Object** — path plus an optional per-module cap in bytes:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string | yes | Path to the `.wasm` file |
| `max_memory_bytes` | integer | no | Per-module memory cap in bytes; overrides `--wasm-default-max-memory` |
| `description` | string | no | Human description shown on the module's MCP [stub tool](#mcp-stub-tools) (overridden by `--wasm-stub-description`) |

**Example:**

```json
{
  "math": "/opt/wasm/math.wasm",
  "sqlite": {
    "path": "/opt/wasm/sqlite3.wasm",
    "max_memory_bytes": 33554432,
    "description": "In-memory SQLite database (exec/query SQL)."
  }
}
```

## --wasm-default-max-memory

```
--wasm-default-max-memory <size>
```

Default memory cap applied to every module that does not specify its own cap via
the `:max_memory` suffix or the `max_memory_bytes` config field.

| Default | Effective bytes |
|---------|----------------|
| `16m` | 16 777 216 (16 MiB) |

## Size suffixes

Used in `--wasm-default-max-memory` and the `:max_memory` suffix of
`--wasm-module`. Case-insensitive.

| Suffix | Multiplier | Example input | Effective bytes |
|--------|-----------|---------------|-----------------|
| `k` or `K` | 1 024 | `32k` | 32 768 |
| `m` or `M` | 1 048 576 | `16m` | 16 777 216 |
| `g` or `G` | 1 073 741 824 | `1g` | 1 073 741 824 |
| (none) | 1 | `1048576` | 1 048 576 |

## Module name rules

| Constraint | Detail |
|-----------|--------|
| First character | ASCII letter (`a`–`z`, `A`–`Z`), underscore `_`, or dollar sign `$` |
| Subsequent characters | ASCII alphanumeric (`a`–`z`, `A`–`Z`, `0`–`9`), underscore, or dollar sign |
| Empty name | Rejected at startup |
| Duplicate name | Rejected at startup |

## Memory cap enforcement

The cap is a structural check that runs before the module is compiled into V8. Both
the module's own sections and any imported memory or table are checked against the
same budget.

| Resource checked | Budget formula | Example with 16 MiB cap |
|-----------------|---------------|-------------------------|
| Declared initial memory (`mem.initial`) | `cap_bytes / 65 536` pages | 256 pages |
| Declared initial table elements | `cap_bytes / 8` elements | 2 097 152 elements |

If `mem.initial` exceeds the page budget, or the table's initial element count
exceeds the element budget, the execution fails immediately before any user code
runs.

The check covers `initial` only. Modules compiled with `ALLOW_MEMORY_GROWTH=1`
can grow beyond their declared initial memory at runtime; the cap does not install
a runtime allocator limit.

## JavaScript-side global shape

### Modules without imports

The module is compiled and instantiated automatically. The `exports` object of the
resulting `WebAssembly.Instance` is assigned to `globalThis[name]`.

```js
// Global: math = WebAssembly.Instance.exports
const sum = math.add(1, 2);          // call an exported function
const val = math.someGlobal.value;   // read an exported global
```

### Modules with imports

The module is compiled but not instantiated. The `WebAssembly.Module` object is
assigned to `globalThis["__wasm_" + name]`. User code must supply an import object
and call `new WebAssembly.Instance(...)`.

```js
// Global: __wasm_sqlite = WebAssembly.Module

// Inspect what imports the module needs
var needed = WebAssembly.Module.imports(__wasm_sqlite);

// Build an import object (example: WASI stubs)
var importObject = { wasi_snapshot_preview1: { fd_write: function() { return 0; } } };

var instance = new WebAssembly.Instance(__wasm_sqlite, importObject);
var exports = instance.exports;      // exports.sqlite3_open, .malloc, etc.
```

A server-side warning is printed to stderr when a module with imports is loaded,
noting the `__wasm_<name>` global name and that manual instantiation is required.

## MCP stub tools

Each loaded module is also advertised on the server's own MCP surface as a
**stub tool** so a downstream MCP client can discover it via `tools/list` or tool
search without reading server configuration. This mirrors the
[upstream MCP tool stubs](mcp-client.md).

| Aspect | Detail |
|--------|--------|
| Stub tool name | `<prefix>wasm__<name>` (default prefix `runjs__`, e.g. `runjs__wasm__sqlite`) |
| Description | Auto-generated usage hint (the `__wasm_<name>` global, export names, whether an imports object is needed), plus any configured description |
| Calling the stub | Returns instructional text only — it is **not** an executable proxy. WASM exports have no MCP-level schema, so the agent must drive the module from JavaScript via `run_js`. |

| Flag | Default | Description |
|------|---------|-------------|
| `--wasm-stubs <bool>` | `true` whenever ≥1 module is loaded | Advertise loaded modules as stub tools. Pass `--wasm-stubs false` to disable. |
| `--wasm-stub-prefix <prefix>` | `runjs__` | Prefix for stub tool names. No effect when `--wasm-stubs false`. |
| `--wasm-stub-description <name>=<text>` | — | Set the stub description for a loaded module (repeatable). Overrides a `description` set inline in `--wasm-config`. The named module must be loaded. |

## See also

- [How to use WebAssembly modules](../how-to/wasm-modules.md)
- [WebAssembly modules — concepts](../concepts/wasm-modules.md)
- [CLI flags reference](../reference/cli-flags.md)
- [Running JavaScript & TypeScript](../how-to/js-execution.md)
