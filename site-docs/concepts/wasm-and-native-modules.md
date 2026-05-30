# WASM and Native Modules

`mcp-v8` supports WebAssembly through the standard JavaScript `WebAssembly`
API and can also pre-load `.wasm` modules when the server starts.

That gives the runtime two distinct WASM shapes:

- code that loads or compiles WebAssembly inside JavaScript
- modules preloaded by the server through `--wasm-module` or `--wasm-config`

```mermaid
flowchart TD
  A[server startup] --> B[load wasm file or config]
  B --> C[compile WebAssembly.Module]
  C --> D[expose __wasm_name compiled module]
  D --> E{module has imports?}
  E -->|no| F[auto-instantiate and expose name exports]
  E -->|yes| G[user JavaScript creates WebAssembly.Instance]
  F --> H[call exported wasm functions]
  G --> H
```

Preloading has two paths. The compiled `WebAssembly.Module` is always exposed
as `__wasm_<name>`. If the module is self-contained, `mcp-v8` also
auto-instantiates it and exposes its exports directly on `<name>`.

If the module requires imports, auto-instantiation is skipped. That is the
path used by WASI-style modules such as SQLite: JavaScript reads
`__wasm_<name>`, supplies the required imports, and creates its own
`WebAssembly.Instance`.

The most important conceptual boundary is memory. WASM memory limits are
separate from the V8 heap limit. A page that talks about `--heap-memory-max`
is not automatically describing the ceiling for preloaded native memory.

This is why `--wasm-default-max-memory` and per-module limits matter. They
control native WASM memory, while the V8 heap limit controls JavaScript heap
allocation.

The SQLite WASM example in the README is a good illustration of the imported
module path: WASM can be preloaded as a reusable runtime capability, then
instantiated from ordinary JavaScript with the imports it needs.

See [Reference](../reference/cli-flags.md) for the exact WASM flags.
