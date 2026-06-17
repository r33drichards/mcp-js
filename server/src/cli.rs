use clap::Parser;

use crate::engine::DEFAULT_EXECUTION_TIMEOUT_SECS;

fn default_max_concurrent() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// Backend selection for an independent state axis (heap or filesystem).
///
/// `none` disables that axis entirely; `dir` uses a node-local directory; `s3`
/// uses the shared `--s3-bucket` (optionally fronted by `--cache-dir`). Heap and
/// filesystem persistence are independent: any combination of the two is valid
/// (neither / heap-only / fs-only / both).

pub enum StoreKind {
    None,
    Dir,
    S3,
}

impl std::fmt::Display for StoreKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StoreKind::None => "none",
            StoreKind::Dir => "dir",
            StoreKind::S3 => "s3",
        };
        f.write_str(s)
    }
}

/// Command line arguments. Heap snapshots and the content-addressed filesystem
/// are two independent, opt-in axes of per-session state (see `StoreKind`).
///
/// Every flag is also bindable from an `MCP_V8_*` environment variable
/// (precedence: explicit CLI flag > env var > default).


pub struct Cli {
    /// Print the OpenAPI JSON specification to stdout and exit.
    /// Use this to regenerate openapi.json: `./server --print-openapi > openapi.json`
    "Core"
    pub print_openapi: bool,

    /// JWKS endpoint URL for fetching public keys (e.g., Keycloak OIDC certs URL).
    /// Enables JWT verification of Authorization: Bearer tokens during initialize.
    "JWKS_URL""Core"
    pub jwks_url: Option<String>,

    /// HTTP port using Streamable HTTP transport (MCP 2025-03-26+, load-balanceable)
    "MCP_V8_HTTP_PORT""sse_port""Core"
    pub http_port: Option<u16>,

    /// SSE port using the older HTTP+SSE transport
    "MCP_V8_SSE_PORT""http_port""Core"
    pub sse_port: Option<u16>,

    /// Host/address the HTTP and SSE transports bind to. Defaults to all IPv4
    /// interfaces (0.0.0.0). Set to "::" for a dual-stack IPv6 listener, which is
    /// required to be reachable over IPv6-resolving private networks (e.g. Railway).
    "MCP_V8_BIND_HOST""0.0.0.0""Core"
    pub bind_host: String,

    /// Maximum V8 heap memory per isolate in megabytes (default: 8)
    "MCP_V8_HEAP_MEMORY_MAX""8""Core"
    pub heap_memory_max: u64,

    /// Maximum execution timeout in seconds (default: 30, max: 300)
    "MCP_V8_EXECUTION_TIMEOUT""Core"
    pub execution_timeout: u64,

    /// Maximum concurrent V8 executions (default: CPU core count)
    "MCP_V8_MAX_CONCURRENT_EXECUTIONS""Core"
    pub max_concurrent_executions: usize,

    /// Path to the sled database for the session log (per-session heap+fs
    /// history) and the execution registry. Also the default parent for the
    /// heap-tag store, fs blob store, and fs label db. Default: /tmp/mcp-v8-sessions.
    "MCP_V8_SESSION_DB_PATH""/tmp/mcp-v8-sessions""Core"
    pub session_db_path: String,

        /// V8 heap-snapshot backend. `none` (default) = no heap persistence; JS
    /// globals do NOT survive between runs. `dir` = node-local directory
    /// (`--heap-dir`). `s3` = shared `--s3-bucket` (optionally `--cache-dir`).
    ///
    /// Heap snapshots require a V8 SnapshotCreator isolate, which disables
    /// WebAssembly — so heap persistence is mutually exclusive with
    /// `--wasm-module`/`--wasm-config` (rejected at startup).
    "MCP_V8_HEAP_STORE""Heap"
    pub heap_store: StoreKind,

    /// Directory for the heap-snapshot store when `--heap-store dir`.
    /// Defaults to /tmp/mcp-v8-heaps.
    "MCP_V8_HEAP_DIR""DIR""Heap"
    pub heap_dir: Option<String>,

        /// Content-addressed `/work` filesystem backend. `none` (default) = no fs
    /// persistence. `dir` = node-local blob store (`--fs-dir`). `s3` = shared
    /// `--s3-bucket`. When enabled, the `fs` parameter of run_js can mount a
    /// snapshot (by label or CA id) and the `fs_*` tools / `/api/fs/...`
    /// endpoints become functional. Works with any isolate (compatible with
    /// `--wasm-module`).
    ///
    /// In cluster mode labels replicate cluster-wide, but blobs/manifests are
    /// only shared when stored on shared storage — so `--fs-store s3` is required
    /// when running fs persistence in a cluster.
    "MCP_V8_FS_STORE""Filesystem"
    pub fs_store: StoreKind,

