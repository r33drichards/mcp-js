# How to Load and Run WebAssembly Modules

Pre-load WASM modules as globals available to all JavaScript executions.

## Load a WASM module via CLI flag

```bash
mcp-v8 --wasm-module math=/path/to/math.wasm
```

The module's exports are available as a global variable named `math` in JavaScript:

```js
const result = math.add(1, 2);
console.log(result);
```

## Set a per-module memory limit

Append a memory limit after the path with a colon:

```bash
mcp-v8 --wasm-module math=/path/to/math.wasm:16m
```

Supported suffixes: raw bytes, `k`/`K` (KiB), `m`/`M` (MiB), `g`/`G` (GiB).

```bash
mcp-v8 --wasm-module math=/path/to/math.wasm:1048576   # 1 MiB in bytes
mcp-v8 --wasm-module math=/path/to/math.wasm:16m        # 16 MiB
```

## Load multiple modules

Repeat the flag:

```bash
mcp-v8 --wasm-module math=/path/to/math.wasm --wasm-module crypto=/path/to/crypto.wasm:32m
```

## Use a JSON config file

Create a JSON file mapping global names to WASM file paths:

```json
{
  "math": "/path/to/math.wasm",
  "crypto": {
    "path": "/path/to/crypto.wasm",
    "max_memory_bytes": 16777216
  }
}
```

Load it with:

```bash
mcp-v8 --wasm-config /etc/mcp-v8/wasm.json
```

## Set the default memory limit

Change the default max native memory for modules without a per-module limit (default is 16 MiB):

```bash
mcp-v8 --wasm-default-max-memory 32m
```

This limit applies to WASM linear memory, which is allocated as native memory outside the V8 heap. It is separate from `--heap-memory-max`.

## Use standard WebAssembly APIs

You can also instantiate WASM modules directly in JavaScript using `WebAssembly.Module` and `WebAssembly.Instance`, without pre-loading:

```js
const wasmBytes = new Uint8Array([0, 97, 115, 109, /* ... */]);
const module = new WebAssembly.Module(wasmBytes);
const instance = new WebAssembly.Instance(module);
console.log(instance.exports.add(1, 2));
```
