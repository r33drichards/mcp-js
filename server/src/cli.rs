use clap::{CommandFactory, FromArgMatches, Parser};
use cli_derive::StructuredArgs;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
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
#[derive(Parser, StructuredArgs, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Print the OpenAPI JSON specification to stdout and exit.
    /// Use this to regenerate openapi.json: `./server --print-openapi > openapi.json`
    #[arg(long, help_heading = "Core")]
    pub print_openapi: bool,

    /// JWKS endpoint URL for fetching public keys (e.g., Keycloak OIDC certs URL).
    /// Enables JWT verification of Authorization: Bearer tokens during initialize.
    #[arg(long, env = "JWKS_URL", help_heading = "Core")]
    pub jwks_url: Option<String>,

    /// HTTP port using Streamable HTTP transport (MCP 2025-03-26+, load-balanceable)
    #[arg(long, env = "MCP_V8_HTTP_PORT", conflicts_with = "sse_port", help_heading = "Core")]
    pub http_port: Option<u16>,

    /// SSE port using the legacy HTTP+SSE transport (served by a vendored rmcp
    /// 0.1.5; no MCP tasks support — use --http-port for tasks).
    #[arg(long, env = "MCP_V8_SSE_PORT", conflicts_with = "http_port", help_heading = "Core")]
    pub sse_port: Option<u16>,

    /// Host/address the HTTP and SSE transports bind to. Defaults to all IPv4
    /// interfaces (0.0.0.0). Set to "::" for a dual-stack IPv6 listener, which is
    /// required to be reachable over IPv6-resolving private networks (e.g. Railway).
    #[arg(long, env = "MCP_V8_BIND_HOST", default_value = "0.0.0.0", help_heading = "Core")]
    pub bind_host: String,

    /// Maximum V8 heap memory per isolate in megabytes (default: 8)
    #[arg(
        long,
        env = "MCP_V8_HEAP_MEMORY_MAX",
        default_value = "8",
        value_parser = clap::value_parser!(u64).range(1..),
        help_heading = "Core"
    )]
    pub heap_memory_max: u64,

    /// Maximum execution timeout in seconds (default: 30, max: 300)
    #[arg(
        long,
        env = "MCP_V8_EXECUTION_TIMEOUT",
        default_value_t = DEFAULT_EXECUTION_TIMEOUT_SECS,
        value_parser = clap::value_parser!(u64).range(1..=300),
        help_heading = "Core"
    )]
    pub execution_timeout: u64,

    /// Maximum concurrent V8 executions (default: CPU core count)
    #[arg(long, env = "MCP_V8_MAX_CONCURRENT_EXECUTIONS", default_value_t = default_max_concurrent(), help_heading = "Core")]
    pub max_concurrent_executions: usize,

    /// Path to the sled database for the session log (per-session heap+fs
    /// history) and the execution registry. Also the default parent for the
    /// heap-tag store, fs blob store, and fs label db. Default: /tmp/mcp-v8-sessions.
    #[arg(long, env = "MCP_V8_SESSION_DB_PATH", default_value = "/tmp/mcp-v8-sessions", help_heading = "Core")]
    pub session_db_path: String,

    // ── Heap persistence (independent axis) ──────────────────────────────────
    /// V8 heap-snapshot backend. `none` (default) = no heap persistence; JS
    /// globals do NOT survive between runs. `dir` = node-local directory
    /// (`--heap-dir`). `s3` = shared `--s3-bucket` (optionally `--cache-dir`).
    ///
    /// Heap snapshots require a V8 SnapshotCreator isolate, which disables
    /// WebAssembly — so heap persistence is mutually exclusive with
    /// `--wasm-module`/`--wasm-config` (rejected at startup).
    #[arg(long, env = "MCP_V8_HEAP_STORE", value_enum, default_value_t = StoreKind::None, help_heading = "Heap")]
    pub heap_store: StoreKind,

    /// Directory for the heap-snapshot store when `--heap-store dir`.
    /// Defaults to /tmp/mcp-v8-heaps.
    #[arg(long, env = "MCP_V8_HEAP_DIR", value_name = "DIR", help_heading = "Heap")]
    pub heap_dir: Option<String>,

    // ── Filesystem persistence (independent axis) ────────────────────────────
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
    #[arg(long, env = "MCP_V8_FS_STORE", value_enum, default_value_t = StoreKind::None, help_heading = "Filesystem")]
    pub fs_store: StoreKind,

    /// Directory for the fs snapshot blob store (chunks + manifests) when
    /// `--fs-store dir`. Defaults to `<session-db-path>/fs-blobs`.
    #[arg(long = "fs-dir", env = "MCP_V8_FS_DIR", value_name = "DIR", help_heading = "Filesystem")]
    pub fs_dir: Option<String>,

    /// Path for the fs label/reflog database (sled). Defaults to
    /// `<session-db-path>/fs-labels`.
    #[arg(long = "fs-labels-db", env = "MCP_V8_FS_LABELS_DB", value_name = "PATH", help_heading = "Filesystem")]
    pub fs_labels_db: Option<String>,

    /// Overlay read behaviour when a per-session fs snapshot is mounted.
    /// Off (default): overlay-only — the mounted snapshot is the entire fs view,
    /// so a read that misses it is ENOENT (strict isolation). On: overlayfs-style
    /// — fall through to the real filesystem as a read-only lower layer (still
    /// gated by the filesystem policy), so bundled read-only paths like
    /// `/opt/languages` resolve while `/work` stays the per-session overlay.
    #[arg(long = "fs-passthrough", env = "MCP_V8_FS_PASSTHROUGH", default_value = "false", help_heading = "Filesystem")]
    pub fs_passthrough: bool,

    // ── Sandbox hardening (all opt-in; OFF by default → unhardened) ───────────
    /// Freeze `Deno.core.ops` so user code cannot replace/intercept any op
    /// (e.g. a persistent trojan op surviving in stateful/snapshot mode).
    #[arg(long = "harden-freeze-ops", env = "MCP_V8_HARDEN_FREEZE_OPS", default_value = "false", help_heading = "Sandbox")]
    pub harden_freeze_ops: bool,

    /// Neutralize `op_get_proxy_details` (otherwise it bypasses `Proxy` handlers
    /// and can read a proxied target).
    #[arg(long = "harden-neutralize-proxy-details", env = "MCP_V8_HARDEN_NEUTRALIZE_PROXY_DETAILS", default_value = "false", help_heading = "Sandbox")]
    pub harden_neutralize_proxy_details: bool,

    /// Neutralize `op_memory_usage` + `op_is_terminal` (host info leaks).
    #[arg(long = "harden-neutralize-introspection", env = "MCP_V8_HARDEN_NEUTRALIZE_INTROSPECTION", default_value = "false", help_heading = "Sandbox")]
    pub harden_neutralize_introspection: bool,

    /// Remove `globalThis.__bootstrap` (event-loop hooks, primordials such as a
    /// pristine `Function` constructor, and internal registries).
    #[arg(long = "harden-remove-bootstrap", env = "MCP_V8_HARDEN_REMOVE_BOOTSTRAP", default_value = "false", help_heading = "Sandbox")]
    pub harden_remove_bootstrap: bool,

    /// Remove `globalThis.SharedArrayBuffer` + `globalThis.Atomics` — the
    /// high-resolution Spectre-timer prerequisite. NOTE: these are also the
    /// shared-memory primitives emscripten wasm-threads require, so leave this
    /// OFF to run pthreads-based WASM modules.
    #[arg(long = "harden-remove-shared-memory", env = "MCP_V8_HARDEN_REMOVE_SHARED_MEMORY", default_value = "false", help_heading = "Sandbox")]
    pub harden_remove_shared_memory: bool,

    // ── Shared S3 backend (used by heap-store=s3 and/or fs-store=s3) ──────────
    /// S3 bucket backing whichever axes select `s3`. Required when
    /// `--heap-store s3` or `--fs-store s3` is set.
    #[arg(long, env = "MCP_V8_S3_BUCKET", help_heading = "Storage (S3)")]
    pub s3_bucket: Option<String>,

    /// Local filesystem cache directory for S3 write-through caching (only used
    /// with `--s3-bucket`).
    #[arg(long, env = "MCP_V8_CACHE_DIR", requires = "s3_bucket", help_heading = "Storage (S3)")]
    pub cache_dir: Option<String>,

    /// Override the MCP server `instructions` (the "system prompt" the server
    /// reports to clients during `initialize`). The value is used verbatim as
    /// inline text, unless it begins with `@`, in which case the remainder is
    /// treated as a path to a file whose contents are used (`@-` is not special;
    /// use `@@` for a literal leading `@`).
    /// Examples: --instructions "Run JS for me"  --instructions @./prompt.txt
    #[arg(long, env = "MCP_V8_INSTRUCTIONS", value_name = "TEXT_OR_@FILE", help_heading = "Prompt")]
    pub instructions: Option<String>,

    /// Override the description advertised for the `run_js` tool in `tools/list`.
    /// The value is used verbatim as inline text, unless it begins with `@`, in
    /// which case the remainder is treated as a path to a file whose contents are
    /// used (use `@@` for a literal leading `@`).
    /// Examples: --run-js-description "Execute JS"  --run-js-description @./run_js.md
    #[arg(long = "run-js-description", env = "MCP_V8_RUN_JS_DESCRIPTION", value_name = "TEXT_OR_@FILE", help_heading = "Prompt")]
    pub run_js_description: Option<String>,

    /// Port for the Raft cluster HTTP server. Enables cluster mode when set.
    #[arg(long, env = "MCP_V8_CLUSTER_PORT", help_heading = "Cluster")]
    pub cluster_port: Option<u16>,

    /// Unique node identifier within the cluster
    #[arg(long, env = "MCP_V8_NODE_ID", default_value = "node1", help_heading = "Cluster")]
    pub node_id: String,

    // Help is generated from `peers_grammar()` via `build_command` (the
    // structured-arg registry), so it cannot drift from the parser.
    #[arg(long, env = "MCP_V8_PEERS", value_delimiter = ',', help_heading = "Cluster")]
    #[structured(grammar = crate::cli::peers_grammar)]
    pub peers: Vec<String>,

    /// Join an existing cluster by contacting this seed address (host:port).
    /// The node will register itself with the cluster leader via /raft/join.
    #[arg(long, env = "MCP_V8_JOIN", help_heading = "Cluster")]
    pub join: Option<String>,

    /// Join as a non-voting learner: the node replicates the log but is
    /// excluded from election and commit quorums and never starts elections.
    /// Use for ephemeral nodes whose churn must not affect availability.
    #[arg(long, env = "MCP_V8_JOIN_AS_LEARNER", help_heading = "Cluster")]
    pub join_as_learner: bool,

    /// Advertise address for this node (host:port). Used for peer discovery
    /// and write forwarding. Defaults to <node-id>:<cluster-port>.
    #[arg(long, env = "MCP_V8_ADVERTISE_ADDR", help_heading = "Cluster")]
    pub advertise_addr: Option<String>,

    /// Heartbeat interval in milliseconds
    #[arg(long, env = "MCP_V8_HEARTBEAT_INTERVAL", default_value = "100", help_heading = "Cluster")]
    pub heartbeat_interval: u64,

    /// Minimum election timeout in milliseconds
    #[arg(long, env = "MCP_V8_ELECTION_TIMEOUT_MIN", default_value = "300", help_heading = "Cluster")]
    pub election_timeout_min: u64,

    /// Maximum election timeout in milliseconds
    #[arg(long, env = "MCP_V8_ELECTION_TIMEOUT_MAX", default_value = "500", help_heading = "Cluster")]
    pub election_timeout_max: u64,

    // Help is generated from `wasm_module_grammar()` via `build_command` (the
    // structured-arg registry), so it cannot drift from the parser.
    #[arg(long = "wasm-module", value_name = "NAME=PATH[:LIMIT]", help_heading = "WASM")]
    #[structured(grammar = crate::cli::wasm_module_grammar)]
    pub wasm_modules: Vec<String>,

    /// Path to a JSON config file mapping global names to .wasm file paths or objects.
    /// String value: {"name": "/path/to/module.wasm"}
    /// Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216, "description": "what the module does"}}
    /// The optional "description" sets the MCP stub tool's description.
    /// NOTE: incompatible with heap persistence (`--heap-store` other than none).
    #[arg(long = "wasm-config", env = "MCP_V8_WASM_CONFIG", value_name = "PATH", help_heading = "WASM")]
    pub wasm_config: Option<String>,

    /// Default max native memory for WASM modules without a per-module limit.
    /// Supports suffixes: k/K (KiB), m/M (MiB), g/G (GiB), or raw bytes.
    /// This is separate from --heap-memory-max (JS heap); WASM linear memory
    /// is allocated as native memory outside the V8 heap.
    #[arg(long = "wasm-default-max-memory", env = "MCP_V8_WASM_DEFAULT_MAX_MEMORY", default_value = "16m", help_heading = "WASM")]
    pub wasm_default_max_memory: String,

    /// Expose pre-loaded WASM modules on the MCPJS server itself as
    /// `<prefix>wasm__<name>` stubs. When `true` (the default whenever at
    /// least one WASM module is loaded), an external client of MCPJS can
    /// discover the module via tools/list and tool search; calling a stub
    /// returns instructional text telling the caller to use the module from
    /// JavaScript via run_js (the module is available as the `__wasm_<name>`
    /// global). Pass `--wasm-stubs false` to disable.
    #[arg(long = "wasm-stubs", env = "MCP_V8_WASM_STUBS", default_value = "true", num_args = 1, help_heading = "WASM")]
    pub wasm_stubs: bool,

    /// Prefix applied to WASM stub tool names. Defaults to `runjs__` so it is
    /// obvious to a calling agent that these modules execute through the JS
    /// runtime rather than dispatching directly. Has no effect when
    /// --wasm-stubs is false.
    #[arg(
        long = "wasm-stub-prefix",
        env = "MCP_V8_WASM_STUB_PREFIX",
        default_value = crate::engine::wasm_stub::DEFAULT_WASM_STUB_PREFIX,
        help_heading = "WASM"
    )]
    pub wasm_stub_prefix: String,

    // Help is generated from `wasm_stub_description_grammar()` via `build_command`
    // (the structured-arg registry), so it cannot drift from the parser.
    #[arg(long = "wasm-stub-description", value_name = "NAME=TEXT", help_heading = "WASM")]
    #[structured(grammar = crate::cli::wasm_stub_description_grammar)]
    pub wasm_stub_descriptions: Vec<String>,

    // Help is generated from `fetch_header_grammar()` via `build_command` (the
    // structured-arg registry), with the key list coming straight from
    // `FetchHeaderKey`, so it cannot drift from the parser.
    #[arg(long = "fetch-header", value_name = "RULE", help_heading = "Fetch")]
    #[structured(grammar = crate::cli::fetch_header_grammar)]
    pub fetch_headers: Vec<String>,

    /// Path to a JSON file with header injection rules. Each rule sets "host"
    /// (plus optional "methods") and exactly one of "headers" or "auth".
    /// Static: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}]
    /// OAuth: [{"host": "api.example.com", "auth": {"type": "oauth_client_credentials", "header": "Authorization", "token_url": "https://issuer.example.com/token", "client_id": "abc", "client_secret": "xyz", "scope": "read:all", "refresh_buffer_secs": 30}}]
    #[arg(long = "fetch-header-config", env = "MCP_V8_FETCH_HEADER_CONFIG", value_name = "PATH", help_heading = "Fetch")]
    pub fetch_header_config: Option<String>,

    /// Allow external module imports (npm:, jsr:, and URL imports).
    /// When disabled (the default), code using import declarations for external
    /// packages will be rejected. Enable with --allow-external-modules.
    #[arg(long = "allow-external-modules", env = "MCP_V8_ALLOW_EXTERNAL_MODULES", default_value = "false", help_heading = "Module Import")]
    pub allow_external_modules: bool,

    /// Allow the `run_js` tool to read its code from a file on the server's own
    /// filesystem (the `file` parameter). OFF by default. When set, ANY path the
    /// server process can read is allowed — this is the easy "allow all" switch.
    /// For finer control, leave this off and configure a `run_js_file` policy in
    /// --policies-json instead (a Rego/OPA chain decides which paths are allowed);
    /// the policy input is `{ "operation": "read", "path": "<canonical path>" }`.
    /// This flag takes precedence over a configured run_js_file policy.
    #[arg(long = "allow-run-js-file", env = "MCP_V8_ALLOW_RUN_JS_FILE", default_value = "false", help_heading = "Run JS File")]
    pub allow_run_js_file: bool,

    /// JSON policy configuration (inline JSON or path to a JSON file).
    /// Enables fetch() and/or module policy gating via local Rego files
    /// and/or remote OPA servers.
    ///
    /// Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}'
    ///
    /// Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }
    #[arg(long = "policies-json", env = "MCP_V8_POLICIES_JSON", value_name = "JSON_OR_PATH", help_heading = "Policy")]
    pub policies_json: Option<String>,

    // Help is generated from `mcp_server_grammar()` via `build_command` (the
    // structured-arg registry), so it cannot drift from the parser.
    #[arg(long = "mcp-server", value_name = "NAME=TRANSPORT:...", help_heading = "MCP Server Module")]
    #[structured(grammar = crate::cli::mcp_server_grammar)]
    pub mcp_servers: Vec<String>,

    /// Path to a JSON config file for MCP server modules.
    /// Format: [{"name": "srv", "transport": "stdio", "command": "cmd", "args": ["a"]},
    ///          {"name": "srv2", "transport": "sse", "url": "http://..."}]
    #[arg(long = "mcp-config", env = "MCP_V8_MCP_CONFIG", value_name = "PATH", help_heading = "MCP Server Module")]
    pub mcp_config: Option<String>,

    /// Expose upstream MCP server tools on the MCPJS server itself as
    /// `<prefix><server>__<tool>` stubs. When `true` (the default whenever
    /// at least one --mcp-server is configured), an external client of
    /// MCPJS can discover those tools via tools/list and tool search;
    /// calling a stub returns instructional text telling the caller to
    /// invoke the tool from JavaScript via run_js + mcp.callTool(...).
    /// Pass `--mcp-stubs false` to disable.
    #[arg(long = "mcp-stubs", env = "MCP_V8_MCP_STUBS", default_value = "true", num_args = 1, help_heading = "MCP Server Module")]
    pub mcp_stubs: bool,

    /// Prefix applied to stub tool names. Defaults to `runjs__` so it is
    /// obvious to a calling agent that these tools execute through the JS
    /// runtime rather than dispatching directly. Has no effect when
    /// --mcp-stubs is false.
    #[arg(
        long = "mcp-stub-prefix",
        env = "MCP_V8_MCP_STUB_PREFIX",
        default_value = crate::engine::mcp_client::DEFAULT_STUB_PREFIX,
        help_heading = "MCP Server Module"
    )]
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

