# Configuration file

> Generated from the Clap `Cli` definition and the `--config` loader's
> section/rejected-key tables. Do not edit this page by hand.

One TOML or JSON file, passed via `--config <PATH>` (environment:
`MCP_V8_CONFIG`), configures everything the CLI can. The format is chosen
by file extension: `.toml` or `.json`.

```bash
mcp-v8 --config /etc/mcp-v8/server.toml
```

## Precedence

Each setting is resolved independently, highest priority first:

1. Explicit command-line flag
2. `MCP_V8_*` environment variable
3. Config file
4. Built-in default

## Value forms

Keys are named after their CLI flag; dashes and underscores are
interchangeable (`http-port` ≡ `http_port`). Values are parsed exactly like
flag values, so anything the flag accepts, the key accepts; scalars may be
written as strings, numbers, or booleans. Keys marked as arrays take one
array element per repetition of the flag. Relative paths are resolved
against the server's working directory, not the config file's location.

Every violation is a fatal startup error: unknown keys (the error lists
every accepted key), keys set twice (counting both spellings), values of
the wrong shape, and keys whose flags Clap declares as conflicting (e.g.
`http_port` and `sse_port`).

## Sections

- [Structured sections](#structured-sections)
- [Cluster](#cluster)
- [Core](#core)
- [Fetch](#fetch)
- [Filesystem](#filesystem)
- [Heap](#heap)
- [MCP Server Module](#mcp-server-module)
- [Module Import](#module-import)
- [OS Sandbox](#os-sandbox)
- [Policy](#policy)
- [Prompt](#prompt)
- [Run JS File](#run-js-file)
- [Sandbox](#sandbox)
- [Storage (S3)](#storage-s3)
- [WASM](#wasm)
- [Keys not available in config files](#keys-not-available-in-config-files)

## Structured sections

These keys hold structured data inline, replacing what is otherwise a
separate JSON file (or inline-JSON flag value). Each section is
re-serialized to JSON and handed to its target flag's loader, so the
schema is exactly the target flag's. A section and its target key (e.g.
`wasm` and `wasm_config`) cannot both be set in the same file.

### `wasm`

Shape: a table/object. Feeds `--wasm-config` (schema below).

JSON config mapping global names to .wasm file paths or objects (a path to a JSON file, or inline JSON — also settable as the `wasm` section of a --config file). String value: {"name": "/path/to/module.wasm"} Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216, "description": "what the module does"}} The optional "description" sets the MCP stub tool's description. NOTE: incompatible with heap persistence (`--heap-store` other than none)

### `mcp_servers`

Shape: an array. Feeds `--mcp-config` (schema below).

JSON config for MCP server modules (a path to a JSON file, or inline JSON — also settable as the `mcp_servers` section of a --config file). Format: [{"name": "srv", "transport": "stdio", "command": "cmd", "args": ["a"]}, {"name": "srv2", "transport": "sse", "url": "http://..."}, {"name": "srv3", "transport": "http", "url": "https://...", "auth": {"type": "oauth_browser", "scope": ["read"], "client_id": "...", "client_secret": "...", "redirect_port": 48123, "token_cache": "/path/to/cache.json"}}] `oauth_browser` is supported for HTTP only through `--mcp-config` and structured JSON/TOML `mcp_servers` configuration; compact `--mcp-server` syntax does not support it. Protected-resource and discovered OAuth endpoints require HTTPS unless loopback. It prints an authorization URL only when no cached credential can provide a token; cached refresh tokens renew access on connection or reconnect without opening a browser. The callback binds to localhost on redirect_port (or an available port). See the README for headless authorization and cache-file security

### `fetch_headers`

Shape: an array. Feeds `--fetch-header-config` (schema below).

JSON array of header injection rules (a path to a JSON file, or inline JSON — also settable as the `fetch_headers` section of a --config file). Each rule sets "host" (plus optional "methods") and exactly one of "headers" or "auth". Static: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}] OAuth: [{"host": "api.example.com", "auth": {"type": "oauth_client_credentials", "header": "Authorization", "token_url": "https://issuer.example.com/token", "client_id": "abc", "client_secret": "xyz", "scope": "read:all", "refresh_buffer_secs": 30}}]

### `policies`

Shape: a table/object. Feeds `--policies-json` (schema below).

JSON policy configuration (inline JSON or path to a JSON file). Enables fetch() and/or module policy gating via local Rego files and/or remote OPA servers. Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}' Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }

### `sandbox`

Shape: a table/object. Feeds `--sandbox-manifest` (schema below).

Confine the whole server process with an OS-enforced sandbox (nono: Landlock on Linux 5.13+, Seatbelt on macOS) described by a nono capability manifest — a JSON file path or inline JSON; in a --config file the structured `sandbox` section is the ergonomic form (and the NixOS module exposes it as a plain attrset). nono capabilities compose additively, so the enforced set is the manifest (verbatim, in nono's schema and semantics) unioned with a server baseline derived from this configuration: storage directories read-write, config/policy/WASM inputs and system paths read-only, and bind grants for configured listener ports. The manifest owns the network egress posture — the baseline never opens outbound access. Applied before the async runtime and V8 spawn any threads; irreversible once applied. This is defense in depth *underneath* the OPA policy layer. Fails closed: startup aborts if the platform cannot enforce the composed set, and manifest features that need nono's CLI supervisor (network proxy mode, credentials, rollback, fs deny rules) are rejected rather than silently ignored

## Cluster

### `cluster_port`

Port for the Raft cluster HTTP server. Enables cluster mode when set

- CLI flag: `--cluster-port`
- Environment: `MCP_V8_CLUSTER_PORT`

### `metadata_only`

Run as a metadata-only cluster node: serve Raft replication of session metadata (session log, heap tags, fs labels) and nothing else. No V8 engine is created, no MCP transport or REST sidecar is started, and no policies are needed — the node's only surface is the Raft HTTP server on --cluster-port, where it acts purely as a leader or replica for the replicated metadata store. Requires --cluster-port and conflicts with the MCP transports and all JS-execution configuration

- CLI flag: `--metadata-only`
- Environment: `MCP_V8_METADATA_ONLY`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

### `node_id`

Unique node identifier within the cluster

- CLI flag: `--node-id`
- Environment: `MCP_V8_NODE_ID`
- Default: `node1`

### `join`

Join an existing cluster by contacting this seed address (host:port). The node will register itself with the cluster leader via /raft/join

- CLI flag: `--join`
- Environment: `MCP_V8_JOIN`

### `join_as_learner`

Join as a non-voting learner: the node replicates the log but is excluded from election and commit quorums and never starts elections. Use for ephemeral nodes whose churn must not affect availability

- CLI flag: `--join-as-learner`
- Environment: `MCP_V8_JOIN_AS_LEARNER`
- Possible values: `true`, `false`
- Type: boolean

### `advertise_addr`

Advertise address for this node (host:port). Used for peer discovery and write forwarding. Defaults to <node-id>:<cluster-port>

- CLI flag: `--advertise-addr`
- Environment: `MCP_V8_ADVERTISE_ADDR`

### `heartbeat_interval`

Heartbeat interval in milliseconds

- CLI flag: `--heartbeat-interval`
- Environment: `MCP_V8_HEARTBEAT_INTERVAL`
- Default: `100`

### `election_timeout_min`

Minimum election timeout in milliseconds

- CLI flag: `--election-timeout-min`
- Environment: `MCP_V8_ELECTION_TIMEOUT_MIN`
- Default: `300`

### `election_timeout_max`

Maximum election timeout in milliseconds

- CLI flag: `--election-timeout-max`
- Environment: `MCP_V8_ELECTION_TIMEOUT_MAX`
- Default: `500`

### `peers`

Comma-separated list of seed peer addresses. Peers can also join dynamically via POST /raft/join. Forms: id@host:port — peer address with an explicit node id host:port — peer address only (node id learned on join) Examples: node2@10.0.0.2:4000 10.0.0.3:4000

- CLI flag: `--peers`
- Environment: `MCP_V8_PEERS`
- Type: array (one element per flag repetition)

## Core

### `jwks_url`

JWKS endpoint URL for fetching public keys (e.g., Keycloak OIDC certs URL). When set, the HTTP transports ENFORCE auth: every request to /mcp and the HTTP API (/api/*) must carry a valid `Authorization: Bearer <jwt>` (or `agent-session` header) verified against this JWKS, else it is rejected with 401. Leaving it unset keeps the server open (no auth). The openapi spec route and CORS preflight (OPTIONS) are exempt

- CLI flag: `--jwks-url`
- Environment: `JWKS_URL`

### `session_id`

Fixed session id for this process, used when no X-MCP-Session-Id header is available (i.e. the stdio transport). Keys per-session heap+fs state so a process spawned for a given logical session (e.g. one per thread) resumes that session's stateful heap+fs. Over HTTP the header still wins

- CLI flag: `--session-id`
- Environment: `MCP_V8_SESSION_ID`

### `session_fork_from`

Fork the new session (given by --session-id) from a previous session's latest heap+fs snapshot. The new session starts with the source session's state but is independent: subsequent snapshots are written under the new session id, leaving the source untouched (copy-on-write). Requires --session-id and heap and/or fs persistence. No-op if the target session already has history

- CLI flag: `--session-fork-from`
- Environment: `MCP_V8_SESSION_FORK_FROM`

### `http_port`

HTTP port using Streamable HTTP transport (MCP 2025-03-26+, load-balanceable). Falls back to the `PORT` environment variable when neither this nor `--sse-port` is set anywhere, so platforms that inject `PORT` (Railway, Render, Heroku, Fly, Cloud Run) serve Streamable HTTP unmodified

- CLI flag: `--http-port`
- Environment: `MCP_V8_HTTP_PORT`

### `sse_port`

SSE port using the legacy HTTP+SSE transport (served by a vendored rmcp 0.1.5; no MCP tasks support — use --http-port for tasks)

- CLI flag: `--sse-port`
- Environment: `MCP_V8_SSE_PORT`

### `bind_host`

Host/address the HTTP and SSE transports bind to. Defaults to all IPv4 interfaces (0.0.0.0). Set to "::" for a dual-stack IPv6 listener, which is required to be reachable over IPv6-resolving private networks (e.g. Railway)

- CLI flag: `--bind-host`
- Environment: `MCP_V8_BIND_HOST`
- Default: `0.0.0.0`

### `allowed_hosts`

`Host` header allowlist for the Streamable HTTP transport, which rejects unlisted hosts with 403 to blunt DNS-rebinding attacks. Entries are hostnames or `host:port` authorities (a bare hostname matches any port); `*` allows any host. Defaults to loopback-only (`localhost`, `127.0.0.1`, `::1`) whatever `--bind-host` is, because a wildcard bind still answers on loopback and so is reachable by a browser on the same machine. Set this to the hostnames clients use when serving over a network; the Docker image ships `MCP_V8_ALLOWED_HOSTS=*`

- CLI flag: `--allowed-hosts`
- Environment: `MCP_V8_ALLOWED_HOSTS`
- Type: array (one element per flag repetition)

### `allowed_origins`

`Origin` header allowlist for the Streamable HTTP transport, for browser clients. Entries must include a scheme (`https://app.example.com`); `null` matches a sandboxed browser's `Origin: null`. Empty (the default) skips Origin validation entirely; when non-empty, a request carrying an unlisted Origin is rejected with 403, while one sending no Origin at all still passes

- CLI flag: `--allowed-origins`
- Environment: `MCP_V8_ALLOWED_ORIGINS`
- Type: array (one element per flag repetition)

### `heap_memory_max`

Maximum V8 heap memory per isolate in megabytes (default: 8)

- CLI flag: `--heap-memory-max`
- Environment: `MCP_V8_HEAP_MEMORY_MAX`
- Default: `8`

### `execution_timeout`

Maximum execution timeout in seconds (default: 30, max: 300)

- CLI flag: `--execution-timeout`
- Environment: `MCP_V8_EXECUTION_TIMEOUT`
- Default: `30`

### `max_concurrent_executions`

Maximum concurrent V8 executions (default: CPU core count)

- CLI flag: `--max-concurrent-executions`
- Environment: `MCP_V8_MAX_CONCURRENT_EXECUTIONS`

### `session_db_path`

Path to the sled database for the session log (per-session heap+fs history) and the execution registry. Also the default parent for the heap-tag store, fs blob store, and fs label db. Default: /tmp/mcp-v8-sessions

- CLI flag: `--session-db-path`
- Environment: `MCP_V8_SESSION_DB_PATH`
- Default: `/tmp/mcp-v8-sessions`

## Fetch

### `fetch_header_config`

JSON array of header injection rules (a path to a JSON file, or inline JSON — also settable as the `fetch_headers` section of a --config file). Each rule sets "host" (plus optional "methods") and exactly one of "headers" or "auth". Static: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}] OAuth: [{"host": "api.example.com", "auth": {"type": "oauth_client_credentials", "header": "Authorization", "token_url": "https://issuer.example.com/token", "client_id": "abc", "client_secret": "xyz", "scope": "read:all", "refresh_buffer_secs": 30}}]

- CLI flag: `--fetch-header-config`
- Environment: `MCP_V8_FETCH_HEADER_CONFIG`

## Filesystem

### `fs_store`

Content-addressed `/work` filesystem backend. `none` (default) = no fs persistence. `dir` = node-local blob store (`--fs-dir`). `s3` = shared `--s3-bucket`. When enabled, the `fs` parameter of run_js can mount a snapshot (by label or CA id) and the `fs_*` tools / `/api/fs/...` endpoints become functional. Works with any isolate (compatible with `--wasm-module`). In cluster mode labels replicate cluster-wide, but blobs/manifests are only shared when stored on shared storage — so `--fs-store s3` is required when running fs persistence in a cluster.

- CLI flag: `--fs-store`
- Environment: `MCP_V8_FS_STORE`
- Default: `none`
- Possible values: `none`, `dir`, `s3`

### `fs_dir`

Directory for the fs snapshot blob store (chunks + manifests) when `--fs-store dir`. Defaults to `<session-db-path>/fs-blobs`

- CLI flag: `--fs-dir`
- Environment: `MCP_V8_FS_DIR`

### `fs_labels_db`

Path for the fs label/reflog database (sled). Defaults to `<session-db-path>/fs-labels`

- CLI flag: `--fs-labels-db`
- Environment: `MCP_V8_FS_LABELS_DB`

### `fs_passthrough`

Overlay read behaviour when a per-session fs snapshot is mounted. Off (default): overlay-only — the mounted snapshot is the entire fs view, so a read that misses it is ENOENT (strict isolation). On: overlayfs-style — fall through to the real filesystem as a read-only lower layer (still gated by the filesystem policy), so bundled read-only paths like `/opt/languages` resolve while `/work` stays the per-session overlay

- CLI flag: `--fs-passthrough`
- Environment: `MCP_V8_FS_PASSTHROUGH`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

## Heap

### `heap_store`

V8 heap-snapshot backend. `none` (default) = no heap persistence; JS globals do NOT survive between runs. `dir` = node-local directory (`--heap-dir`). `s3` = shared `--s3-bucket` (optionally `--cache-dir`). Heap snapshots require a V8 SnapshotCreator isolate, which disables WebAssembly — so heap persistence is mutually exclusive with `--wasm-module`/`--wasm-config` (rejected at startup).

- CLI flag: `--heap-store`
- Environment: `MCP_V8_HEAP_STORE`
- Default: `none`
- Possible values: `none`, `dir`, `s3`

### `heap_dir`

Directory for the heap-snapshot store when `--heap-store dir`. Defaults to /tmp/mcp-v8-heaps

- CLI flag: `--heap-dir`
- Environment: `MCP_V8_HEAP_DIR`

## MCP Server Module

### `mcp_config`

JSON config for MCP server modules (a path to a JSON file, or inline JSON — also settable as the `mcp_servers` section of a --config file). Format: [{"name": "srv", "transport": "stdio", "command": "cmd", "args": ["a"]}, {"name": "srv2", "transport": "sse", "url": "http://..."}, {"name": "srv3", "transport": "http", "url": "https://...", "auth": {"type": "oauth_browser", "scope": ["read"], "client_id": "...", "client_secret": "...", "redirect_port": 48123, "token_cache": "/path/to/cache.json"}}] `oauth_browser` is supported for HTTP only through `--mcp-config` and structured JSON/TOML `mcp_servers` configuration; compact `--mcp-server` syntax does not support it. Protected-resource and discovered OAuth endpoints require HTTPS unless loopback. It prints an authorization URL only when no cached credential can provide a token; cached refresh tokens renew access on connection or reconnect without opening a browser. The callback binds to localhost on redirect_port (or an available port). See the README for headless authorization and cache-file security

- CLI flag: `--mcp-config`
- Environment: `MCP_V8_MCP_CONFIG`

### `mcp_stubs`

Expose upstream MCP server tools on the MCPJS server itself as `<prefix><server>__<tool>` stubs. When `true` (the default whenever at least one --mcp-server is configured), an external client of MCPJS can discover those tools via tools/list and tool search; calling a stub returns instructional text telling the caller to invoke the tool from JavaScript via run_js + mcp.callTool(...). Pass `--mcp-stubs false` to disable

- CLI flag: `--mcp-stubs`
- Environment: `MCP_V8_MCP_STUBS`
- Default: `true`
- Possible values: `true`, `false`
- Type: boolean

### `mcp_stub_prefix`

Prefix applied to stub tool names. Defaults to `runjs__` so it is obvious to a calling agent that these tools execute through the JS runtime rather than dispatching directly. Has no effect when --mcp-stubs is false

- CLI flag: `--mcp-stub-prefix`
- Environment: `MCP_V8_MCP_STUB_PREFIX`
- Default: `runjs__`

## Module Import

### `allow_external_modules`

Allow external module imports (npm:, jsr:, and URL imports). When disabled (the default), code using import declarations for external packages will be rejected. Enable with --allow-external-modules

- CLI flag: `--allow-external-modules`
- Environment: `MCP_V8_ALLOW_EXTERNAL_MODULES`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

## OS Sandbox

### `sandbox_manifest`

Confine the whole server process with an OS-enforced sandbox (nono: Landlock on Linux 5.13+, Seatbelt on macOS) described by a nono capability manifest — a JSON file path or inline JSON; in a --config file the structured `sandbox` section is the ergonomic form (and the NixOS module exposes it as a plain attrset). nono capabilities compose additively, so the enforced set is the manifest (verbatim, in nono's schema and semantics) unioned with a server baseline derived from this configuration: storage directories read-write, config/policy/WASM inputs and system paths read-only, and bind grants for configured listener ports. The manifest owns the network egress posture — the baseline never opens outbound access. Applied before the async runtime and V8 spawn any threads; irreversible once applied. This is defense in depth *underneath* the OPA policy layer. Fails closed: startup aborts if the platform cannot enforce the composed set, and manifest features that need nono's CLI supervisor (network proxy mode, credentials, rollback, fs deny rules) are rejected rather than silently ignored

- CLI flag: `--sandbox-manifest`
- Environment: `MCP_V8_SANDBOX_MANIFEST`

## Policy

### `policies_json`

JSON policy configuration (inline JSON or path to a JSON file). Enables fetch() and/or module policy gating via local Rego files and/or remote OPA servers. Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}' Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }

- CLI flag: `--policies-json`
- Environment: `MCP_V8_POLICIES_JSON`

## Prompt

### `instructions`

Override the MCP server `instructions` (the "system prompt" the server reports to clients during `initialize`). The value is used verbatim as inline text, unless it begins with `@`, in which case the remainder is treated as a path to a file whose contents are used (`@-` is not special; use `@@` for a literal leading `@`). Examples: --instructions "Run JS for me" --instructions @./prompt.txt

- CLI flag: `--instructions`
- Environment: `MCP_V8_INSTRUCTIONS`

### `run_js_description`

Override the description advertised for the `run_js` tool in `tools/list`. The value is used verbatim as inline text, unless it begins with `@`, in which case the remainder is treated as a path to a file whose contents are used (use `@@` for a literal leading `@`). Examples: --run-js-description "Execute JS" --run-js-description @./run_js.md

- CLI flag: `--run-js-description`
- Environment: `MCP_V8_RUN_JS_DESCRIPTION`

## Run JS File

### `allow_run_js_file`

Allow the `run_js` tool to read its code from a file on the server's own filesystem (the `file` parameter). OFF by default. When set, ANY path the server process can read is allowed — this is the easy "allow all" switch. For finer control, leave this off and configure a `run_js_file` policy in --policies-json instead (a Rego/OPA chain decides which paths are allowed); the policy input is `{ "operation": "read", "path": "<canonical path>" }`. This flag takes precedence over a configured run_js_file policy

- CLI flag: `--allow-run-js-file`
- Environment: `MCP_V8_ALLOW_RUN_JS_FILE`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

## Sandbox

### `harden_freeze_ops`

Freeze `Deno.core.ops` so user code cannot replace/intercept any op (e.g. a persistent trojan op surviving in stateful/snapshot mode)

- CLI flag: `--harden-freeze-ops`
- Environment: `MCP_V8_HARDEN_FREEZE_OPS`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

### `harden_neutralize_proxy_details`

Neutralize `op_get_proxy_details` (otherwise it bypasses `Proxy` handlers and can read a proxied target)

- CLI flag: `--harden-neutralize-proxy-details`
- Environment: `MCP_V8_HARDEN_NEUTRALIZE_PROXY_DETAILS`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

### `harden_neutralize_introspection`

Neutralize `op_memory_usage` + `op_is_terminal` (host info leaks)

- CLI flag: `--harden-neutralize-introspection`
- Environment: `MCP_V8_HARDEN_NEUTRALIZE_INTROSPECTION`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

### `harden_remove_bootstrap`

Remove `globalThis.__bootstrap` (event-loop hooks, primordials such as a pristine `Function` constructor, and internal registries)

- CLI flag: `--harden-remove-bootstrap`
- Environment: `MCP_V8_HARDEN_REMOVE_BOOTSTRAP`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

### `harden_remove_shared_memory`

Remove `globalThis.SharedArrayBuffer` + `globalThis.Atomics` — the high-resolution Spectre-timer prerequisite. NOTE: these are also the shared-memory primitives emscripten wasm-threads require, so leave this OFF to run pthreads-based WASM modules

- CLI flag: `--harden-remove-shared-memory`
- Environment: `MCP_V8_HARDEN_REMOVE_SHARED_MEMORY`
- Default: `false`
- Possible values: `true`, `false`
- Type: boolean

## Storage (S3)

### `s3_bucket`

S3 bucket backing whichever axes select `s3`. Required when `--heap-store s3` or `--fs-store s3` is set

- CLI flag: `--s3-bucket`
- Environment: `MCP_V8_S3_BUCKET`

### `cache_dir`

Local filesystem cache directory for S3 write-through caching (only used with `--s3-bucket`)

- CLI flag: `--cache-dir`
- Environment: `MCP_V8_CACHE_DIR`

## WASM

### `wasm_config`

JSON config mapping global names to .wasm file paths or objects (a path to a JSON file, or inline JSON — also settable as the `wasm` section of a --config file). String value: {"name": "/path/to/module.wasm"} Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216, "description": "what the module does"}} The optional "description" sets the MCP stub tool's description. NOTE: incompatible with heap persistence (`--heap-store` other than none)

- CLI flag: `--wasm-config`
- Environment: `MCP_V8_WASM_CONFIG`

### `wasm_default_max_memory`

Default max native memory for WASM modules without a per-module limit. Supports suffixes: k/K (KiB), m/M (MiB), g/G (GiB), or raw bytes. This is separate from --heap-memory-max (JS heap); WASM linear memory is allocated as native memory outside the V8 heap

- CLI flag: `--wasm-default-max-memory`
- Environment: `MCP_V8_WASM_DEFAULT_MAX_MEMORY`
- Default: `16m`

### `wasm_stubs`

Expose pre-loaded WASM modules on the MCPJS server itself as `<prefix>wasm__<name>` stubs. When `true` (the default whenever at least one WASM module is loaded), an external client of MCPJS can discover the module via tools/list and tool search; calling a stub returns instructional text telling the caller to use the module from JavaScript via run_js (the module is available as the `__wasm_<name>` global). Pass `--wasm-stubs false` to disable

- CLI flag: `--wasm-stubs`
- Environment: `MCP_V8_WASM_STUBS`
- Default: `true`
- Possible values: `true`, `false`
- Type: boolean

### `wasm_stub_prefix`

Prefix applied to WASM stub tool names. Defaults to `runjs__` so it is obvious to a calling agent that these modules execute through the JS runtime rather than dispatching directly. Has no effect when --wasm-stubs is false

- CLI flag: `--wasm-stub-prefix`
- Environment: `MCP_V8_WASM_STUB_PREFIX`
- Default: `runjs__`

## Keys not available in config files

- `config` — a config file cannot chain-load another config file.
- `print_openapi` — run `--print-openapi` on the command line instead.
- `wasm_modules` — use the `wasm` section (or a `wasm_config` path) instead.
- `wasm_stub_descriptions` — set `description` on entries in the `wasm` section instead.

## Example

```toml
# /etc/mcp-v8/server.toml
http_port = 8080
heap_store = "dir"
heap_dir = "/var/lib/mcp-v8/heaps"
execution_timeout = 60

[wasm.math]
path = "/opt/modules/math.wasm"
description = "Adds two numbers"

[[mcp_servers]]
name = "weather"
transport = "stdio"
command = "python"
args = ["server.py"]

[[fetch_headers]]
host = "api.github.com"
headers = { Authorization = "Bearer ..." }

[policies.fetch]
policies = [{ url = "file:///etc/mcp-v8/fetch.rego" }]
```
