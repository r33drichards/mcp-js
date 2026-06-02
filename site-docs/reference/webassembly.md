# WebAssembly Reference

## Supported APIs

The standard `WebAssembly` global is always available with these APIs:

| API | Description |
|-----|-------------|
| `WebAssembly.Module(buffer)` | Compile a module synchronously |
| `WebAssembly.Instance(module, imports?)` | Instantiate a module synchronously |
| `WebAssembly.validate(buffer)` | Validate WASM bytes |
| `WebAssembly.compile(buffer)` | Compile a module asynchronously (returns Promise) |
| `WebAssembly.instantiate(buffer, imports?)` | Compile and instantiate asynchronously |

## Pre-loaded WASM Modules

WASM modules can be pre-loaded at server startup and injected as globals before every execution.

### --wasm-module Flag

```
--wasm-module <NAME>=<PATH>[:<MAX_MEMORY>]
```

Can be specified multiple times.

| Component | Description |
|-----------|-------------|
| `NAME` | Global variable name (must be a valid JS identifier) |
| `PATH` | Path to the `.wasm` file |
| `MAX_MEMORY` | Optional per-module native memory limit |

Examples:

```
--wasm-module math=/path/to/math.wasm
--wasm-module math=/path/to/math.wasm:16m
--wasm-module math=/path/to/math.wasm:1048576
```

### --wasm-config Flag

```
--wasm-config <PATH>
```

Path to a JSON file mapping global names to module configurations.

**String value format:**

```json
{
  "math": "/path/to/math.wasm"
}
```

**Object value format:**

```json
{
  "math": {
    "path": "/path/to/math.wasm",
    "max_memory_bytes": 16777216
  }
}
```

### --wasm-default-max-memory Flag

```
--wasm-default-max-memory <SIZE>
```

Default: `16m` (16 MiB)

Default maximum native memory for WASM modules without a per-module limit. This is separate from `--heap-memory-max` (JS heap) -- WASM linear memory is allocated as native memory outside the V8 heap.

## Global Naming

| Module Type | Global Name | Description |
|-------------|-------------|-------------|
| No imports | `<name>` | Auto-instantiated; global is the exports object |
| No imports | `__wasm_<name>` | Compiled `WebAssembly.Module` object |
| Has imports | `__wasm_<name>` only | Compiled `WebAssembly.Module`; must be manually instantiated |

Modules without imports get two globals: the auto-instantiated exports as `<name>` and the compiled module as `__wasm_<name>`.

Modules with imports only get `__wasm_<name>` and must be manually instantiated in JavaScript:

```js
var instance = new WebAssembly.Instance(__wasm_sqlite, {
  wasi_snapshot_preview1: { /* ... */ }
});
```

## Memory Size Parsing

Memory sizes support the following suffixes:

| Suffix | Unit | Multiplier |
|--------|------|------------|
| (none) | bytes | 1 |
| `k` or `K` | KiB | 1,024 |
| `m` or `M` | MiB | 1,048,576 |
| `g` or `G` | GiB | 1,073,741,824 |

Examples: `1048576`, `1024k`, `16m`, `1g`

## Resource Validation

Before V8 compiles a WASM module, the server validates:

1. **wasmparser validation**: Structural correctness of the `.wasm` bytes
2. **Memory declarations**: `initial` pages must fit within `max_memory_bytes / 65536` pages
3. **Table declarations**: `initial` elements must fit within `max_memory_bytes / 8` elements
4. **Import section**: Imported memories and tables are also validated against the budget

Constants:

| Constant | Value |
|----------|-------|
| WASM page size | 65,536 bytes (64 KiB) |
| Table element size estimate | 8 bytes |

## Module Name Validation

WASM module names must be valid JavaScript identifiers:

- Must start with a letter, underscore (`_`), or dollar sign (`$`)
- Subsequent characters must be alphanumeric, underscore, or dollar sign
- Cannot be empty
- Duplicate names across `--wasm-module` and `--wasm-config` are rejected