    /// Directory for the fs snapshot blob store (chunks + manifests) when
    /// `--fs-store dir`. Defaults to `<session-db-path>/fs-blobs`.
    "fs-dir""MCP_V8_FS_DIR""DIR""Filesystem"
    pub fs_dir: Option<String>,

    /// Path for the fs label/reflog database (sled). Defaults to
    /// `<session-db-path>/fs-labels`.
    "fs-labels-db""MCP_V8_FS_LABELS_DB""PATH""Filesystem"
    pub fs_labels_db: Option<String>,

    /// Overlay read behaviour when a per-session fs snapshot is mounted.
    /// Off (default): overlay-only — the mounted snapshot is the entire fs view,
    /// so a read that misses it is ENOENT (strict isolation). On: overlayfs-style
    /// — fall through to the real filesystem as a read-only lower layer (still
    /// gated by the filesystem policy), so bundled read-only paths like
    /// `/opt/languages` resolve while `/work` stays the per-session overlay.
    "fs-passthrough""MCP_V8_FS_PASSTHROUGH""false""Filesystem"
    pub fs_passthrough: bool,

        /// Freeze `Deno.core.ops` so user code cannot replace/intercept any op
    /// (e.g. a persistent trojan op surviving in stateful/snapshot mode).
    "harden-freeze-ops""MCP_V8_HARDEN_FREEZE_OPS""false""Sandbox"
    pub harden_freeze_ops: bool,

    /// Neutralize `op_get_proxy_details` (otherwise it bypasses `Proxy` handlers
    /// and can read a proxied target).
    "harden-neutralize-proxy-details""MCP_V8_HARDEN_NEUTRALIZE_PROXY_DETAILS""false""Sandbox"
    pub harden_neutralize_proxy_details: bool,

    /// Neutralize `op_memory_usage` + `op_is_terminal` (host info leaks).
    "harden-neutralize-introspection""MCP_V8_HARDEN_NEUTRALIZE_INTROSPECTION""false""Sandbox"
    pub harden_neutralize_introspection: bool,

    /// Remove `globalThis.__bootstrap` (event-loop hooks, primordials such as a
    /// pristine `Function` constructor, and internal registries).
    "harden-remove-bootstrap""MCP_V8_HARDEN_REMOVE_BOOTSTRAP""false""Sandbox"
    pub harden_remove_bootstrap: bool,

    /// Remove `globalThis.SharedArrayBuffer` + `globalThis.Atomics` — the
    /// high-resolution Spectre-timer prerequisite. NOTE: these are also the
    /// shared-memory primitives emscripten wasm-threads require, so leave this
    /// OFF to run pthreads-based WASM modules.
    "harden-remove-shared-memory""MCP_V8_HARDEN_REMOVE_SHARED_MEMORY""false""Sandbox"
    pub harden_remove_shared_memory: bool,

        /// S3 bucket backing whichever axes select `s3`. Required when
    /// `--heap-store s3` or `--fs-store s3` is set.
    "MCP_V8_S3_BUCKET""Storage (S3)"
    pub s3_bucket: Option<String>,

    /// Local filesystem cache directory for S3 write-through caching (only used
    /// with `--s3-bucket`).
    "MCP_V8_CACHE_DIR""s3_bucket""Storage (S3)"
    pub cache_dir: Option<String>,

    /// Override the MCP server `instructions` (the "system prompt" the server
    /// reports to clients during `initialize`). The value is used verbatim as
    /// inline text, unless it begins with `@`, in which case the remainder is
    /// treated as a path to a file whose contents are used (`@-` is not special;
    /// use `@@` for a literal leading `@`).
    /// Examples: --instructions "Run JS for me"  --instructions @./prompt.txt
    "MCP_V8_INSTRUCTIONS""TEXT_OR_@FILE""Prompt"
    pub instructions: Option<String>,

    /// Override the description advertised for the `run_js` tool in `tools/list`.
    /// The value is used verbatim as inline text, unless it begins with `@`, in
    /// which case the remainder is treated as a path to a file whose contents are
    /// used (use `@@` for a literal leading `@`).
    /// Examples: --run-js-description "Execute JS"  --run-js-description @./run_js.md
    "run-js-description""MCP_V8_RUN_JS_DESCRIPTION""TEXT_OR_@FILE""Prompt"
    pub run_js_description: Option<String>,

