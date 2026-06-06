# mcp-v8

**mcp-v8** is a [Model Context Protocol](https://modelcontextprotocol.io) (MCP)
server that executes **JavaScript and TypeScript inside a V8 isolate**. LLM agents
call a single `run_js` tool to run code; the server transpiles TypeScript, enforces
memory and time limits, and — in its default *stateful* mode — persists the V8 heap
as a content-addressed snapshot so state survives across calls.

Code can be granted carefully scoped host capabilities — network (`fetch`),
filesystem, subprocess, WebAssembly, ES-module imports, and calls to *other* MCP
servers — each gated by **OPA/Rego policies**. The server speaks stdio, Streamable
HTTP, or SSE, can authenticate requests with JWT/JWKS, and can form a **Raft
cluster** that replicates session metadata.

## Why mcp-v8

- **One tool, unbounded capability.** Instead of exposing dozens of narrow tools, an
  agent writes code. A single `run_js` call can loop, branch, transform data, and
  chain other tools — often using far fewer tokens than equivalent tool-call
  sequences.
- **Durable state.** Heap snapshots let an agent build up state across many turns
  without re-sending context.
- **Secure by default.** Network, filesystem, subprocess, and module imports are all
  *off* until you grant them with an explicit policy.
- **Production-ready transports & ops.** HTTP/SSE with a REST sidecar, async
  execution with pagination, JWKS auth, and Raft-replicated clustering.

## Start here

- [Install](install/overview.md) — get the server running.
- [Tutorials](tutorials/overview.md) — end-to-end use-case walkthroughs (e.g. SQLite WASM).
- [How-to guides](how-to/overview.md) — task-focused recipes by feature.
- [Concepts](concepts/overview.md) — how and why it works.
- [Reference](reference/overview.md) — flags, tools, APIs.

## Capabilities at a glance

| Area | What it does |
|------|--------------|
| [Running JavaScript & TypeScript](concepts/js-execution.md) | Execute JS/TS in a V8 isolate with memory & time limits |
| [Stateful sessions & heap snapshots](concepts/sessions-and-heaps.md) | Persist & restore V8 heap state across calls |
| [Heap storage backends](concepts/storage-backends.md) | Local FS, S3, S3 + write-through cache, or stateless |
| [Asynchronous execution & output](concepts/async-execution.md) | Submit, poll, paginate output, cancel |
| [Transports: stdio, HTTP, SSE](concepts/transports.md) | Connect MCP clients three ways, plus a REST sidecar |
| [Network access with fetch](concepts/fetch.md) | Policy-gated `fetch()` with server-side header/OAuth injection |
| [Filesystem access](concepts/filesystem.md) | Policy-gated file I/O from JS |
| [Subprocess execution](concepts/subprocess.md) | Policy-gated process spawning from JS |
| [WebAssembly modules](concepts/wasm-modules.md) | Embed host WASM modules as JS globals |
| [ES module imports](concepts/module-imports.md) | Optional, policy-gated external imports |
| [Calling upstream MCP servers](concepts/mcp-client.md) | Compose other MCP servers from JS via `mcp.*` |
| [Security policies (OPA/Rego)](concepts/policies.md) | Gate every capability with OPA or embedded Rego |
| [Authentication (JWT/JWKS)](concepts/authentication.md) | Verify request tokens against a JWKS |
| [Clustering & replication (Raft)](concepts/clustering.md) | Replicate session metadata across nodes |
