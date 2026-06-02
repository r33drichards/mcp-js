# The V8 JavaScript Runtime

mcp-v8 executes JavaScript and TypeScript inside Google's V8 engine, the same engine that powers Chrome and Node.js. Rather than embedding V8 directly, it uses `deno_core`, the runtime foundation of the Deno project. This gives mcp-v8 a mature event loop, ES module support, and a clean Rust-to-JavaScript bridge -- without bringing in Deno's full standard library or permission system.

## How Code Enters V8

When mcp-v8 receives code (via an MCP tool call or HTTP request), the code passes through several stages before V8 sees it:

1. **TypeScript type stripping** -- The code is parsed by SWC (a Rust-based JavaScript/TypeScript compiler). SWC strips all TypeScript type annotations -- interfaces, type aliases, generics, enums, and type-only imports -- producing plain JavaScript. This is type _removal_, not type _checking_. If the code is already plain JavaScript, SWC parses it and emits it unchanged. This step runs synchronously on the Rust side before V8 is involved.

2. **ES module loading** -- All code is executed as an ES module, not as a classic script. This means `import` and `export` declarations work at the top level, and top-level `await` is supported. deno_core's module loader is responsible for resolving and fetching any imported modules.

3. **V8 execution** -- The prepared JavaScript is loaded into a `JsRuntime` (stateless mode) or `JsRuntimeForSnapshot` (stateful mode) via `load_side_es_module_from_code`. The module is evaluated, and the deno_core event loop is driven to completion using `run_event_loop`.

## The deno_core Event Loop

deno_core provides a Tokio-integrated event loop that drives asynchronous operations inside V8. When JavaScript code calls an async function -- like `fetch()`, `fs.readFile()`, or `mcp.callTool()` -- the call is dispatched to a Rust "op" (operation) that returns a future. The event loop polls these futures alongside V8's microtask queue, resolving Promises on the JavaScript side as the Rust futures complete.

The event loop runs until all pending operations are finished: all Promises are settled, all timers have fired, and no pending module loads remain. This means that top-level `await` works naturally:

```js
const resp = await fetch("https://api.example.com/data");
const data = await resp.json();
console.log(data.length);
```

Each line awaits its result before proceeding, and the event loop keeps running until `console.log` has executed and no further work remains.

## Async/Await Resolution

Async operations in mcp-v8 follow the standard JavaScript Promise model, but the actual I/O happens in Rust. The flow is:

1. JavaScript calls an async op (e.g., `Deno.core.ops.op_fetch(...)`).
2. deno_core converts the arguments and dispatches the call to a Rust async function.
3. The Rust function returns a future. deno_core registers this future in its internal `FuturesUnorderedDriver`.
4. The event loop polls Rust futures and V8's microtask queue in interleaved fashion.
5. When a Rust future completes, its result is delivered back to JavaScript as a resolved (or rejected) Promise.

This architecture means that `await` in mcp-v8 is truly non-blocking. While one `fetch()` is waiting for a network response, timers can fire, other Promises can resolve, and the V8 microtask queue can process `.then()` callbacks.

## TypeScript Type Stripping via SWC

SWC is used purely for type erasure. The transformation pipeline is:

1. **Parse** -- SWC's TypeScript parser (with TSX support enabled) produces an AST.
2. **Resolve** -- Identifier scope analysis determines which names are local vs. imported.
3. **Strip** -- The TypeScript strip transform removes all type-level constructs: type annotations, interfaces, type aliases, enums (converted to plain objects), abstract classes (converted to regular classes), and type-only imports/exports.
4. **Hygiene** -- Identifiers that share names but belong to different scopes are disambiguated.
5. **Fixer** -- Missing parentheses are inserted to preserve correct precedence after AST transforms.
6. **Emit** -- The cleaned AST is serialized back to JavaScript source text.

Because this is type removal rather than full compilation, TypeScript-specific runtime features like `const enum` with complex initializers or `namespace` merging may not work as they would in `tsc`. The focus is on supporting the common case: TypeScript code with type annotations that can be mechanically stripped to produce valid JavaScript.

## ES Module Execution

All user code runs as ES modules. This design choice has several implications:

- `import` declarations are supported at the top level. External modules (npm, JSR, URL) are fetched at load time by the module loader.
- `export` declarations are permitted (though the exported values are not captured by mcp-v8).
- Top-level `await` is supported. The event loop drives async operations to completion.
- Code runs in strict mode by default (as required by the ES module specification).
- Each execution gets a unique synthetic module URL (`file:///main_0.js`, `file:///main_1.js`, ...) generated from a global atomic counter. This prevents collisions when restoring from a snapshot that already has registered modules.

## Sandbox Setup

Before user code runs, several setup steps harden the V8 environment:

1. **Console wrapper** -- `globalThis.console` is replaced with a custom implementation that routes `console.log`, `console.info`, `console.warn`, and `console.error` through a Rust op that writes to the console output WAL.
2. **Dangerous op neutralization** -- Built-in deno_core ops like `op_panic` (which would call Rust `panic!()`) and `op_print` (which would write directly to stdout, corrupting the JSON-RPC stream) are replaced with safe JavaScript alternatives.
3. **Optional capabilities** -- `fetch()`, `fs`, `mcp`, `setTimeout`/`clearTimeout`, and subprocess APIs are injected based on server configuration.
4. **Runtime hardening** -- `Deno.core.ops` is frozen to prevent interception. `__bootstrap` (which exposes event loop hooks and primordials) is deleted. `SharedArrayBuffer` and `Atomics` are removed as a defense-in-depth measure against Spectre-style timing attacks.

After hardening, user code cannot access internal deno_core machinery, cannot write directly to stdout/stderr, and cannot reverse the sandbox lockdown.