    /// Port for the Raft cluster HTTP server. Enables cluster mode when set.
    "MCP_V8_CLUSTER_PORT""Cluster"
    pub cluster_port: Option<u16>,

    /// Unique node identifier within the cluster
    "MCP_V8_NODE_ID""node1""Cluster"
    pub node_id: String,

    /// Comma-separated list of seed peer addresses. Format: id@host:port or host:port.
    /// Peers can also join dynamically via POST /raft/join.
    "MCP_V8_PEERS""Cluster"
    pub peers: Vec<String>,

    /// Join an existing cluster by contacting this seed address (host:port).
    /// The node will register itself with the cluster leader via /raft/join.
    "MCP_V8_JOIN""Cluster"
    pub join: Option<String>,

    /// Join as a non-voting learner: the node replicates the log but is
    /// excluded from election and commit quorums and never starts elections.
    /// Use for ephemeral nodes whose churn must not affect availability.
    "MCP_V8_JOIN_AS_LEARNER""Cluster"
    pub join_as_learner: bool,

    /// Advertise address for this node (host:port). Used for peer discovery
    /// and write forwarding. Defaults to <node-id>:<cluster-port>.
    "MCP_V8_ADVERTISE_ADDR""Cluster"
    pub advertise_addr: Option<String>,

    /// Heartbeat interval in milliseconds
    "MCP_V8_HEARTBEAT_INTERVAL""100""Cluster"
    pub heartbeat_interval: u64,

    /// Minimum election timeout in milliseconds
    "MCP_V8_ELECTION_TIMEOUT_MIN""300""Cluster"
    pub election_timeout_min: u64,

    /// Maximum election timeout in milliseconds
    "MCP_V8_ELECTION_TIMEOUT_MAX""500""Cluster"
    pub election_timeout_max: u64,

    /// Pre-load a WASM module as a global. Format: name=/path/to/module.wasm[:max_memory]
    /// The module's exports will be available as a global variable with the given name.
    /// Optional memory suffix caps the module's native memory (linear memory + tables).
    /// Supported suffixes: raw bytes, k/K (KiB), m/M (MiB), g/G (GiB).
    /// Examples: math=/path.wasm  math=/path.wasm:16m  math=/path.wasm:1048576
    /// Can be specified multiple times for multiple modules.
    /// NOTE: incompatible with heap persistence (`--heap-store` other than none).
    "wasm-module""NAME=PATH[:LIMIT]""WASM"
    pub wasm_modules: Vec<String>,

    /// Path to a JSON config file mapping global names to .wasm file paths or objects.
    /// String value: {"name": "/path/to/module.wasm"}
    /// Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216, "description": "what the module does"}}
    /// The optional "description" sets the MCP stub tool's description.
    /// NOTE: incompatible with heap persistence (`--heap-store` other than none).
    "wasm-config""MCP_V8_WASM_CONFIG""PATH""WASM"
    pub wasm_config: Option<String>,

    /// Default max native memory for WASM modules without a per-module limit.
    /// Supports suffixes: k/K (KiB), m/M (MiB), g/G (GiB), or raw bytes.
    /// This is separate from --heap-memory-max (JS heap); WASM linear memory
    /// is allocated as native memory outside the V8 heap.
    "wasm-default-max-memory""MCP_V8_WASM_DEFAULT_MAX_MEMORY""16m""WASM"
    pub wasm_default_max_memory: String,

    /// Expose pre-loaded WASM modules on the MCPJS server itself as
    /// `<prefix>wasm__<name>` stubs. When `true` (the default whenever at
    /// least one WASM module is loaded), an external client of MCPJS can
    /// discover the module via tools/list and tool search; calling a stub
    /// returns instructional text telling the caller to use the module from
    /// JavaScript via run_js (the module is available as the `__wasm_<name>`
    /// global). Pass `--wasm-stubs false` to disable.
    "wasm-stubs""MCP_V8_WASM_STUBS""true""WASM"
    pub wasm_stubs: bool,

    /// Prefix applied to WASM stub tool names. Defaults to `runjs__` so it is
    /// obvious to a calling agent that these modules execute through the JS
    /// runtime rather than dispatching directly. Has no effect when
    /// --wasm-stubs is false.
    "wasm-stub-prefix""MCP_V8_WASM_STUB_PREFIX""WASM"
    pub wasm_stub_prefix: String,

