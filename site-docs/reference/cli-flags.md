# CLI Flags

> Generated from the Clap `Cli` definition. Do not edit this page by hand.

`mcp-v8` is configured through command-line flags. This page is grouped
using the same help headings exposed by the CLI itself.

## Sections

- [Cluster](#cluster)
- [Core](#core)
- [Fetch](#fetch)
- [MCP Server Module](#mcp-server-module)
- [Module Import](#module-import)
- [Policy](#policy)
- [WASM](#wasm)

## Cluster

### `--cluster-port`

Port for the Raft cluster HTTP server. Enables cluster mode when set

- Value: `CLUSTER_PORT`

### `--node-id`

Unique node identifier within the cluster

- Default: `node1`
- Value: `NODE_ID`

### `--peers`

Comma-separated list of seed peer addresses. Format: id@host:port or host:port. Peers can also join dynamically via POST /raft/join

- Value: `PEERS`
- Delimiter: `,`
- Repeatable: yes

### `--join`

Join an existing cluster by contacting this seed address (host:port). The node will register itself with the cluster leader via /raft/join

- Value: `JOIN`

### `--advertise-addr`

Advertise address for this node (host:port). Used for peer discovery and write forwarding. Defaults to <node-id>:<cluster-port>

- Value: `ADVERTISE_ADDR`

### `--heartbeat-interval`

Heartbeat interval in milliseconds

- Default: `100`
- Value: `HEARTBEAT_INTERVAL`

### `--election-timeout-min`

Minimum election timeout in milliseconds

- Default: `300`
- Value: `ELECTION_TIMEOUT_MIN`

### `--election-timeout-max`

Maximum election timeout in milliseconds

- Default: `500`
- Value: `ELECTION_TIMEOUT_MAX`

## Core

### `--print-openapi`

Print the OpenAPI JSON specification to stdout and exit. Use this to regenerate openapi.json: `./server --print-openapi > openapi.json`

### `--s3-bucket`

S3 bucket name (required if --use-s3)

- Value: `S3_BUCKET`

### `--cache-dir`

Local filesystem cache directory for S3 write-through caching (only used with --s3-bucket)

- Value: `CACHE_DIR`

### `--directory-path`

Directory path for filesystem storage (required if --use-filesystem)

- Value: `DIRECTORY_PATH`

### `--stateless`

Run in stateless mode - no heap snapshots are saved or loaded

### `--jwks-url`

JWKS endpoint URL for fetching public keys (e.g., Keycloak OIDC certs URL). Enables JWT verification of Authorization: Bearer tokens during initialize

- Environment: `JWKS_URL`
- Value: `JWKS_URL`

### `--http-port`

HTTP port using Streamable HTTP transport (MCP 2025-03-26+, load-balanceable)

- Value: `HTTP_PORT`

### `--sse-port`

SSE port using the older HTTP+SSE transport

- Value: `SSE_PORT`

### `--heap-memory-max`

Maximum V8 heap memory per isolate in megabytes (default: 8)

- Default: `8`
- Value: `HEAP_MEMORY_MAX`

### `--execution-timeout`

Maximum execution timeout in seconds (default: 30, max: 300)

- Default: `30`
- Value: `EXECUTION_TIMEOUT`

### `--max-concurrent-executions`

Maximum concurrent V8 executions (default: CPU core count)

- Value: `MAX_CONCURRENT_EXECUTIONS`

### `--session-db-path`

Path to the sled database for session logging (default: /tmp/mcp-v8-sessions)

- Default: `/tmp/mcp-v8-sessions`
- Value: `SESSION_DB_PATH`

## Fetch

### `--fetch-header`

Inject headers into fetch requests matching host/method rules. Format: host=<host>,header=<name>,value=<val>[,methods=GET;POST] Can be specified multiple times

- Value: `RULE`
- Repeatable: yes

### `--fetch-header-config`

Path to a JSON file with header injection rules. Format: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}]

- Value: `PATH`

## MCP Server Module

### `--mcp-server`

Connect to an external MCP server as a module. JS code can call its tools via the `mcp` global object (mcp.callTool, mcp.listTools, mcp.servers). Format for stdio: name=stdio:command:arg1:arg2 Format for SSE: name=sse:url Can be specified multiple times for multiple servers

- Value: `NAME=TRANSPORT:...`
- Repeatable: yes

### `--mcp-config`

Path to a JSON config file for MCP server modules. Format: [{"name": "srv", "transport": "stdio", "command": "cmd", "args": ["a"]}, {"name": "srv2", "transport": "sse", "url": "http://..."}]

- Value: `PATH`

### `--mcp-stubs`

Expose upstream MCP server tools on the MCPJS server itself as `<prefix><server>__<tool>` stubs. When `true` (the default whenever at least one --mcp-server is configured), an external client of MCPJS can discover those tools via tools/list and tool search; calling a stub returns instructional text telling the caller to invoke the tool from JavaScript via run_js + mcp.callTool(...). Pass `--mcp-stubs false` to disable

- Default: `true`

### `--mcp-stub-prefix`

Prefix applied to stub tool names. Defaults to `runjs__` so it is obvious to a calling agent that these tools execute through the JS runtime rather than dispatching directly. Has no effect when --mcp-stubs is false

- Default: `runjs__`
- Value: `MCP_STUB_PREFIX`

## Module Import

### `--allow-external-modules`

Allow external module imports (npm:, jsr:, and URL imports). When disabled (the default), code using import declarations for external packages will be rejected. Enable with --allow-external-modules

- Default: `false`

## Policy

### `--policies-json`

JSON policy configuration (inline JSON or path to a JSON file). Enables fetch() and/or module policy gating via local Rego files and/or remote OPA servers. Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}' Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }

- Value: `JSON_OR_PATH`

## WASM

### `--wasm-module`

Pre-load a WASM module as a global. Format: name=/path/to/module.wasm[:max_memory] The module's exports will be available as a global variable with the given name. Optional memory suffix caps the module's native memory (linear memory + tables). Supported suffixes: raw bytes, k/K (KiB), m/M (MiB), g/G (GiB). Examples: math=/path.wasm math=/path.wasm:16m math=/path.wasm:1048576 Can be specified multiple times for multiple modules

- Value: `NAME=PATH[:LIMIT]`
- Repeatable: yes

### `--wasm-config`

Path to a JSON config file mapping global names to .wasm file paths or objects. String value: {"name": "/path/to/module.wasm"} Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216}}

- Value: `PATH`

### `--wasm-default-max-memory`

Default max native memory for WASM modules without a per-module limit. Supports suffixes: k/K (KiB), m/M (MiB), g/G (GiB), or raw bytes. This is separate from --heap-memory-max (JS heap); WASM linear memory is allocated as native memory outside the V8 heap

- Default: `16m`
- Value: `WASM_DEFAULT_MAX_MEMORY`

### `--wasm-stubs`

Expose pre-loaded WASM modules on the MCPJS server itself as `<prefix>wasm__<name>` stubs. When `true` (the default whenever at least one WASM module is loaded), an external client of MCPJS can discover the module via tools/list and tool search; calling a stub returns instructional text telling the caller to use the module from JavaScript via run_js (the module is available as the `__wasm_<name>` global). Pass `--wasm-stubs false` to disable

- Default: `true`

### `--wasm-stub-prefix`

Prefix applied to WASM stub tool names. Defaults to `runjs__` so it is obvious to a calling agent that these modules execute through the JS runtime rather than dispatching directly. Has no effect when --wasm-stubs is false

- Default: `runjs__`
- Value: `WASM_STUB_PREFIX`
