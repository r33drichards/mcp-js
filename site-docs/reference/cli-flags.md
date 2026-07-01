# CLI Flags

> Generated from the Clap `Cli` definition. Do not edit this page by hand.

`mcp-v8` is configured through command-line flags. This page is grouped
using the same help headings exposed by the CLI itself.

## Sections

- [Cluster](#cluster)
- [Core](#core)
- [Fetch](#fetch)
- [Filesystem](#filesystem)
- [Heap](#heap)
- [MCP Server Module](#mcp-server-module)
- [Module Import](#module-import)
- [Policy](#policy)
- [Prompt](#prompt)
- [Run JS File](#run-js-file)
- [Sandbox](#sandbox)
- [Storage (S3)](#storage-s3)
- [WASM](#wasm)

## Cluster

### `--cluster-port`

Port for the Raft cluster HTTP server. Enables cluster mode when set

- Environment: `MCP_V8_CLUSTER_PORT`
- Value: `CLUSTER_PORT`

### `--node-id`

Unique node identifier within the cluster

- Environment: `MCP_V8_NODE_ID`
- Default: `node1`
- Value: `NODE_ID`

### `--join`

Join an existing cluster by contacting this seed address (host:port). The node will register itself with the cluster leader via /raft/join

- Environment: `MCP_V8_JOIN`
- Value: `JOIN`

### `--join-as-learner`

Join as a non-voting learner: the node replicates the log but is excluded from election and commit quorums and never starts elections. Use for ephemeral nodes whose churn must not affect availability

- Environment: `MCP_V8_JOIN_AS_LEARNER`

### `--advertise-addr`

Advertise address for this node (host:port). Used for peer discovery and write forwarding. Defaults to <node-id>:<cluster-port>

- Environment: `MCP_V8_ADVERTISE_ADDR`
- Value: `ADVERTISE_ADDR`

### `--heartbeat-interval`

Heartbeat interval in milliseconds

- Environment: `MCP_V8_HEARTBEAT_INTERVAL`
- Default: `100`
- Value: `HEARTBEAT_INTERVAL`

### `--election-timeout-min`

Minimum election timeout in milliseconds

- Environment: `MCP_V8_ELECTION_TIMEOUT_MIN`
- Default: `300`
- Value: `ELECTION_TIMEOUT_MIN`

### `--election-timeout-max`

Maximum election timeout in milliseconds

- Environment: `MCP_V8_ELECTION_TIMEOUT_MAX`
- Default: `500`
- Value: `ELECTION_TIMEOUT_MAX`

### `--peers`

Comma-separated list of seed peer addresses. Peers can also join dynamically via POST /raft/join. Forms: id@host:port — peer address with an explicit node id host:port — peer address only (node id learned on join) Examples: node2@10.0.0.2:4000 10.0.0.3:4000

- Environment: `MCP_V8_PEERS`
- Value: `PEERS`
- Delimiter: `,`
- Repeatable: yes

## Core

### `--print-openapi`

Print the OpenAPI JSON specification to stdout and exit. Use this to regenerate openapi.json: `./server --print-openapi > openapi.json`

### `--jwks-url`

JWKS endpoint URL for fetching public keys (e.g., Keycloak OIDC certs URL). Enables JWT verification of Authorization: Bearer tokens during initialize

- Environment: `JWKS_URL`
- Value: `JWKS_URL`

### `--http-port`

HTTP port using Streamable HTTP transport (MCP 2025-03-26+, load-balanceable)

- Environment: `MCP_V8_HTTP_PORT`
- Value: `HTTP_PORT`

### `--sse-port`

SSE port using the legacy HTTP+SSE transport (served by a vendored rmcp 0.1.5; no MCP tasks support — use --http-port for tasks)

- Environment: `MCP_V8_SSE_PORT`
- Value: `SSE_PORT`

### `--bind-host`

Host/address the HTTP and SSE transports bind to. Defaults to all IPv4 interfaces (0.0.0.0). Set to "::" for a dual-stack IPv6 listener, which is required to be reachable over IPv6-resolving private networks (e.g. Railway)

- Environment: `MCP_V8_BIND_HOST`
- Default: `0.0.0.0`
- Value: `BIND_HOST`

### `--heap-memory-max`

Maximum V8 heap memory per isolate in megabytes (default: 8)

- Environment: `MCP_V8_HEAP_MEMORY_MAX`
- Default: `8`
- Value: `HEAP_MEMORY_MAX`

### `--execution-timeout`

Maximum execution timeout in seconds (default: 30, max: 300)

- Environment: `MCP_V8_EXECUTION_TIMEOUT`
- Default: `30`
- Value: `EXECUTION_TIMEOUT`

### `--max-concurrent-executions`

Maximum concurrent V8 executions (default: CPU core count)

- Environment: `MCP_V8_MAX_CONCURRENT_EXECUTIONS`
- Value: `MAX_CONCURRENT_EXECUTIONS`

### `--session-db-path`

Path to the sled database for the session log (per-session heap+fs history) and the execution registry. Also the default parent for the heap-tag store, fs blob store, and fs label db. Default: /tmp/mcp-v8-sessions

- Environment: `MCP_V8_SESSION_DB_PATH`
- Default: `/tmp/mcp-v8-sessions`
- Value: `SESSION_DB_PATH`

## Fetch

### `--fetch-header-config`

Path to a JSON file with header injection rules. Each rule sets "host" (plus optional "methods") and exactly one of "headers" or "auth". Static: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}] OAuth: [{"host": "api.example.com", "auth": {"type": "oauth_client_credentials", "header": "Authorization", "token_url": "https://issuer.example.com/token", "client_id": "abc", "client_secret": "xyz", "scope": "read:all", "refresh_buffer_secs": 30}}]

- Environment: `MCP_V8_FETCH_HEADER_CONFIG`
- Value: `PATH`

### `--fetch-header`

Inject a header into fetch requests that match host/method rules. Each rule is a comma-separated list of key=value pairs and must use either the static value form or the OAuth client-credentials form (mutually exclusive). Can be specified multiple times. Accepted keys: host — host pattern the request URL must match (required) methods — semicolon-separated HTTP methods to match, e.g. GET;POST (optional) header — name of the header to inject (required) value — static header value (static form) token_url — OAuth token endpoint URL (OAuth form) client_id — OAuth client id (OAuth form) client_secret — OAuth client secret (OAuth form) scope — OAuth scope (OAuth form, optional) refresh_buffer_secs — seconds before expiry to refresh the token, default 30 (OAuth form, optional)

- Value: `RULE`
- Repeatable: yes

## Filesystem

### `--fs-store`

Content-addressed `/work` filesystem backend. `none` (default) = no fs persistence. `dir` = node-local blob store (`--fs-dir`). `s3` = shared `--s3-bucket`. When enabled, the `fs` parameter of run_js can mount a snapshot (by label or CA id) and the `fs_*` tools / `/api/fs/...` endpoints become functional. Works with any isolate (compatible with `--wasm-module`). In cluster mode labels replicate cluster-wide, but blobs/manifests are only shared when stored on shared storage — so `--fs-store s3` is required when running fs persistence in a cluster.

- Environment: `MCP_V8_FS_STORE`
- Default: `none`
- Value: `FS_STORE`

### `--fs-dir`

Directory for the fs snapshot blob store (chunks + manifests) when `--fs-store dir`. Defaults to `<session-db-path>/fs-blobs`

- Environment: `MCP_V8_FS_DIR`
- Value: `DIR`

### `--fs-labels-db`

Path for the fs label/reflog database (sled). Defaults to `<session-db-path>/fs-labels`

- Environment: `MCP_V8_FS_LABELS_DB`
- Value: `PATH`

### `--fs-passthrough`

Overlay read behaviour when a per-session fs snapshot is mounted. Off (default): overlay-only — the mounted snapshot is the entire fs view, so a read that misses it is ENOENT (strict isolation). On: overlayfs-style — fall through to the real filesystem as a read-only lower layer (still gated by the filesystem policy), so bundled read-only paths like `/opt/languages` resolve while `/work` stays the per-session overlay

- Environment: `MCP_V8_FS_PASSTHROUGH`
- Default: `false`

## Heap

### `--heap-store`

V8 heap-snapshot backend. `none` (default) = no heap persistence; JS globals do NOT survive between runs. `dir` = node-local directory (`--heap-dir`). `s3` = shared `--s3-bucket` (optionally `--cache-dir`). Heap snapshots require a V8 SnapshotCreator isolate, which disables WebAssembly — so heap persistence is mutually exclusive with `--wasm-module`/`--wasm-config` (rejected at startup).

- Environment: `MCP_V8_HEAP_STORE`
- Default: `none`
- Value: `HEAP_STORE`

### `--heap-dir`

Directory for the heap-snapshot store when `--heap-store dir`. Defaults to /tmp/mcp-v8-heaps

- Environment: `MCP_V8_HEAP_DIR`
- Value: `DIR`

## MCP Server Module

### `--mcp-server`

Connect to an external MCP server as a module. JS code can call its tools via the `mcp` global object (mcp.callTool, mcp.listTools, mcp.servers). Format for stdio: name=stdio:command:arg1:arg2 Format for SSE: name=sse:url Format for HTTP: name=http:url Can be specified multiple times for multiple servers. Note: authentication requires --mcp-config JSON file (too complex for CLI)

- Value: `NAME=TRANSPORT:...`
- Repeatable: yes


### `--mcp-config`

Path to a JSON config file for MCP server modules. Supports auth. Format: [{"name": "srv", "transport": "stdio", "command": "cmd", "args": ["a"]}, {"name": "srv2", "transport": "sse", "url": "http://..."}, {"name": "srv3", "transport": "http", "url": "https://mcp.example.com/mcp", "auth": {"type": "bearer", "token": "sk-..."}}, {"name": "srv4", "transport": "http", "url": "https://mcp.example.com/mcp", "auth": {"type": "client_credentials", "token_url": "https://auth.example.com/token", "client_id": "my-app", "client_secret": "secret", "scope": "mcp:read"}}, {"name": "srv5", "transport": "http", "url": "https://mcp.example.com/mcp", "auth": {"type": "oauth_discovery", "client_id": "my-app", "client_secret": "secret", "scope": ["mcp:read", "tools:call"]}}, {"name": "supabase", "transport": "http", "url": "https://mcp.supabase.com/mcp", "auth": {"type": "oauth_browser", "scope": ["projects:read", "database:read"]}}] Auth types: - bearer: static token, sent as Authorization: Bearer <token> - client_credentials: OAuth 2.0 client_credentials grant (you provide token_url) - oauth_discovery: full MCP OAuth flow (RFC 9728 + RFC 8414) — discovers the authorization server from the MCP server's Protected Resource Metadata, then performs client_credentials exchange. No token_url needed. - oauth_browser: interactive browser OAuth 2.1 authorization-code + PKCE flow with Dynamic Client Registration (RFC 7591) — for servers whose AS does NOT support client_credentials and requires a user sign-in (e.g. Supabase's hosted MCP server). Opens the default browser once, caches access + refresh tokens to a JSON file, and silently refreshes on later startups. Optional fields: scope (array), client_id, client_secret, redirect_port, token_cache (path)

- Environment: `MCP_V8_MCP_CONFIG`
- Value: `PATH`

### `--mcp-stubs`

Expose upstream MCP server tools on the MCPJS server itself as `<prefix><server>__<tool>` stubs. When `true` (the default whenever at least one --mcp-server is configured), an external client of MCPJS can discover those tools via tools/list and tool search; calling a stub returns instructional text telling the caller to invoke the tool from JavaScript via run_js + mcp.callTool(...). Pass `--mcp-stubs false` to disable

- Environment: `MCP_V8_MCP_STUBS`
- Default: `true`

### `--mcp-stub-prefix`

Prefix applied to stub tool names. Defaults to `runjs__` so it is obvious to a calling agent that these tools execute through the JS runtime rather than dispatching directly. Has no effect when --mcp-stubs is false

- Environment: `MCP_V8_MCP_STUB_PREFIX`
- Default: `runjs__`
- Value: `MCP_STUB_PREFIX`

### `--mcp-server`

Connect to an external MCP server as a module; JS can call its tools via the `mcp` global (mcp.callTool, mcp.listTools, mcp.servers). Can be specified multiple times. Transports: name=stdio:command:arg1:arg2 — spawn a stdio MCP server process name=sse:url — connect to an SSE MCP server endpoint Examples: weather=stdio:python:server.py remote=sse:http://localhost:9000/sse

- Value: `NAME=TRANSPORT:...`
- Repeatable: yes

## Module Import

### `--allow-external-modules`

Allow external module imports (npm:, jsr:, and URL imports). When disabled (the default), code using import declarations for external packages will be rejected. Enable with --allow-external-modules

- Environment: `MCP_V8_ALLOW_EXTERNAL_MODULES`
- Default: `false`

## Policy

### `--policies-json`

JSON policy configuration (inline JSON or path to a JSON file). Enables fetch() and/or module policy gating via local Rego files and/or remote OPA servers. Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}' Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }

- Environment: `MCP_V8_POLICIES_JSON`
- Value: `JSON_OR_PATH`

## Prompt

### `--instructions`

Override the MCP server `instructions` (the "system prompt" the server reports to clients during `initialize`). The value is used verbatim as inline text, unless it begins with `@`, in which case the remainder is treated as a path to a file whose contents are used (`@-` is not special; use `@@` for a literal leading `@`). Examples: --instructions "Run JS for me" --instructions @./prompt.txt

- Environment: `MCP_V8_INSTRUCTIONS`
- Value: `TEXT_OR_@FILE`

### `--run-js-description`

Override the description advertised for the `run_js` tool in `tools/list`. The value is used verbatim as inline text, unless it begins with `@`, in which case the remainder is treated as a path to a file whose contents are used (use `@@` for a literal leading `@`). Examples: --run-js-description "Execute JS" --run-js-description @./run_js.md

- Environment: `MCP_V8_RUN_JS_DESCRIPTION`
- Value: `TEXT_OR_@FILE`

## Run JS File

### `--allow-run-js-file`

Allow the `run_js` tool to read its code from a file on the server's own filesystem (the `file` parameter). OFF by default. When set, ANY path the server process can read is allowed — this is the easy "allow all" switch. For finer control, leave this off and configure a `run_js_file` policy in --policies-json instead (a Rego/OPA chain decides which paths are allowed); the policy input is `{ "operation": "read", "path": "<canonical path>" }`. This flag takes precedence over a configured run_js_file policy

- Environment: `MCP_V8_ALLOW_RUN_JS_FILE`
- Default: `false`

## Sandbox

### `--harden-freeze-ops`

Freeze `Deno.core.ops` so user code cannot replace/intercept any op (e.g. a persistent trojan op surviving in stateful/snapshot mode)

- Environment: `MCP_V8_HARDEN_FREEZE_OPS`
- Default: `false`

### `--harden-neutralize-proxy-details`

Neutralize `op_get_proxy_details` (otherwise it bypasses `Proxy` handlers and can read a proxied target)

- Environment: `MCP_V8_HARDEN_NEUTRALIZE_PROXY_DETAILS`
- Default: `false`

### `--harden-neutralize-introspection`

Neutralize `op_memory_usage` + `op_is_terminal` (host info leaks)

- Environment: `MCP_V8_HARDEN_NEUTRALIZE_INTROSPECTION`
- Default: `false`

### `--harden-remove-bootstrap`

Remove `globalThis.__bootstrap` (event-loop hooks, primordials such as a pristine `Function` constructor, and internal registries)

- Environment: `MCP_V8_HARDEN_REMOVE_BOOTSTRAP`
- Default: `false`

### `--harden-remove-shared-memory`

Remove `globalThis.SharedArrayBuffer` + `globalThis.Atomics` — the high-resolution Spectre-timer prerequisite. NOTE: these are also the shared-memory primitives emscripten wasm-threads require, so leave this OFF to run pthreads-based WASM modules

- Environment: `MCP_V8_HARDEN_REMOVE_SHARED_MEMORY`
- Default: `false`

## Storage (S3)

### `--s3-bucket`

S3 bucket backing whichever axes select `s3`. Required when `--heap-store s3` or `--fs-store s3` is set

- Environment: `MCP_V8_S3_BUCKET`
- Value: `S3_BUCKET`

### `--cache-dir`

Local filesystem cache directory for S3 write-through caching (only used with `--s3-bucket`)

- Environment: `MCP_V8_CACHE_DIR`
- Value: `CACHE_DIR`

## WASM

### `--wasm-config`

Path to a JSON config file mapping global names to .wasm file paths or objects. String value: {"name": "/path/to/module.wasm"} Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216, "description": "what the module does"}} The optional "description" sets the MCP stub tool's description. NOTE: incompatible with heap persistence (`--heap-store` other than none)

- Environment: `MCP_V8_WASM_CONFIG`
- Value: `PATH`

### `--wasm-default-max-memory`

Default max native memory for WASM modules without a per-module limit. Supports suffixes: k/K (KiB), m/M (MiB), g/G (GiB), or raw bytes. This is separate from --heap-memory-max (JS heap); WASM linear memory is allocated as native memory outside the V8 heap

- Environment: `MCP_V8_WASM_DEFAULT_MAX_MEMORY`
- Default: `16m`
- Value: `WASM_DEFAULT_MAX_MEMORY`

### `--wasm-stubs`

Expose pre-loaded WASM modules on the MCPJS server itself as `<prefix>wasm__<name>` stubs. When `true` (the default whenever at least one WASM module is loaded), an external client of MCPJS can discover the module via tools/list and tool search; calling a stub returns instructional text telling the caller to use the module from JavaScript via run_js (the module is available as the `__wasm_<name>` global). Pass `--wasm-stubs false` to disable

- Environment: `MCP_V8_WASM_STUBS`
- Default: `true`

### `--wasm-stub-prefix`

Prefix applied to WASM stub tool names. Defaults to `runjs__` so it is obvious to a calling agent that these modules execute through the JS runtime rather than dispatching directly. Has no effect when --wasm-stubs is false

- Environment: `MCP_V8_WASM_STUB_PREFIX`
- Default: `runjs__`
- Value: `WASM_STUB_PREFIX`

### `--wasm-module`

Pre-load a WASM module as a global named <name>; its exports become that global. An optional :max_memory suffix caps the module's native memory (linear memory + tables) with suffixes raw bytes, k/K (KiB), m/M (MiB), g/G (GiB). Can be specified multiple times. Incompatible with heap persistence (--heap-store other than none). Format: name=/path/to/module.wasm[:max_memory] — load <name> from a .wasm file, optionally capping its native memory Examples: math=/path.wasm math=/path.wasm:16m math=/path.wasm:1048576

- Value: `NAME=PATH[:LIMIT]`
- Repeatable: yes

### `--wasm-stub-description`

Set the MCP stub tool description for a loaded WASM module; the text is shown to downstream agents alongside the auto-generated usage hint. Overrides a `description` set inline via --wasm-config. The named module must be loaded with --wasm-module or --wasm-config. Can be specified multiple times. Format: name=description text — set <name>'s stub tool description Examples: math=Adds two numbers and returns the sum

- Value: `NAME=TEXT`
- Repeatable: yes
