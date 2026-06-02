# Architecture Overview

mcp-v8 is a Rust server that gives AI agents the ability to execute JavaScript and TypeScript code inside a sandboxed V8 engine. It exposes this capability through the Model Context Protocol (MCP), a standard interface that language models use to interact with external tools. The server can also be driven through a plain HTTP REST API, making it accessible to any HTTP client.

## Why mcp-v8 Exists

Language models are good at generating code, but they need somewhere to run it. Handing an LLM direct access to a shell or a full Node.js process is risky: there is no isolation, no resource control, and no way to enforce policy over what the code does. mcp-v8 solves this by providing a purpose-built execution environment where every operation -- network requests, filesystem access, module imports, subprocess execution -- is gated by configurable policies.

## High-Level Component Map

The system is organized into several layers:

```
  MCP / HTTP Clients
        |
  +-----------+
  | Transport |   stdio, Streamable HTTP (/mcp), SSE (/sse + /message)
  +-----------+
        |
  +------------------+
  | MCP Service      |   Tool registration, resource serving, session verification
  | HTTP API         |   REST endpoints (/api/exec, /api/executions/...)
  +------------------+
        |
  +------------------+
  | Engine           |   Orchestrates execution, manages concurrency
  +------------------+
        |
  +----------+-------+-------+--------+--------+----------+
  | V8       | Heap  | Sess. | Console| Policy | Module   |
  | Runtime  | Store | Log   | Output | (OPA)  | Loader   |
  +----------+-------+-------+--------+--------+----------+
        |
  +----------+-------+--------+-------+--------+
  | fetch()  | fs    | mcp    | WASM  | subprocess
  | (HTTP)   |       | client | modules|
  +----------+-------+--------+-------+--------+
```

**Transport layer** -- Accepts connections over stdio (default, for local MCP clients like Claude Desktop), Streamable HTTP (the modern MCP transport), or SSE (the legacy MCP transport). All three expose the same MCP tool interface.

**MCP Service / HTTP API** -- The MCP service registers tools (`run_js`, `get_execution_status`, `get_execution_output`, `cancel_execution`, and session/tag management tools in stateful mode). The HTTP API provides equivalent REST endpoints with an OpenAPI specification and optional Swagger UI.

**Engine** -- The central coordinator. It manages the V8 execution lifecycle: TypeScript type-stripping, snapshot loading, V8 isolate creation, timeout enforcement, concurrency control via a semaphore, and snapshot persistence. The Engine is the boundary between the protocol layer and V8.

**Storage and state** -- Heap snapshots are stored in a pluggable `HeapStorage` backend (local filesystem, S3, or write-through cache). Session logs and heap tags are persisted in sled, an embedded key-value database. Console output is buffered into per-execution sled trees using a WAL-style append pattern.

**Policy layer** -- Every external operation (network fetch, filesystem I/O, module import, MCP tool call, subprocess execution) passes through an OPA policy chain before executing. Policies can be local Rego files evaluated by the embedded regorus engine, or queries to a remote OPA server. The chain supports AND and OR composition modes.

**Module loader** -- ES module `import` statements are resolved at runtime. `npm:` and `jsr:` specifiers are rewritten to esm.sh URLs. URL imports are fetched over HTTP. All external imports can be disabled entirely or gated by policy.

## Stateful vs. Stateless

The Engine operates in one of two modes, chosen at startup:

- **Stateful** -- After each execution, the V8 heap is serialized into a snapshot, hashed with SHA-256, and stored. Subsequent executions can resume from any prior snapshot, creating a content-addressed history of computation states. Session logs track the chain of snapshots. This mode is ideal for multi-turn agent conversations where the LLM builds up state across calls.

- **Stateless** -- Each execution starts from a clean V8 isolate. No snapshots are stored. This mode is simpler, fully parallelizable (no snapshot mutex), and suited for one-shot code evaluation.

## Execution Flow

Regardless of mode, every execution follows this path:

1. Code is received (via MCP tool call or HTTP POST).
2. TypeScript type annotations are stripped using SWC, producing plain JavaScript.
3. A semaphore permit is acquired (bounding concurrent V8 executions).
4. The code is submitted to a background task, which returns an execution ID immediately.
5. On the blocking thread pool, a V8 isolate is created (optionally from a snapshot), the sandbox is hardened, and the code runs as an ES module.
6. A timeout races against the execution. If the timeout wins, the V8 isolate is terminated.
7. Console output streams into sled in real time. The caller can poll for output while execution is still running.
8. On completion, the result (and snapshot, in stateful mode) is persisted, and the execution status is updated.

## Clustering

Multiple mcp-v8 nodes can form a Raft cluster for high availability. The cluster replicates session logs and heap tags through Raft consensus. Writes are forwarded to the leader. Peers can join dynamically via a REST endpoint. Heap snapshot storage itself is not replicated through Raft -- it relies on the shared storage backend (e.g., S3).

## Security Model

Security is layered:

- **V8 isolation** -- Each execution runs in its own V8 isolate with a bounded heap and a bounded ArrayBuffer allocator. The sandbox is hardened after setup: dangerous ops are neutralized, `Deno.core.ops` is frozen, `SharedArrayBuffer` is removed, and introspection APIs are disabled.
- **Policy gating** -- OPA policies control what code can do: which URLs it can fetch, which files it can access, which modules it can import, which MCP tools it can call, and which subprocesses it can spawn.
- **Snapshot validation** -- Heap snapshots are wrapped in an envelope with a magic header and SHA-256 checksum. Invalid or corrupted data is rejected before it reaches V8.
- **Resource limits** -- Heap size, execution timeout, WASM memory, and concurrent execution count are all configurable and enforced.
- **JWT verification** -- Optional JWKS-based JWT verification can authenticate MCP sessions.