/// Declares the `--fetch-header` key vocabulary — and each key's help blurb — in
/// exactly one place.
///
/// Adding, renaming, or removing a key is a single edit to the
/// `fetch_header_keys!` invocation below. Everything downstream is generated
/// from it: the enum, the variant<->string mapping (`as_str`), the accepted-key
/// list (`ALL`), the "expected keys" error text (`expected`), and — via
/// [`fetch_header_long_help`] — the flag's `--help` text. So nothing can drift
/// (O(1) maintenance), and the help can never omit a key: there is no separate
/// list to keep in sync and therefore no runtime test to forget. The parser's
/// dispatch `match` (in main.rs) is exhaustive, so a newly added key is a
/// *compile error* until it is handled there too.
macro_rules! fetch_header_keys {
    ( $( $variant:ident = $name:literal : $desc:literal ),+ $(,)? ) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum FetchHeaderKey {
            $( $variant, )+
        }

        impl FetchHeaderKey {
            /// Every accepted key, generated from the declaration.
            pub const ALL: &'static [FetchHeaderKey] = &[ $( FetchHeaderKey::$variant ),+ ];

            pub fn as_str(self) -> &'static str {
                match self {
                    $( FetchHeaderKey::$variant => $name, )+
                }
            }

            /// One-line help blurb for the key, rendered into `--help`.
            fn description(self) -> &'static str {
                match self {
                    $( FetchHeaderKey::$variant => $desc, )+
                }
            }

            pub fn from_key(key: &str) -> Option<FetchHeaderKey> {
                FetchHeaderKey::ALL
                    .iter()
                    .copied()
                    .find(|candidate| candidate.as_str() == key)
            }

            /// Comma-separated list of accepted keys, for the "unknown key" error.
            pub fn expected() -> String {
                FetchHeaderKey::ALL
                    .iter()
                    .map(|key| key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }
    };
}

