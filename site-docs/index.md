# mcp-v8

**mcp-v8** is a [Model Context Protocol](https://modelcontextprotocol.io) (MCP)
server that executes **JavaScript and TypeScript inside a V8 isolate**. LLM agents
call a single `run_js` tool to run code; the server transpiles TypeScript, enforces
memory and time limits. Heap and filesystem persistence are independent opt-ins:
`--heap-store dir|s3` saves V8 state between calls; `--fs-store dir|s3` saves the
virtual filesystem. Both default to `none`.

Code can be granted carefully scoped host capabilities — network (`fetch`),
filesystem, subprocess, WebAssembly, ES-module imports, and calls to *other* MCP
servers — each gated by **OPA/Rego policies**. The server speaks stdio, Streamable
HTTP, or SSE, can authenticate requests with JWT/JWKS, and can form a **Raft
cluster** that replicates session metadata.

<figure class="docs-video" markdown="span">
  <video controls preload="none" playsinline
         poster="media/zero-trust-agent-sandbox-poster.jpg"
         data-src="media/zero-trust-agent-sandbox.webm">
    <source data-src="media/zero-trust-agent-sandbox.webm" type="video/webm">
    Your browser does not support the video tag.
  </video>
  <figcaption>Inside the zero-trust agent sandbox — a walkthrough of mcp-v8.</figcaption>
</figure>

## Why mcp-v8

- **One tool, unbounded capability.** Instead of exposing dozens of narrow tools, an
  agent writes code. A single `run_js` call can loop, branch, transform data, and
  chain other tools — often using far fewer tokens than equivalent tool-call
  sequences.
- **Durable state.** Heap snapshots let an agent build up state across many turns
  without re-sending context.
- **Secure by default.** Network, filesystem, subprocess, and module imports are all
  *off* until you grant them with an explicit policy.
- **Network transports and operations.** HTTP/SSE with a REST sidecar, async
  execution with pagination, JWKS auth, and Raft-replicated clustering.

## New capabilities to evaluate

- [Schedule work](tutorials/first-schedule.md) with cron/timezones, cancellation,
  and fenced worker admission. The scheduler remains experimental.
- [Create HTTP functions](tutorials/first-http-function.md) and update definitions
  without restarting, through HTTP or MCP management.
- [Choose SQLite or DynamoDB metadata](how-to/metadata-backends.md) while keeping
  [runtime bytes in S3 or files](concepts/runtime-storage.md).
- [Embed the native JavaScript SDK](how-to/native-javascript-sdk.md) and provide
  host tools; Linux x64 releases have glibc and musl/Alpine variants.
- [Validate builds and performance](how-to/development-validation.md) with cached
  compilation, fault replay, and reproducible filesystem workloads.

Read the [upgrade notes](reference/release-upgrade.md) for persistence defaults,
reader-format compatibility, V8 snapshots, and supported deployment boundaries.

## Start here

- [Install](install/overview.md) — get the server running.
- [Releases](https://github.com/r33drichards/mcp-js/releases) — published assets; [install npm packages from a GitHub release](install/github-release-packages.md).
- [Tutorials](tutorials/overview.md) — end-to-end use-case walkthroughs (e.g. SQLite WASM).
- [How-to guides](how-to/overview.md) — task-focused recipes by feature.
- [Concepts](concepts/overview.md) — how and why it works.
- [Reference](reference/overview.md) — flags, tools, APIs.

## Community

Join the [mcp-js Discord community](https://discord.gg/BqjerAUDzC) for questions,
examples, and project discussion.

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

## Experimental scheduling and HTTP handlers

Start with the [implemented feature status](reference/scheduler-http-status.md),
then choose a tutorial, operational guide, reference, or explanation. The scheduler and HTTP management features are merged into `main`. The scheduler
remains experimental; the status page separates implemented behavior from
production-readiness work and future proposals.
