# WebAssembly modules — concepts

An explanation of how mcp-v8 loads WebAssembly modules, exposes them to
JavaScript, and enforces memory budgets.

## Why embed WASM in an MCP server

V8 can execute WebAssembly directly, and compiled C or Rust libraries — SQLite,
compression codecs, numeric kernels — can be packaged as `.wasm` files and loaded
once at server startup. This lets LLM-generated scripts call into well-tested,
binary-safe native code without requiring filesystem access or subprocess
execution. The entire execution stays within the V8 isolate.

## The module injection pipeline

mcp-v8 reads `.wasm` bytes from disk at startup (flag parsing), then injects each
module into the V8 isolate before user code runs. The diagram below shows the full
path from flag to JavaScript global.

```mermaid
flowchart TD
    A["--wasm-module name=path<br/>--wasm-config file.json"] --> B[Read .wasm bytes from disk at startup]
    B --> C[validate_wasm_name<br/>first char: letter / _ / $<br/>rest: alphanumeric / _ / $]
    C --> D["Per execution: inject_wasm_modules()"]
    D --> E[wasmparser: validate binary structure]
    E --> F["Check declared initial memory ≤ cap ÷ 65 536 pages<br/>Check declared initial table ≤ cap ÷ 8 elements"]
    F --> G{Module has imports?}
    G -- "No" --> H["Compile + auto-instantiate<br/>new WebAssembly.Instance(module, {})"]
    G -- "Yes" --> I["Compile only<br/>WebAssembly.Module object"]
    H --> J["globalThis[name] = instance.exports<br/>Call exports directly in JS"]
    I --> K["globalThis['__wasm_' + name] = compiled module<br/>User code must provide imports and instantiate"]
```

## Auto-instantiation vs. manual instantiation

Before compiling, the engine scans the module's import section:

**No imports** — the module is compiled and immediately instantiated with an empty
import object. The `exports` property of the resulting `WebAssembly.Instance` is
assigned to `globalThis[name]`. JavaScript code accesses exports as plain
properties:

```js
const result = math.add(1, 2);
```

**Has imports** — the module cannot be instantiated without caller-supplied
bindings. The compiled `WebAssembly.Module` object is instead assigned to
`globalThis["__wasm_" + name]`. JavaScript code is responsible for calling
`new WebAssembly.Instance(__wasm_name, importObject)` with a suitable import
object. This is the pattern used by the SQLite example, which needs a set of WASI
function stubs.

The separation is intentional: forcing the user to construct the import object
makes the binding explicit and avoids silent mismatches between what the module
expects and what is provided.

## The memory cap model

The cap controlled by `--wasm-default-max-memory` and the per-module `:max_memory`
flag suffix is a **structural pre-execution check**, not a runtime allocator limit.

Before compiling a module into V8, mcp-v8 parses the binary with `wasmparser` and
inspects the `MemorySection`, `TableSection`, and `ImportSection`:

- Declared initial memory pages (`mem.initial`) must not exceed
  `cap_bytes / 65 536`.
- Declared initial table elements must not exceed `cap_bytes / 8`.
- Both the module's own sections and any imported memory or table are checked
  against the same budget.

If the check fails, the execution returns an error before any V8 code runs. If the
check passes, the module is compiled and no further runtime limit is installed on
the WebAssembly linear memory. Modules compiled with `ALLOW_MEMORY_GROWTH=1` can
grow beyond their declared initial size at runtime; the cap does not prevent this.

The default cap is 16 MiB (256 pages). The SQLite example builds with
`INITIAL_MEMORY=16777216` (exactly 256 pages), which exactly meets the default
cap — 256 pages declared against a 256-page budget, passing the `initial > max`
check with zero pages to spare.

## Heap snapshots and persistence

WASM cannot be combined with V8 heap persistence. Heap snapshots require a V8
`SnapshotCreator` isolate, which disables WebAssembly. The server rejects
`--wasm-module`/`--wasm-config` with any `--heap-store` other than `none`;
the embedded engine likewise rejects `wasm_modules` with `heap_store`.

With heap persistence disabled, each execution receives freshly compiled
modules from the module bytes loaded at server startup. Module instances and
WASM linear memory do not survive between executions through a `heap` key.

[Filesystem persistence](../how-to/fs-snapshots.md) is independent and can be
used with WASM. It persists explicitly written files, not module instances or
linear memory. A library needs an explicit storage integration to use it; the
bundled in-memory SQLite example does not provide one.

## Discovery: WASM stub tools

A pre-loaded module is injected as a JavaScript global, but a downstream MCP
client has no way to know it exists from the protocol alone. To close that gap,
mcp-v8 advertises each loaded module on its own MCP surface as a **stub tool**
named `runjs__wasm__<name>` (the prefix is configurable). This mirrors the
[upstream MCP tool stubs](../concepts/mcp-client.md): the module shows up in
`tools/list` and tool search, and its description explains the `__wasm_<name>`
global, the module's exports, and whether an imports object is required.

The stub is deliberately **not** an executable proxy. WebAssembly exports carry no
MCP-level schema, so the agent cannot call them directly over the protocol;
instead, calling the stub returns instructional text telling the agent to drive
the module from JavaScript through `run_js`. Discovery happens at the MCP layer;
execution stays in the V8 isolate.

Stubbing is on by default whenever at least one module is loaded. `--wasm-stubs
false` hides the stubs, `--wasm-stub-prefix` changes the `runjs__` prefix, and a
per-module human description can be set with `--wasm-stub-description name=text`
or the `description` field in `--wasm-config`.

## Name validation

Module names must satisfy a restricted identifier grammar:

| Position | Allowed characters |
|----------|--------------------|
| First | ASCII letter (`a`–`z`, `A`–`Z`), underscore `_`, dollar sign `$` |
| Rest | ASCII alphanumeric, underscore, dollar sign |

Names that fail this check are rejected at startup before any module file is read.
The restriction ensures the name is directly usable as a JavaScript global
identifier without quoting or escaping.

## See also

- [How to use WebAssembly modules](../how-to/wasm-modules.md)
- [Running JavaScript & TypeScript — concepts](../concepts/js-execution.md)
- [ES module imports — concepts](../concepts/module-imports.md)
