# mcp-v8 — a JavaScript/TypeScript runtime for AI agents

[![Docs](https://img.shields.io/badge/docs-mkdocs-blue)](https://r33drichards.github.io/mcp-js/)
[![Release](https://img.shields.io/github/v/release/r33drichards/mcp-js)](https://github.com/r33drichards/mcp-js/releases)
[![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0-blue)](./LICENSE)

[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/deploy/mcp-js?referralCode=cj5P6Z&utm_medium=integration&utm_source=template&utm_campaign=generic)


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
- **Kernel-enforced confinement.** Opt-in [`--sandbox-manifest`](https://r33drichards.github.io/mcp-js/concepts/os-sandbox/) confines the whole process with a [nono](https://github.com/nolabs-ai/nono) capability manifest (Landlock on Linux, Seatbelt on macOS) as defense in depth beneath the policy layer.
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

Prefer a hosted server? [Deploy on Railway](./RAILWAY.md) — the repo's
`railway.json` configures the build, healthcheck, and restart policy, and
[RAILWAY.md](./RAILWAY.md) walks through the volume, variables, and one-click
template setup.

### Connect an MCP client

```bash
# Claude Code (stdio)
claude mcp add mcp-v8 -- mcp-v8 --heap-store dir --heap-dir /tmp/mcp-v8-heaps  # stateful
claude mcp add mcp-v8 -- mcp-v8                                                # stateless (default)
```

For Claude Desktop / Cursor, add to the client's `mcpServers` config:

```json
{ "mcpServers": { "js": { "command": "mcp-v8", "args": [] } } }
```

Then ask the agent: *"Run this JavaScript: `console.log([1,2,3].map(x => x*2))`"*.

### Run over HTTP

```bash
mcp-v8 --http-port 8080
# MCP endpoint: POST http://localhost:8080/mcp
# REST sidecar: POST http://localhost:8080/api/exec  (JSON body, or a raw-body file upload)
```

`/api/exec` accepts either a JSON body or a raw-body file upload — send the
script as the request body with a non-JSON `Content-Type`
(`curl --data-binary @script.js -H 'Content-Type: application/javascript' .../api/exec`).
The `run_js` MCP tool can also read a script from a path on the server itself
via an optional `file` parameter — off by default, enabled with
`--allow-run-js-file` or a `run_js_file`
[policy](https://r33drichards.github.io/mcp-js/concepts/policies/).

See the [Quick Start tutorials](https://r33drichards.github.io/mcp-js/) and the
[transports guide](https://r33drichards.github.io/mcp-js/concepts/transports/) for more.

### Configure with a single file

Every flag can also live in one TOML or JSON file passed via `--config` (or
`MCP_V8_CONFIG`), including structured sections that replace the separate
WASM / MCP-server / fetch-header / policy JSON files:

```toml
# server.toml — run with: mcp-v8 --config server.toml
http_port = 8080
heap_store = "dir"
heap_dir = "/var/lib/mcp-v8/heaps"

[policies.fetch]
policies = [{ url = "file:///etc/mcp-v8/fetch.rego" }]
```

Precedence is CLI flag > `MCP_V8_*` env var > config file > default. See the
[configuration file reference](https://r33drichards.github.io/mcp-js/reference/config-file/).

## Features

- **JavaScript & TypeScript** in an isolated V8 engine (via `deno_core`); TypeScript types are stripped with [SWC](https://swc.rs/) (type removal, not type checking).
- **Async/await & timers** — Promises and the event loop, plus `setTimeout`/`clearTimeout`.
- **Console capture** — `console.log/info/warn/error/debug/trace`, streamed to storage and readable with line- or byte-based pagination.
- **Async execution model** — `run_js` returns an execution ID; poll status and stream output; cancel running work.
- **Content-addressed heap snapshots** — persist/restore V8 state across calls (local FS, S3, or S3 + write-through cache), or run **stateless**.
- **WebAssembly** — the standard `WebAssembly` API, plus pre-loaded modules (`--wasm-module`) exposed as globals and advertised to clients as `runjs__wasm__<name>` stub tools. Requires stateless mode: heap persistence uses a V8 SnapshotCreator isolate that disables WASM entirely.
- **ES module imports** — optional `npm:`, `jsr:`, and URL imports fetched at runtime (policy-gated).
- **Policy-gated capabilities** — `fetch`, filesystem (`fs`), and subprocess access, each checked against a Rego policy per operation; plus header/OAuth injection for `fetch`.
- **Compose other MCP servers** — connect upstream MCP servers and call them from JS via `mcp.callTool()` / `mcp.listTools()`.
- **Customizable surface** — override the server `instructions` and the `run_js` description (`--instructions`, `--run-js-description`).
- **Single-file configuration** — one TOML/JSON `--config` file can set every flag (precedence: CLI flag > env var > config file > default).
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

### Browser OAuth for upstream MCP servers

Use `--mcp-config` to connect to a protected Streamable HTTP MCP server with
the OAuth 2.1 authorization-code flow. This setting is JSON-only; it is not
available through the compact `--mcp-server` syntax.

```json
[
  {
    "name": "protected",
    "transport": "http",
    "url": "https://mcp.example.com/mcp",
    "auth": {
      "type": "oauth_browser",
      "scope": ["tools.read", "tools.call"],
      "client_id": "optional-registered-client-id",
      "client_secret": "optional-client-secret",
      "redirect_port": 48123,
      "token_cache": "/home/alice/.cache/mcp-js/oauth-protected.json"
    }
  }
]
```

All fields inside `auth` other than `type` are optional. Without `client_id`,
the server uses OAuth dynamic client registration; otherwise it uses the
provided registered client. `scope` is an array of requested scopes.

Protected-resource, authorization, token, and dynamic-registration endpoints
require HTTPS unless they are loopback endpoints. This permits local OAuth test
servers while refusing plaintext remote authorization infrastructure.

On first use, or when no usable cached credentials exist, `mcp-v8` starts a
loopback callback listener and prints the authorization URL. It then attempts
to open that URL locally. For a headless host, copy the printed URL into a
browser on another machine, complete authorization, and make sure that browser
can reach the host's `http://localhost:<port>/callback` URL (for example,
through an SSH port forward). Authorization waits up to five minutes. A
callback with the wrong OAuth `state` is ignored; an authorization denial or
timeout fails the connection.

When `redirect_port` is omitted, the listener selects an available local port.
Set it only when an identity provider requires a registered callback port. The
callback always binds to loopback and uses the resulting
`http://localhost:<port>/callback` redirect URI.

Credentials are cached at `token_cache`, or by default at
`${XDG_CACHE_HOME:-$HOME/.cache}/mcp-js/oauth-<server-name>.json` (falling back
to the system temporary directory when neither cache location is available).
The cache is bound to the MCP URL, scopes, and client configuration. OAuth
credentials are resolved on each initial connection or reconnect. A valid
refresh token renews an expired access token during that resolution without
opening a browser; interactive authorization happens only when cached
credentials cannot provide a token. An established transport keeps its bearer
token until reconnect or invalidity. It does not refresh an already-established
transport proactively.

Treat the cache as a secret: it can contain refresh tokens. On Unix, mcp-v8
writes it with mode `0600` and rejects symlinks, non-regular files, files owned
by another user, and group/world-readable files. Unsafe, wrong-owner, or
symlinked cache files are never consumed. They are treated as unusable, so
reauthorization starts and successful authorization may replace the cache
securely. Do not commit, share, or edit it. To revoke local access, revoke the
grant at the authorization server and delete the configured cache file; the
next connection starts authorization again.

## CLI client (`mcp-v8-cli`)

A fully-typed client for the REST API, generated from the OpenAPI spec via
[progenitor](https://github.com/oxidecomputer/progenitor):

```bash
mcp-v8 --http-port 3000 &
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
# MCP-V8 Load Test Benchmark Report v0.18.1

Comparison of single-node vs 3-node cluster at various request rates.

## Results

| Topology | Target Rate | Actual Iter/s | HTTP Req/s | Exec Avg (ms) | Exec p95 (ms) | Exec p99 (ms) | Success % | Dropped | Max VUs |
|----------|-------------|---------------|------------|----------------|----------------|----------------|-----------|---------|---------|
| cluster-stateful | 100/s | 99.9 | 301.3 | 52.42 | 53.46 | 102.44 | 100% | 1 | 11 |
| cluster-stateful | 200/s | 134.5 | 3855.4 | 1359.67 | 4062.32 | 4119.07 | 100% | 3450 | 200 |
| cluster-stateless | 1000/s | 602.2 | 1609.8 | 1377.83 | 5443.24 | 8890.83 | 99.9% | 22267 | 1000 |
| cluster-stateless | 100/s | 99.9 | 299.8 | 51.49 | 52.6 | 53.34 | 100% | 0 | 10 |
| cluster-stateless | 200/s | 199.8 | 599.5 | 51.92 | 53.43 | 55.14 | 100% | 0 | 20 |
| cluster-stateless | 500/s | 499.6 | 1498.5 | 53.2 | 56.33 | 63.75 | 100% | 0 | 50 |
| single-stateful | 100/s | 51.3 | 1970.1 | 1849.35 | 2081.54 | 2103.18 | 100% | 2823 | 100 |
| single-stateful | 200/s | 46.6 | 3828.4 | 4074.71 | 4628.01 | 4673.8 | 100% | 9020 | 200 |
| single-stateless | 1000/s | 115.4 | 430.8 | 8174.65 | 13854.8 | 16506.74 | 90% | 52654 | 1000 |
| single-stateless | 100/s | 99.9 | 299.6 | 53.31 | 55.96 | 59.52 | 100% | 4 | 14 |
| single-stateless | 200/s | 198.2 | 589.1 | 68.52 | 126.81 | 337.57 | 100% | 50 | 69 |
| single-stateless | 500/s | 138.9 | 348.3 | 3460.41 | 7332.03 | 10000.8 | 99.7% | 21416 | 500 |

## P95 Latency

| Topology | Rate | P95 (ms) | |
|----------|------|----------|-|
| cluster-stateful | 100/s | 53.46 | `████████████` |
| cluster-stateful | 200/s | 4062.32 | `██████████████████████████` |
| cluster-stateless | 100/s | 52.6 | `████████████` |
| cluster-stateless | 200/s | 53.43 | `████████████` |
| cluster-stateless | 500/s | 56.33 | `█████████████` |
| cluster-stateless | 1000/s | 5443.24 | `███████████████████████████` |
| single-stateful | 100/s | 2081.54 | `████████████████████████` |
| single-stateful | 200/s | 4628.01 | `███████████████████████████` |
| single-stateless | 100/s | 55.96 | `█████████████` |
| single-stateless | 200/s | 126.81 | `███████████████` |
| single-stateless | 500/s | 7332.03 | `████████████████████████████` |
| single-stateless | 1000/s | 13854.8 | `██████████████████████████████` |

## Notes

- **Target Rate**: The configured constant-arrival-rate (requests/second k6 attempts)
- **Actual Iter/s**: Achieved iterations per second (each iteration = 1 POST /api/exec)
- **HTTP Req/s**: Total HTTP requests per second (1 per iteration)
- **Dropped**: Iterations k6 couldn't schedule because VUs were exhausted (indicates server saturation)
- **Topology**: `single` = 1 MCP-V8 node; `cluster` = 3 MCP-V8 nodes with Raft
