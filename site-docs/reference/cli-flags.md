# CLI Flags Reference

Complete reference for all `mcp-v8` server flags.

## Utility

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--print-openapi` | `bool` | `false` | Print OpenAPI JSON specification to stdout and exit |

## Storage

| Flag | Type | Default | Conflicts | Description |
|------|------|---------|-----------|-------------|
| `--s3-bucket` | `string` | (none) | `--directory-path`, `--stateless` | S3 bucket name for heap storage |
| `--cache-dir` | `string` | (none) | -- | Local FS cache directory (requires `--s3-bucket`) |
| `--directory-path` | `string` | `/tmp/mcp-v8-heaps` | `--s3-bucket`, `--stateless` | Directory path for filesystem storage |
| `--stateless` | `bool` | `false` | `--s3-bucket`, `--directory-path` | Run in stateless mode (no heap snapshots) |

## Transport

| Flag | Type | Default | Conflicts | Description |
|------|------|---------|-----------|-------------|
| `--http-port` | `u16` | (none) | `--sse-port` | Streamable HTTP transport port (MCP 2025-03-26+) |
| `--sse-port` | `u16` | (none) | `--http-port` | SSE transport port (older HTTP+SSE) |

## Execution

| Flag | Type | Default | Range | Description |
|------|------|---------|-------|-------------|
| `--heap-memory-max` | `u64` | `8` | 1+ | Maximum V8 heap memory per isolate in megabytes |
| `--execution-timeout` | `u64` | `30` | 1-300 | Maximum execution timeout in seconds |
| `--max-concurrent-executions` | `usize` | CPU core count | 1+ | Maximum concurrent V8 executions |

## Session

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--session-db-path` | `string` | `/tmp/mcp-v8-sessions` | Path to the sled database for session logging |

## Cluster

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--cluster-port` | `u16` | (none) | Raft cluster HTTP server port. Enables cluster mode. |
| `--node-id` | `string` | `"node1"` | Unique node identifier |
| `--peers` | `string` (comma-separated) | (none) | Seed peer addresses (`id@host:port` or `host:port`) |
| `--join` | `string` | (none) | Join existing cluster via seed address |
| `--advertise-addr` | `string` | `<node-id>:<cluster-port>` | Externally-reachable address for this node |
| `--heartbeat-interval` | `u64` | `100` | Heartbeat interval in milliseconds |
| `--election-timeout-min` | `u64` | `300` | Minimum election timeout in milliseconds |
| `--election-timeout-max` | `u64` | `500` | Maximum election timeout in milliseconds |

## Policy

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--policies-json` | `string` | (none) | JSON policy config (inline JSON or file path) |

## Fetch

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--fetch-header` | `string` (repeatable) | (none) | Header injection rule (`host=...,header=...,value=...`) |
| `--fetch-header-config` | `string` | (none) | Path to JSON file with header injection rules |

## WASM

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--wasm-module` | `string` (repeatable) | (none) | Pre-load WASM module (`NAME=PATH[:LIMIT]`) |
| `--wasm-config` | `string` | (none) | Path to JSON WASM configuration file |
| `--wasm-default-max-memory` | `string` | `"16m"` | Default max native memory for WASM modules |

## Modules

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--allow-external-modules` | `bool` | `false` | Allow npm:, jsr:, and URL module imports |

## MCP Servers

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--mcp-server` | `string` (repeatable) | (none) | Connect to external MCP server (`name=stdio:cmd:args` or `name=sse:url`) |
| `--mcp-config` | `string` | (none) | Path to JSON file with MCP server configurations |
| `--mcp-stubs` | `bool` | `true` | Expose upstream MCP tools as stub tools on this server |
| `--mcp-stub-prefix` | `string` | `"runjs__"` | Prefix for stub tool names |

## Auth

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--jwks-url` | `string` | (none) | JWKS endpoint URL for JWT verification. Also settable via `JWKS_URL` env var. |