fetch_header_keys! {
    Host = "host": "host pattern the request URL must match (required)",
    Methods = "methods": "semicolon-separated HTTP methods to match, e.g. GET;POST (optional)",
    Header = "header": "name of the header to inject (required)",
    Value = "value": "static header value (static form)",
    TokenUrl = "token_url": "OAuth token endpoint URL (OAuth form)",
    ClientId = "client_id": "OAuth client id (OAuth form)",
    ClientSecret = "client_secret": "OAuth client secret (OAuth form)",
    Scope = "scope": "OAuth scope (OAuth form, optional)",
    RefreshBufferSecs = "refresh_buffer_secs": "seconds before expiry to refresh the token, default 30 (OAuth form, optional)",
}

/// A documented grammar for a structured CLI flag value, rendered uniformly into
/// `--help`. Declaring the grammar once (and registering it in
/// [`structured_args`]) means a flag's mini-language is *generated*, never
/// hand-written prose that can drift from the parser.
pub struct Grammar {
    /// One-line summary; also the short (`-h`) help.
    summary: &'static str,
    /// Heading for the labelled parts (e.g. "Accepted keys", "Transports").
    parts_label: &'static str,
    /// `(label, description)` rows — the keys, transports, or format forms.
    parts: Vec<(&'static str, &'static str)>,
    /// Illustrative example values.
    examples: &'static [&'static str],
}

