# CLI Flags

> Generated from the Clap `Cli` struct in `server/src/main.rs`. Do not edit this page by hand.

`mcp-v8` is configured through command-line flags. This page is grouped
the same way the flags are grouped in the source.

## Sections

- [Core](#core)
- [Cluster](#cluster)
- [WASM Module](#wasm-module)
- [Fetch](#fetch)
- [Module Import](#module-import)
- [Policy](#policy)
- [MCP Server Module](#mcp-server-module)

## Core

### `--print-openapi`

Print the OpenAPI JSON specification to stdout and exit. Use this to regenerate openapi.json: `./server --print-openapi > openapi.json`

### `--s3-bucket`

S3 bucket name (required if --use-s3)

- Conflicts: `--directory-path`, `--stateless`

### `--cache-dir`

Local filesystem cache directory for S3 write-through caching (only used with --s3-bucket)

- Requires: `--s3-bucket`

### `--directory-path`

Directory path for filesystem storage (required if --use-filesystem)

- Conflicts: `--s3-bucket`, `--stateless`

### `--stateless`

Run in stateless mode - no heap snapshots are saved or loaded

- Conflicts: `--s3-bucket`, `--directory-path`

### `--jwks-url`

JWKS endpoint URL for fetching public keys (e.g., Keycloak OIDC certs URL). Enables JWT verification of Authorization: Bearer tokens during initialize.

- Environment: `JWKS_URL`

### `--http-port`

HTTP port using Streamable HTTP transport (MCP 2025-03-26+, load-balanceable)

- Conflicts: `--sse-port`

### `--sse-port`

SSE port using the older HTTP+SSE transport

- Conflicts: `--http-port`

### `--heap-memory-max`

Maximum V8 heap memory per isolate in megabytes (default: 8)

### `--execution-timeout`

Maximum execution timeout in seconds (default: 30, max: 300)

### `--max-concurrent-executions`

Maximum concurrent V8 executions (default: CPU core count)

### `--session-db-path`

Path to the sled database for session logging (default: /tmp/mcp-v8-sessions)

## Cluster

### `--cluster-port`

Port for the Raft cluster HTTP server. Enables cluster mode when set.

### `--node-id`

Unique node identifier within the cluster

### `--peers`

Comma-separated list of seed peer addresses. Format: id@host:port or host:port. Peers can also join dynamically via POST /raft/join.

- Delimiter: `,`

### `--join`

Join an existing cluster by contacting this seed address (host:port). The node will register itself with the cluster leader via /raft/join.

### `--advertise-addr`

Advertise address for this node (host:port). Used for peer discovery and write forwarding. Defaults to <node-id>:<cluster-port>.

### `--heartbeat-interval`

Heartbeat interval in milliseconds

### `--election-timeout-min`

Minimum election timeout in milliseconds

### `--election-timeout-max`

Maximum election timeout in milliseconds

## WASM Module

### `--wasm-module`

Pre-load a WASM module as a global. Format: name=/path/to/module.wasm[:max_memory] The module's exports will be available as a global variable with the given name. Optional memory suffix caps the module's native memory (linear memory + tables). Supported suffixes: raw bytes, k/K (KiB), m/M (MiB), g/G (GiB). Examples: math=/path.wasm  math=/path.wasm:16m  math=/path.wasm:1048576 Can be specified multiple times for multiple modules.

- Value: `NAME=PATH[:LIMIT]`

### `--wasm-config`

Path to a JSON config file mapping global names to .wasm file paths or objects. String value: {"name": "/path/to/module.wasm"} Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216}}

- Value: `PATH`

### `--wasm-default-max-memory`

Default max native memory for WASM modules without a per-module limit. Supports suffixes: k/K (KiB), m/M (MiB), g/G (GiB), or raw bytes. This is separate from --heap-memory-max (JS heap); WASM linear memory is allocated as native memory outside the V8 heap.

## Fetch

### `--fetch-header`

Inject headers into fetch requests matching host/method rules. Format: host=<host>,header=<name>,value=<val>[,methods=GET;POST] Can be specified multiple times.

- Value: `RULE`

### `--fetch-header-config`

Path to a JSON file with header injection rules. Format: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}]

- Value: `PATH`

## Module Import

### `--allow-external-modules`

Allow external module imports (npm:, jsr:, and URL imports). When disabled (the default), code using import declarations for external packages will be rejected. Enable with --allow-external-modules.

## Policy

### `--policies-json`

JSON policy configuration (inline JSON or path to a JSON file). Enables fetch() and/or module policy gating via local Rego files and/or remote OPA servers.  Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}'  Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }

- Value: `JSON_OR_PATH`

## MCP Server Module

### `--mcp-server`

Connect to an external MCP server as a module. JS code can call its tools via the `mcp` global object (mcp.callTool, mcp.listTools, mcp.servers). Format for stdio: name=stdio:command:arg1:arg2 Format for SSE:   name=sse:url Can be specified multiple times for multiple servers.

- Value: `NAME=TRANSPORT:...`

### `--mcp-config`

Path to a JSON config file for MCP server modules. Format: [{"name": "srv", "transport": "stdio", "command": "cmd", "args": ["a"]}, {"name": "srv2", "transport": "sse", "url": "http://..."}]

- Value: `PATH`

### `--mcp-stubs`

Expose upstream MCP server tools on the MCPJS server itself as `<prefix><server>__<tool>` stubs. When `true` (the default whenever at least one --mcp-server is configured), an external client of MCPJS can discover those tools via tools/list and tool search; calling a stub returns instructional text telling the caller to invoke the tool from JavaScript via run_js + mcp.callTool(...). Pass `--mcp-stubs false` to disable.

### `--mcp-stub-prefix`

Prefix applied to stub tool names. Defaults to `runjs__` so it is obvious to a calling agent that these tools execute through the JS runtime rather than dispatching directly. Has no effect when --mcp-stubs is false.
