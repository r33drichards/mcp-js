# Concepts

These pages are **explanation** — they clarify how mcp-v8 works and why it is
designed the way it is. They are for understanding, not step-by-step instructions
(see [How-to guides](../how-to/overview.md) for those).

## The big picture

mcp-v8 turns "call a tool" into "run a program." An agent sends JavaScript or
TypeScript to the `run_js` tool; the server runs it in a sandboxed V8 isolate, can
persist the resulting heap, and exposes host capabilities only through explicit,
policy-gated bridges. Everything else below is a facet of that idea.

## Explanations by feature

- [Running JavaScript & TypeScript](js-execution.md) — the V8/`deno_core` execution model.
- [Stateful sessions & heap snapshots](sessions-and-heaps.md) — content-addressed state persistence.
- [Heap storage backends](storage-backends.md) — durability and sharing trade-offs.
- [Asynchronous execution & output](async-execution.md) — the submit→poll lifecycle.
- [Transports: stdio, HTTP, SSE](transports.md) — MCP transports and the REST sidecar.
- [Network access with fetch](fetch.md) — policy gating and server-side secret injection.
- [Filesystem access](filesystem.md) — opt-in, per-operation file I/O.
- [Subprocess execution](subprocess.md) — the process-spawning security model.
- [WebAssembly modules](wasm-modules.md) — embedding host WASM as JS globals.
- [ES module imports](module-imports.md) — why imports are off by default.
- [Calling upstream MCP servers](mcp-client.md) — mcp-v8 as both server and client.
- [Security policies (OPA/Rego)](policies.md) — the unified default-deny policy model.
- [Authentication (JWT/JWKS)](authentication.md) — the token verification trust model.
- [Clustering & replication (Raft)](clustering.md) — consensus and replicated state.

## See also

- [Tutorials](../tutorials/overview.md) — learn by doing.
- [Reference](../reference/overview.md) — the exhaustive facts.