impl Grammar {
    fn render(&self) -> String {
        let mut out = self.summary.to_string();
        if !self.parts.is_empty() {
            out.push('\n');
            out.push_str(self.parts_label);
            out.push(':');
            for (label, desc) in &self.parts {
                out.push_str(&format!("\n  {label} — {desc}"));
            }
        }
        if !self.examples.is_empty() {
            out.push_str("\nExamples:");
            for example in self.examples {
                out.push_str(&format!("\n  {example}"));
            }
        }
        out
    }
}

fn fetch_header_grammar() -> Grammar {
    Grammar {
        summary: "Inject a header into fetch requests that match host/method rules. Each \
                  rule is a comma-separated list of key=value pairs and must use either the \
                  static value form or the OAuth client-credentials form (mutually \
                  exclusive). Can be specified multiple times.",
        parts_label: "Accepted keys",
        // Generated from the parser's own key table, so the documented keys are
        // exactly the keys the parser accepts.
        parts: FetchHeaderKey::ALL
            .iter()
            .map(|key| (key.as_str(), key.description()))
            .collect(),
        examples: &[],
    }
}

fn mcp_server_grammar() -> Grammar {
    Grammar {
        summary: "Connect to an external MCP server as a module; JS can call its tools via \
                  the `mcp` global (mcp.callTool, mcp.listTools, mcp.servers). Can be \
                  specified multiple times.",
        parts_label: "Transports",
        parts: vec![
            ("name=stdio:command:arg1:arg2", "spawn a stdio MCP server process"),
            ("name=sse:url", "connect to an SSE MCP server endpoint"),
        ],
        examples: &["weather=stdio:python:server.py", "remote=sse:http://localhost:9000/sse"],
    }
}

