# mcp-v8 — a JavaScript/TypeScript runtime for AI agents

[![Docs](https://img.shields.io/badge/docs-mkdocs-blue)](https://r33drichards.github.io/mcp-js/)
[![Release](https://img.shields.io/github/v/release/r33drichards/mcp-js)](https://github.com/r33drichards/mcp-js/releases)
[![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0-blue)](./LICENSE)

**mcp-v8** is a [Model Context Protocol](https://modelcontextprotocol.io) server,
written in Rust, that lets an AI agent **run JavaScript and TypeScript in a
sandboxed V8 isolate**. Instead of wiring up dozens of narrow tools, you give the
agent one tool — `run_js` — and it writes code: looping, branching, transforming
data, and calling other tools, often with far fewer tokens than equivalent
tool-call chains.

In its default *stateful* mode the V8 heap is saved as a content-addressed
snapshot, so an agent can build up state across many turns. Host capabilities
(network, filesystem, subprocess, WebAssembly, module imports, and calls to other
MCP servers) are all **off by default** and unlocked only by explicit
[OPA/Rego policies](https://www.openpolicyagent.org/).

## Why mcp-v8

- **One tool, unbounded capability.** The agent runs a program, not a fixed menu of tools.
- **Durable state.** Heap snapshots persist variables and objects across calls.
- **Secure by default.** `fetch`, filesystem, subprocess, and external imports are denied until you grant them via policy.
- **Production-ready.** stdio / Streamable HTTP / SSE transports, a REST sidecar, async execution with pagination, JWKS auth, and Raft-replicated clustering.

## Documentation

Full documentation lives at **<https://r33drichards.github.io/mcp-js/>** (built
from [`site-docs/`](./site-docs)) — tutorials, how-to guides, concept
explanations, and complete reference for the [CLI flags](https://r33drichards.github.io/mcp-js/reference/cli-flags/),
[HTTP API](https://r33drichards.github.io/mcp-js/reference/http-api/), and
[MCP tools](https://r33drichards.github.io/mcp-js/reference/mcp-tools/).

## Quick start

### Install

```bash
# Server
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash

# Optional CLI client
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install-cli.sh | sudo bash
```

Installs to `/usr/local/bin`. Supported platforms: Linux x86_64/arm64 and macOS
Apple Silicon. You can also `nix run github:r33drichards/mcp-js`, use Docker (see
the `docker-compose.*.yml` stacks), or [build from source](#build-from-source).

### Connect an MCP client

```bash
# Claude Code (stdio)
claude mcp add mcp-v8 -- mcp-v8 --directory-path /tmp/mcp-v8-heaps   # stateful
claude mcp add mcp-v8 -- mcp-v8 --stateless                          # stateless
```

For Claude Desktop / Cursor, add to the client's `mcpServers` config:

```json
{ "mcpServers": { "js": { "command": "mcp-v8", "args": ["--stateless"] } } }
```

Then ask the agent: *"Run this JavaScript: `console.log([1,2,3].map(x => x*2))`"*.

### Run over HTTP

```bash
mcp-v8 --stateless --http-port 8080
# MCP endpoint: POST http://localhost:8080/mcp
# REST sidecar: POST http://localhost:8080/api/exec  (JSON body, or a raw-body file upload)
```

`/api/exec` accepts either a JSON body or a raw-body file upload — send the
script as the request body with a non-JSON `Content-Type`
(`curl --data-binary @script.js -H 'Content-Type: application/javascript' .../api/exec`).
The `run_js` MCP tool can also read a script from a path on the server itself
via an optional `file` parameter — off by default, enabled with
`--allow-run-js-file` or a `run_js_file`
[policy](https://r33drichards.github.io/mcp-js/reference/policies/).

See the [Quick Start tutorials](https://r33drichards.github.io/mcp-js/) and the
[transports guide](https://r33drichards.github.io/mcp-js/concepts/transports/) for more.

## Features

- **JavaScript & TypeScript** in an isolated V8 engine (via `deno_core`); TypeScript types are stripped with [SWC](https://swc.rs/) (type removal, not type checking).
- **Async/await & timers** — Promises and the event loop, plus `setTimeout`/`clearTimeout`.
- **Console capture** — `console.log/info/warn/error/debug/trace`, streamed to storage and readable with line- or byte-based pagination.
- **Async execution model** — `run_js` returns an execution ID; poll status and stream output; cancel running work.
- **Content-addressed heap snapshots** — persist/restore V8 state across calls (local FS, S3, or S3 + write-through cache), or run **stateless**.
- **WebAssembly** — the standard `WebAssembly` API, plus pre-loaded modules (`--wasm-module`) exposed as globals and advertised to clients as `runjs__wasm__<name>` stub tools.
- **ES module imports** — optional `npm:`, `jsr:`, and URL imports fetched at runtime (policy-gated).
- **Policy-gated capabilities** — `fetch`, filesystem (`fs`), and subprocess access, each checked against a Rego policy per operation; plus header/OAuth injection for `fetch`.
- **Compose other MCP servers** — connect upstream MCP servers and call them from JS via `mcp.callTool()` / `mcp.listTools()`.
- **Customizable surface** — override the server `instructions` and the `run_js` description (`--instructions`, `--run-js-description`).
- **Auth & clustering** — JWKS-based JWT verification, and optional Raft clustering with replicated session metadata and horizontal scaling.
- **Multiple transports** — stdio, Streamable HTTP (MCP 2025-03-26+), and a legacy HTTP+SSE transport (`--sse-port`, served by a vendored rmcp 0.1.5), with a REST sidecar and OpenAPI spec.
- **Tasks** — native MCP [tasks](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks) (SEP-1319) over Streamable HTTP / stdio: task-enabled clients can run `run_js` as a task (`tasks/get`, `tasks/result`, `tasks/list`, `tasks/cancel`), ideal for long-running calls. (The legacy SSE transport does not offer tasks.)

## What the agent's code can do

These globals are available inside `run_js` (capability globals require a policy):

| Global | Purpose | Gated by |
|--------|---------|----------|
| `console`, `setTimeout` | Output & timers | — |
| `fetch(url, opts?)` | HTTP requests (Fetch API) | `fetch` policy |
| `fs.*` | File I/O (`readFile`, `writeFile`, …) | `filesystem` policy |
| `child_process` / `Deno.Command` | Run subprocesses | `subprocess` policy |
| `import` (`npm:` / `jsr:` / URL) | External ES modules | `--allow-external-modules` + `modules` policy |
| `WebAssembly`, `__wasm_<name>` | Run/instantiate WASM | — |
| `mcp.callTool/listTools/servers` | Call upstream MCP servers | `mcp_tools` policy |

See [Concepts → Security policies](https://r33drichards.github.io/mcp-js/concepts/policies/) for the policy model.

## MCP tools

| Tool | Mode | Description |
|------|------|-------------|
| `run_js` | both | Stateful: queue execution → `{execution_id}`. Stateless: run and return `{output, error?}`. |
| `get_execution` | stateful | Poll status/result of an execution. |
| `get_execution_output` | stateful | Read paginated console output (line or byte). |
| `cancel_execution` | stateful | Terminate a running execution. |
| `list_executions` | stateful | List executions and their status. |
| `list_sessions`, `list_session_snapshots` | stateful | Browse named sessions and history. |
| `get_heap_tags`, `set_heap_tags`, `delete_heap_tags`, `query_heaps_by_tags` | stateful | Tag and search heap snapshots. |

Full parameters: [MCP tools reference](https://r33drichards.github.io/mcp-js/reference/mcp-tools/).

### Long-running calls as tasks

The server natively implements the MCP **tasks** utility (spec `2025-11-25` /
SEP-1319) via rmcp, over both the Streamable HTTP and stdio transports. The
`initialize` result advertises a `tasks` capability, and a client may run the
task-augmentable `run_js` tool as a task by adding a `task` object to the
request params:

```jsonc
// → returns immediately with a task instead of blocking
{ "method": "tools/call",
  "params": { "name": "run_js", "arguments": { "code": "…" }, "task": { "ttl": 300000 } } }
```

The client then polls `tasks/get`, fetches the eventual tool result with
`tasks/result` (which returns exactly what the call would have returned),
enumerates work with `tasks/list`, and stops a run with `tasks/cancel`. A
`tools/call` without a `task` field is unaffected.

## Configuration

`mcp-v8` is configured entirely through CLI flags — storage backend, transport,
execution limits, policies, fetch-header injection, WASM modules, clustering, JWKS
auth, and the prompt/tool-description overrides. The complete, always-current list
is the generated [CLI flags reference](https://r33drichards.github.io/mcp-js/reference/cli-flags/).

```bash
mcp-v8 --help            # all flags
mcp-v8 --print-openapi   # print the REST OpenAPI spec
```

## CLI client (`mcp-v8-cli`)

A fully-typed client for the REST API, generated from the OpenAPI spec via
[progenitor](https://github.com/oxidecomputer/progenitor):

```bash
mcp-v8 --stateless --http-port 3000 &
mcp-v8-cli exec "console.log('hello'); 1 + 1"
mcp-v8-cli exec --file ./script.js                # run a local file (uploaded as the code)
mcp-v8-cli executions get <execution_id>
mcp-v8-cli executions output <execution_id>
export MCP_V8_URL=https://my-server.example.com   # point at a remote server
```

## Rust client (`mcp-v8-client`)

```toml
[dependencies]
mcp-v8-client = { git = "https://github.com/r33drichards/mcp-js" }
```

```rust
use mcp_v8_client::Client;
let client = Client::new("http://localhost:3000");
let body = mcp_v8_client::types::ExecRequest {
    code: "1 + 1".to_string(),
    heap: None, session: None,
    heap_memory_max_mb: None, execution_timeout_secs: None, tags: None,
};
let resp = client.exec_handler(&body).await?;
println!("execution_id: {}", resp.into_inner().execution_id);
```

## TypeScript client (`@mcp-v8/client`)

A fully-typed TypeScript client, also generated from the OpenAPI spec
(`openapi-typescript` types + the `openapi-fetch` runtime). Lives in
[`clients/typescript`](clients/typescript/README.md).

```ts
import { createMcpV8Client } from "@mcp-v8/client";

const client = createMcpV8Client("http://localhost:3000");
const { status, output, heap } = await client.runJs("console.log('hi'); 1 + 1");
```

Regenerate types after API changes with `npm run generate` (reads `openapi.json`).

## Build from source

The repo is a Nix flake (it wires up the prefetched V8 archive so the build stays
offline-friendly):

```bash
nix build github:r33drichards/mcp-js   # → ./result/bin/server
# or for development:
nix develop      # then: cargo build -p server
```

A plain `cargo build --release` inside `server/` also works if your toolchain can
build `deno_core`/V8.

## Limitations

- **`setInterval` is not available** — use a loop with awaited `setTimeout`.
- **No DOM or browser APIs** — there is no `window`/`document`.
- **TypeScript is type-stripped, not type-checked** — invalid types are removed, not reported. JSX/TSX is not supported (it parses to a clear error).

## License

[GNU AGPL-3.0](./LICENSE).

<!-- load-test-report -->
# MCP-V8 Load Test Benchmark Report v0.1.0

Comparison of single-node vs 3-node cluster at various request rates.

## Results

ran on railway gha runners on [pr](https://github.com/r33drichards/mcp-js/pull/36#issuecomment-3946074130)

| Topology | Target Rate | Actual Iter/s | HTTP Req/s | Exec Avg (ms) | Exec p95 (ms) | Exec p99 (ms) | Success % | Dropped | Max VUs |
|----------|-------------|---------------|------------|----------------|----------------|----------------|-----------|---------|---------|
| cluster-stateful | 100/s | 99.5 | 99.5 | 44.9 | 196.88 | 416.99 | 100% | 31 | 41 |
| cluster-stateful | 200/s | 199.6 | 199.6 | 23.22 | 79.32 | 131.13 | 100% | 13 | 33 |
| cluster-stateless | 1000/s | 999.9 | 999.9 | 3.82 | 7.72 | 13.09 | 100% | 0 | 100 |
| cluster-stateless | 100/s | 100 | 100 | 3.67 | 5.65 | 8.03 | 100% | 0 | 10 |
| cluster-stateless | 200/s | 200 | 200 | 3.56 | 5.9 | 8.61 | 100% | 0 | 20 |
| cluster-stateless | 500/s | 500 | 500 | 3.42 | 5.85 | 9.2 | 100% | 0 | 50 |
| single-stateful | 100/s | 99.1 | 99.1 | 215.12 | 362.5 | 376.6 | 100% | 32 | 42 |
| single-stateful | 200/s | 97.8 | 97.8 | 1948.82 | 2212.55 | 2960.96 | 100% | 5939 | 200 |
| single-stateless | 1000/s | 977.1 | 977.1 | 60.98 | 482.98 | 602.38 | 100% | 843 | 561 |
| single-stateless | 100/s | 100 | 100 | 3.71 | 5.73 | 8.73 | 100% | 0 | 10 |
| single-stateless | 200/s | 200 | 200 | 3.61 | 5.43 | 7.74 | 100% | 0 | 20 |
| single-stateless | 500/s | 500 | 500 | 4.67 | 8.49 | 27.98 | 100% | 0 | 50 |

## P95 Latency

| Topology | Rate | P95 (ms) | |
|----------|------|----------|-|
| cluster-stateful | 100/s | 196.88 | `█████████████████████` |
| cluster-stateful | 200/s | 79.32 | `█████████████████` |
| cluster-stateless | 100/s | 5.65 | `███████` |
| cluster-stateless | 200/s | 5.9 | `███████` |
| cluster-stateless | 500/s | 5.85 | `███████` |
| cluster-stateless | 1000/s | 7.72 | `████████` |
| single-stateful | 100/s | 362.5 | `███████████████████████` |
| single-stateful | 200/s | 2212.55 | `██████████████████████████████` |
| single-stateless | 100/s | 5.73 | `███████` |
| single-stateless | 200/s | 5.43 | `██████` |
| single-stateless | 500/s | 8.49 | `████████` |
| single-stateless | 1000/s | 482.98 | `████████████████████████` |

## Notes

- **Target Rate**: The configured constant-arrival-rate (requests/second k6 attempts)
- **Actual Iter/s**: Achieved iterations per second (each iteration = 1 POST /api/exec)
- **HTTP Req/s**: Total HTTP requests per second (1 per iteration)
- **Dropped**: Iterations k6 couldn't schedule because VUs were exhausted (indicates server saturation)
- **Topology**: `single` = 1 MCP-V8 node; `cluster` = 3 MCP-V8 nodes with Raft