    /// Set the MCP stub tool description for a loaded WASM module. Format:
    /// name=description text. The text is shown to downstream agents alongside
    /// the auto-generated usage hint (globals, exports, instantiation), helping
    /// them decide when to use the module. Can be specified multiple times.
    /// Overrides a "description" set inline via --wasm-config. The named module
    /// must be loaded with --wasm-module or --wasm-config.
    "wasm-stub-description""NAME=TEXT""WASM"
    pub wasm_stub_descriptions: Vec<String>,

    /// Inject headers into fetch requests matching host/method rules.
    /// Format: host=<host>,header=<name>,value=<val>[,methods=GET;POST]
    /// Can be specified multiple times.
    "fetch-header""RULE""Fetch"
    pub fetch_headers: Vec<String>,

    /// Path to a JSON file with header injection rules.
    /// Format: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}]
    "fetch-header-config""MCP_V8_FETCH_HEADER_CONFIG""PATH""Fetch"
    pub fetch_header_config: Option<String>,

    /// Allow external module imports (npm:, jsr:, and URL imports).
    /// When disabled (the default), code using import declarations for external
    /// packages will be rejected. Enable with --allow-external-modules.
    "allow-external-modules""MCP_V8_ALLOW_EXTERNAL_MODULES""false""Module Import"
    pub allow_external_modules: bool,

    /// Allow the `run_js` tool to read its code from a file on the server's own
    /// filesystem (the `file` parameter). OFF by default. When set, ANY path the
    /// server process can read is allowed — this is the easy "allow all" switch.
    /// For finer control, leave this off and configure a `run_js_file` policy in
    /// --policies-json instead (a Rego/OPA chain decides which paths are allowed);
    /// the policy input is `{ "operation": "read", "path": "<canonical path>" }`.
    /// This flag takes precedence over a configured run_js_file policy.
    "allow-run-js-file""MCP_V8_ALLOW_RUN_JS_FILE""false""Run JS File"
    pub allow_run_js_file: bool,

    /// JSON policy configuration (inline JSON or path to a JSON file).
    /// Enables fetch() and/or module policy gating via local Rego files
    /// and/or remote OPA servers.
    ///
    /// Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}'
    ///
    /// Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }
    "policies-json""MCP_V8_POLICIES_JSON""JSON_OR_PATH""Policy"
    pub policies_json: Option<String>,

    /// Connect to an external MCP server as a module. JS code can call its tools
    /// via the `mcp` global object (mcp.callTool, mcp.listTools, mcp.servers).
    /// Format for stdio: name=stdio:command:arg1:arg2
    /// Format for SSE:   name=sse:url
    /// Can be specified multiple times for multiple servers.
    "mcp-server""NAME=TRANSPORT:...""MCP Server Module"
    pub mcp_servers: Vec<String>,

    /// Path to a JSON config file for MCP server modules.
    /// Format: [{"name": "srv", "transport": "stdio", "command": "cmd", "args": ["a"]},
    ///          {"name": "srv2", "transport": "sse", "url": "http://..."}]
    "mcp-config""MCP_V8_MCP_CONFIG""PATH""MCP Server Module"
    pub mcp_config: Option<String>,

    /// Expose upstream MCP server tools on the MCPJS server itself as
    /// `<prefix><server>__<tool>` stubs. When `true` (the default whenever
    /// at least one --mcp-server is configured), an external client of
    /// MCPJS can discover those tools via tools/list and tool search;
    /// calling a stub returns instructional text telling the caller to
    /// invoke the tool from JavaScript via run_js + mcp.callTool(...).
    /// Pass `--mcp-stubs false` to disable.
    "mcp-stubs""MCP_V8_MCP_STUBS""true""MCP Server Module"
    pub mcp_stubs: bool,

    /// Prefix applied to stub tool names. Defaults to `runjs__` so it is
    /// obvious to a calling agent that these tools execute through the JS
    /// runtime rather than dispatching directly. Has no effect when
    /// --mcp-stubs is false.
    "mcp-stub-prefix""MCP_V8_MCP_STUB_PREFIX""MCP Server Module"
    pub mcp_stub_prefix: String,
}

impl Cli {
    /// True when heap persistence is configured (`--heap-store` != none).
    pub fn heap_enabled(&self) -> bool {
        self.heap_store != StoreKind::None
    }

    /// True when filesystem persistence is configured (`--fs-store` != none).
    pub fn fs_enabled(&self) -> bool {
        self.fs_store != StoreKind::None
    }
}
