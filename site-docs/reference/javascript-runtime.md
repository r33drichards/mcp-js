# JavaScript Runtime Reference

## Engine

mcp-v8 executes JavaScript and TypeScript using Google's V8 engine via `deno_core`. All code runs as ES modules with top-level `await` support.

## TypeScript Handling

TypeScript is processed by SWC (Speedy Web Compiler) before V8 execution. SWC performs **type removal only** -- no type checking. The pipeline:

1. Parse source as TypeScript (TSX enabled) using `swc_core::ecma::parser`
2. Run identifier scope analysis (`resolver`)
3. Strip type annotations (`strip`)
4. Fix identifier hygiene (`hygiene`)
5. Ensure correct parenthesization (`fixer`)
6. Emit plain JavaScript

Plain JavaScript passes through unchanged.

## Supported ES Features

All features supported by the bundled V8 engine (V8 145 via deno_core 0.381):

- ES2023+ syntax (all modern features)
- ES modules (`import`/`export`, top-level `await`)
- `async`/`await`
- Promises
- Classes, private fields, static blocks
- Optional chaining, nullish coalescing
- Destructuring, spread, rest parameters
- `Map`, `Set`, `WeakMap`, `WeakSet`
- `Symbol`, `Proxy`, `Reflect`
- Typed arrays (`Uint8Array`, `Float64Array`, etc.)
- `BigInt`
- Regular expressions (all V8-supported flags)
- `TextEncoder`, `TextDecoder`
- `JSON`, `Math`, `Date`, `URL`, `URLSearchParams`
- `structuredClone`
- `atob`, `btoa`

## Available Globals

| Global | Availability | Description |
|--------|-------------|-------------|
| `console` | Always | `console.log`, `console.info`, `console.warn`, `console.error` |
| `setTimeout` | Always | Schedule callback after delay (ms). Returns timer ID. |
| `clearTimeout` | Always | Cancel a pending `setTimeout` by ID. |
| `fetch` | When `--policies-json` includes `fetch` section | Web Fetch API (policy-gated) |
| `fs` | When `--policies-json` includes `filesystem` section | Node.js-compatible filesystem API (policy-gated) |
| `mcp` | When `--mcp-server` or `--mcp-config` is set | MCP tool calling API |
| `WebAssembly` | Always | Standard WebAssembly API |
| `Deno.Command` | When `--policies-json` includes `subprocess` section | Deno subprocess API (policy-gated) |
| `child_process` | When `--policies-json` includes `subprocess` section | Node.js-compatible `exec()` (policy-gated) |
| WASM globals | When `--wasm-module` or `--wasm-config` is set | Auto-instantiated modules as `<name>`, compiled modules as `__wasm_<name>` |

## Limitations

| Feature | Status |
|---------|--------|
| `setInterval` | Not available |
| DOM APIs (`document`, `window`, `navigator`) | Not available |
| `process.env` | Not available (environment variables are not exposed) |
| Node.js built-in modules (`path`, `os`, `crypto`, etc.) | Not available (except `fs` and `child_process` when policy-gated) |
| `SharedArrayBuffer` | Removed during sandbox hardening |
| `eval()` | Available but subject to V8 sandbox |
| `Function()` constructor | Available but subject to V8 sandbox |
| `Deno.core` | Frozen and non-configurable after sandbox hardening |
| `import.meta` | Available (standard ES module metadata) |

## Sandbox Hardening

Before user code executes, the runtime applies these hardening steps:

1. `Deno.core.ops` is frozen (Object.freeze)
2. Dangerous built-in ops (`op_panic`, `print`) are replaced with safe alternatives
3. `SharedArrayBuffer` is removed from the global scope
4. `__bootstrap` is removed
5. Introspection APIs are neutralized

## Execution Model

All code runs as ES modules via `runtime.load_side_es_module_from_code()`. This means:

- `import` declarations work
- `export` statements are valid
- Top-level `await` is supported
- Strict mode is always active

## Memory Management

- V8 heap size is bounded by `--heap-memory-max` (default 8 MB, minimum 8 MB)
- ArrayBuffer allocations use a bounded allocator outside the V8 heap, limited to the same budget
- Near-heap-limit callback terminates execution gracefully before V8 aborts
- WASM linear memory is allocated as native memory outside the V8 heap, bounded separately by `--wasm-default-max-memory`
