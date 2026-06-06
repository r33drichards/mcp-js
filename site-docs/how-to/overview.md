# How-to guides

These are **task-oriented** recipes for getting specific things done. They assume you
already have the server installed and running. For a guided introduction, start with
the [Tutorials](../tutorials/overview.md) tutorials instead.

## Guides by feature

- [Running JavaScript & TypeScript](js-execution.md) — run TS, capture output, set per-call limits.
- [Stateful sessions & heap snapshots](sessions-and-heaps.md) — resume heaps, tag and query snapshots, name sessions.
- [Heap storage backends](storage-backends.md) — use local FS, S3, S3 + cache, or stateless.
- [Asynchronous execution & output](async-execution.md) — submit/poll, paginate output, cancel runs.
- [Transports: stdio, HTTP, SSE](transports.md) — configure each transport for clients.
- [Network access with fetch](fetch.md) — enable fetch, inject static and OAuth headers.
- [Filesystem access](filesystem.md) — enable and restrict file I/O.
- [Subprocess execution](subprocess.md) — enable and restrict command execution.
- [WebAssembly modules](wasm-modules.md) — load modules and set memory caps.
- [ES module imports](module-imports.md) — enable and gate external imports.
- [Calling upstream MCP servers](mcp-client.md) — connect stdio/SSE servers, control stubs.
- [Security policies (OPA/Rego)](policies.md) — gate each capability with OPA or Rego.
- [Authentication (JWT/JWKS)](authentication.md) — enable JWKS verification, use Keycloak.
- [Clustering & replication (Raft)](clustering.md) — configure nodes, join, load-balance.

## See also

- [Concepts](../concepts/overview.md) — the reasoning behind these tasks.
- [Reference](../reference/overview.md) — every flag, tool, and API field.
