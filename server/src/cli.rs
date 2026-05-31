use clap::Parser;

use crate::engine::DEFAULT_EXECUTION_TIMEOUT_SECS;

fn default_max_concurrent() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// Command line arguments for configuring heap storage
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Print the OpenAPI JSON specification to stdout and exit.
    /// Use this to regenerate openapi.json: `./server --print-openapi > openapi.json`
    #[arg(long, help_heading = "Core")]
    pub print_openapi: bool,

    /// S3 bucket name (required if --use-s3)
    #[arg(long, conflicts_with_all = ["directory_path", "stateless"], help_heading = "Core")]
    pub s3_bucket: Option<String>,

    /// Local filesystem cache directory for S3 write-through caching (only used with --s3-bucket)
    #[arg(long, requires = "s3_bucket", help_heading = "Core")]
    pub cache_dir: Option<String>,

    /// Directory path for filesystem storage (required if --use-filesystem)
    #[arg(long, conflicts_with_all = ["s3_bucket", "stateless"], help_heading = "Core")]
    pub directory_path: Option<String>,

    /// Run in stateless mode - no heap snapshots are saved or loaded
    #[arg(long, conflicts_with_all = ["s3_bucket", "directory_path"], help_heading = "Core")]
    pub stateless: bool,

    /// JWKS endpoint URL for fetching public keys (e.g., Keycloak OIDC certs URL).
    /// Enables JWT verification of Authorization: Bearer tokens during initialize.
    #[arg(long, env = "JWKS_URL", help_heading = "Core")]
    pub jwks_url: Option<String>,

    /// HTTP port using Streamable HTTP transport (MCP 2025-03-26+, load-balanceable)
    #[arg(long, conflicts_with = "sse_port", help_heading = "Core")]
    pub http_port: Option<u16>,

    /// SSE port using the older HTTP+SSE transport
    #[arg(long, conflicts_with = "http_port", help_heading = "Core")]
    pub sse_port: Option<u16>,

    /// Maximum V8 heap memory per isolate in megabytes (default: 8)
    #[arg(
        long,
        default_value = "8",
        value_parser = clap::value_parser!(u64).range(1..),
        help_heading = "Core"
    )]
    pub heap_memory_max: u64,

    /// Maximum execution timeout in seconds (default: 30, max: 300)
    #[arg(
        long,
        default_value_t = DEFAULT_EXECUTION_TIMEOUT_SECS,
        value_parser = clap::value_parser!(u64).range(1..=300),
        help_heading = "Core"
    )]
    pub execution_timeout: u64,

    /// Maximum concurrent V8 executions (default: CPU core count)
    #[arg(long, default_value_t = default_max_concurrent(), help_heading = "Core")]
    pub max_concurrent_executions: usize,

    /// Path to the sled database for session logging (default: /tmp/mcp-v8-sessions)
    #[arg(long, default_value = "/tmp/mcp-v8-sessions", help_heading = "Core")]
    pub session_db_path: String,

    /// Port for the Raft cluster HTTP server. Enables cluster mode when set.
    #[arg(long, help_heading = "Cluster")]
    pub cluster_port: Option<u16>,

    /// Unique node identifier within the cluster
    #[arg(long, default_value = "node1", help_heading = "Cluster")]
    pub node_id: String,

    /// Comma-separated list of seed peer addresses. Format: id@host:port or host:port.
    /// Peers can also join dynamically via POST /raft/join.
    #[arg(long, value_delimiter = ',', help_heading = "Cluster")]
    pub peers: Vec<String>,

    /// Join an existing cluster by contacting this seed address (host:port).
    /// The node will register itself with the cluster leader via /raft/join.
    #[arg(long, help_heading = "Cluster")]
    pub join: Option<String>,

    /// Advertise address for this node (host:port). Used for peer discovery
    /// and write forwarding. Defaults to <node-id>:<cluster-port>.
    #[arg(long, help_heading = "Cluster")]
    pub advertise_addr: Option<String>,

    /// Heartbeat interval in milliseconds
    #[arg(long, default_value = "100", help_heading = "Cluster")]
    pub heartbeat_interval: u64,

    /// Minimum election timeout in milliseconds
    #[arg(long, default_value = "300", help_heading = "Cluster")]
    pub election_timeout_min: u64,

    /// Maximum election timeout in milliseconds
    #[arg(long, default_value = "500", help_heading = "Cluster")]
    pub election_timeout_max: u64,

    /// Pre-load a WASM module as a global. Format: name=/path/to/module.wasm[:max_memory]
    /// The module's exports will be available as a global variable with the given name.
    /// Optional memory suffix caps the module's native memory (linear memory + tables).
    /// Supported suffixes: raw bytes, k/K (KiB), m/M (MiB), g/G (GiB).
    /// Examples: math=/path.wasm  math=/path.wasm:16m  math=/path.wasm:1048576
    /// Can be specified multiple times for multiple modules.
    #[arg(long = "wasm-module", value_name = "NAME=PATH[:LIMIT]", help_heading = "WASM")]
    pub wasm_modules: Vec<String>,

    /// Path to a JSON config file mapping global names to .wasm file paths or objects.
    /// String value: {"name": "/path/to/module.wasm"}
    /// Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216}}
    #[arg(long = "wasm-config", value_name = "PATH", help_heading = "WASM")]
    pub wasm_config: Option<String>,

    /// Default max native memory for WASM modules without a per-module limit.
    /// Supports suffixes: k/K (KiB), m/M (MiB), g/G (GiB), or raw bytes.
    /// This is separate from --heap-memory-max (JS heap); WASM linear memory
    /// is allocated as native memory outside the V8 heap.
    #[arg(long = "wasm-default-max-memory", default_value = "16m", help_heading = "WASM")]
    pub wasm_default_max_memory: String,

    /// Inject headers into fetch requests matching host/method rules.
    /// Format: host=<host>,header=<name>,value=<val>[,methods=GET;POST]
    /// Can be specified multiple times.
    #[arg(long = "fetch-header", value_name = "RULE", help_heading = "Fetch")]
    pub fetch_headers: Vec<String>,

    /// Path to a JSON file with header injection rules.
    /// Format: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}]
    #[arg(long = "fetch-header-config", value_name = "PATH", help_heading = "Fetch")]
    pub fetch_header_config: Option<String>,

    /// Allow external module imports (npm:, jsr:, and URL imports).
    /// When disabled (the default), code using import declarations for external
    /// packages will be rejected. Enable with --allow-external-modules.
    #[arg(long = "allow-external-modules", default_value = "false", help_heading = "Module Import")]
    pub allow_external_modules: bool,

    /// JSON policy configuration (inline JSON or path to a JSON file).
    /// Enables fetch() and/or module policy gating via local Rego files
    /// and/or remote OPA servers.
    ///
    /// Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}'
    ///
    /// Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }
    #[arg(long = "policies-json", value_name = "JSON_OR_PATH", help_heading = "Policy")]
    pub policies_json: Option<String>,

    /// Connect to an external MCP server as a module. JS code can call its tools
    /// via the `mcp` global object (mcp.callTool, mcp.listTools, mcp.servers).
    /// Format for stdio: name=stdio:command:arg1:arg2
    /// Format for SSE:   name=sse:url
    /// Can be specified multiple times for multiple servers.
    #[arg(long = "mcp-server", value_name = "NAME=TRANSPORT:...", help_heading = "MCP Server Module")]
    pub mcp_servers: Vec<String>,

    /// Path to a JSON config file for MCP server modules.
    /// Format: [{"name": "srv", "transport": "stdio", "command": "cmd", "args": ["a"]},
    ///          {"name": "srv2", "transport": "sse", "url": "http://..."}]
    #[arg(long = "mcp-config", value_name = "PATH", help_heading = "MCP Server Module")]
    pub mcp_config: Option<String>,

    /// Expose upstream MCP server tools on the MCPJS server itself as
    /// `<prefix><server>__<tool>` stubs. When `true` (the default whenever
    /// at least one --mcp-server is configured), an external client of
    /// MCPJS can discover those tools via tools/list and tool search;
    /// calling a stub returns instructional text telling the caller to
    /// invoke the tool from JavaScript via run_js + mcp.callTool(...).
    /// Pass `--mcp-stubs false` to disable.
    #[arg(long = "mcp-stubs", default_value = "true", num_args = 1, help_heading = "MCP Server Module")]
    pub mcp_stubs: bool,

    /// Prefix applied to stub tool names. Defaults to `runjs__` so it is
    /// obvious to a calling agent that these tools execute through the JS
    /// runtime rather than dispatching directly. Has no effect when
    /// --mcp-stubs is false.
    #[arg(
        long = "mcp-stub-prefix",
        default_value = crate::engine::mcp_client::DEFAULT_STUB_PREFIX,
        help_heading = "MCP Server Module"
    )]
    pub mcp_stub_prefix: String,
}