fn wasm_module_grammar() -> Grammar {
    Grammar {
        summary: "Pre-load a WASM module as a global named <name>; its exports become that \
                  global. An optional :max_memory suffix caps the module's native memory \
                  (linear memory + tables) with suffixes raw bytes, k/K (KiB), m/M (MiB), \
                  g/G (GiB). Can be specified multiple times. Incompatible with heap \
                  persistence (--heap-store other than none).",
        parts_label: "Format",
        parts: vec![(
            "name=/path/to/module.wasm[:max_memory]",
            "load <name> from a .wasm file, optionally capping its native memory",
        )],
        examples: &["math=/path.wasm", "math=/path.wasm:16m", "math=/path.wasm:1048576"],
    }
}

fn wasm_stub_description_grammar() -> Grammar {
    Grammar {
        summary: "Set the MCP stub tool description for a loaded WASM module; the text is \
                  shown to downstream agents alongside the auto-generated usage hint. \
                  Overrides a `description` set inline via --wasm-config. The named module \
                  must be loaded with --wasm-module or --wasm-config. Can be specified \
                  multiple times.",
        parts_label: "Format",
        parts: vec![("name=description text", "set <name>'s stub tool description")],
        examples: &["math=Adds two numbers and returns the sum"],
    }
}

fn peers_grammar() -> Grammar {
    Grammar {
        summary: "Comma-separated list of seed peer addresses. Peers can also join \
                  dynamically via POST /raft/join.",
        parts_label: "Forms",
        parts: vec![
            ("id@host:port", "peer address with an explicit node id"),
            ("host:port", "peer address only (node id learned on join)"),
        ],
        examples: &["node2@10.0.0.2:4000", "10.0.0.3:4000"],
    }
}

// `Cli::structured_args()` and `Cli::structured_arg_ids()` are generated by
// `#[derive(StructuredArgs)]` on `Cli` (below) from the `#[structured(grammar
// = ...)]` field attributes — so there is no hand-maintained registry list to
// keep in sync with the fields.

/// Canonical `clap::Command` for the binary: the derived command with every
/// structured flag's short and long help generated from its [`Grammar`]. The
/// server binary ([`parse`]) and `generate-cli-markdown` both build through here,
/// so the live `--help` and the generated CLI reference stay identical and
/// table-driven — there is no per-flag code in this function.
pub fn build_command() -> clap::Command {
    let mut command = Cli::command();
    for (arg_id, grammar) in Cli::structured_args() {
        let summary = grammar.summary;
        let long_help = grammar.render();
        command = command.mut_arg(arg_id, move |arg| arg.help(summary).long_help(long_help));
    }
    command
}

/// Parse CLI arguments through [`build_command`]. Mirrors `Cli::parse`, but with
/// the generated `--fetch-header` help wired in.
pub fn parse() -> Cli {
    let mut matches = build_command().get_matches();
    Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|err| err.exit())
}
