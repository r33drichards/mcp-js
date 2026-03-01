use anyhow::Result;
use rmcp::{ServerHandler, ServiceExt, transport::stdio};
use tracing_subscriber::{self};
use clap::Parser;
use rmcp::transport::sse_server::{SseServer, SseServerConfig};
use rmcp::transport::StreamableHttpServer;
use rmcp::transport::streamable_http_server::axum::StreamableHttpServerConfig;
use tokio_util::sync::CancellationToken;
use std::sync::Arc;
mod engine;
mod mcp;
mod api;
mod cluster;
use engine::{initialize_v8, Engine, WasmModule, DEFAULT_EXECUTION_TIMEOUT_SECS};
use engine::fetch::FetchConfig;
use engine::execution::ExecutionRegistry;
use engine::module_loader::ModuleLoaderConfig;
use engine::opa::{OpaClient, PoliciesConfig, build_policy_chain};
use engine::heap_storage::{AnyHeapStorage, S3HeapStorage, WriteThroughCacheHeapStorage, FileHeapStorage};
use engine::heap_tags::HeapTagStore;
use engine::session_log::SessionLog;
use mcp::{McpService, StatelessMcpService};
use cluster::{ClusterConfig, ClusterNode};

fn default_max_concurrent() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// Command line arguments for configuring heap storage
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {

    /// S3 bucket name (required if --use-s3)
    #[arg(long, conflicts_with_all = ["directory_path", "stateless"])]
    s3_bucket: Option<String>,

    /// Local filesystem cache directory for S3 write-through caching (only used with --s3-bucket)
    #[arg(long, requires = "s3_bucket")]
    cache_dir: Option<String>,

    /// Directory path for filesystem storage (required if --use-filesystem)
    #[arg(long, conflicts_with_all = ["s3_bucket", "stateless"])]
    directory_path: Option<String>,

    /// Run in stateless mode - no heap snapshots are saved or loaded
    #[arg(long, conflicts_with_all = ["s3_bucket", "directory_path"])]
    stateless: bool,

    /// HTTP port using Streamable HTTP transport (MCP 2025-03-26+, load-balanceable)
    #[arg(long, conflicts_with = "sse_port")]
    http_port: Option<u16>,

    /// SSE port using the older HTTP+SSE transport
    #[arg(long, conflicts_with = "http_port")]
    sse_port: Option<u16>,

    /// Maximum V8 heap memory per isolate in megabytes (default: 8, max: 64)
    #[arg(long, default_value = "8", value_parser = clap::value_parser!(u64).range(1..=64))]
    heap_memory_max: u64,

    /// Maximum execution timeout in seconds (default: 30, max: 300)
    #[arg(long, default_value_t = DEFAULT_EXECUTION_TIMEOUT_SECS, value_parser = clap::value_parser!(u64).range(1..=300))]
    execution_timeout: u64,

    /// Maximum concurrent V8 executions (default: CPU core count)
    #[arg(long, default_value_t = default_max_concurrent())]
    max_concurrent_executions: usize,

    /// Path to the sled database for session logging (default: /tmp/mcp-v8-sessions)
    #[arg(long, default_value = "/tmp/mcp-v8-sessions")]
    session_db_path: String,

    // ── Cluster options ────────────────────────────────────────────────

    /// Port for the Raft cluster HTTP server. Enables cluster mode when set.
    #[arg(long)]
    cluster_port: Option<u16>,

    /// Unique node identifier within the cluster
    #[arg(long, default_value = "node1")]
    node_id: String,

    /// Comma-separated list of seed peer addresses. Format: id@host:port or host:port.
    /// Peers can also join dynamically via POST /raft/join.
    #[arg(long, value_delimiter = ',')]
    peers: Vec<String>,

    /// Join an existing cluster by contacting this seed address (host:port).
    /// The node will register itself with the cluster leader via /raft/join.
    #[arg(long)]
    join: Option<String>,

    /// Advertise address for this node (host:port). Used for peer discovery
    /// and write forwarding. Defaults to <node-id>:<cluster-port>.
    #[arg(long)]
    advertise_addr: Option<String>,

    /// Heartbeat interval in milliseconds
    #[arg(long, default_value = "100")]
    heartbeat_interval: u64,

    /// Minimum election timeout in milliseconds
    #[arg(long, default_value = "300")]
    election_timeout_min: u64,

    /// Maximum election timeout in milliseconds
    #[arg(long, default_value = "500")]
    election_timeout_max: u64,

    // ── WASM module options ──────────────────────────────────────────

    /// Pre-load a WASM module as a global. Format: name=/path/to/module.wasm[:max_memory]
    /// The module's exports will be available as a global variable with the given name.
    /// Optional memory suffix caps the module's native memory (linear memory + tables).
    /// Supported suffixes: raw bytes, k/K (KiB), m/M (MiB), g/G (GiB).
    /// Examples: math=/path.wasm  math=/path.wasm:16m  math=/path.wasm:1048576
    /// Can be specified multiple times for multiple modules.
    #[arg(long = "wasm-module", value_name = "NAME=PATH[:LIMIT]")]
    wasm_modules: Vec<String>,

    /// Path to a JSON config file mapping global names to .wasm file paths or objects.
    /// String value: {"name": "/path/to/module.wasm"}
    /// Object value: {"name": {"path": "/path/to/module.wasm", "max_memory_bytes": 16777216}}
    #[arg(long = "wasm-config", value_name = "PATH")]
    wasm_config: Option<String>,

    /// Default max native memory for WASM modules without a per-module limit.
    /// Supports suffixes: k/K (KiB), m/M (MiB), g/G (GiB), or raw bytes.
    /// This is separate from --heap-memory-max (JS heap); WASM linear memory
    /// is allocated as native memory outside the V8 heap.
    #[arg(long = "wasm-default-max-memory", default_value = "16m")]
    wasm_default_max_memory: String,

    // ── OPA / fetch options ──────────────────────────────────────────────

    /// OPA server URL for policy-gated operations. When set, fetch() becomes
    /// available in the JS runtime, with each request checked against OPA.
    /// Example: http://localhost:8181
    /// Deprecated: prefer --policies-json for new deployments.
    #[arg(long = "opa-url", value_name = "URL", conflicts_with = "policies_json")]
    opa_url: Option<String>,

    /// OPA policy path for fetch requests (appended to /v1/data/).
    /// Default: "mcp/fetch". The policy must return {"allow": true} to permit a request.
    #[arg(long = "opa-fetch-policy", default_value = "mcp/fetch", requires = "opa_url")]
    opa_fetch_policy: String,

    /// Inject headers into fetch requests matching host/method rules.
    /// Format: host=<host>,header=<name>,value=<val>[,methods=GET;POST]
    /// Can be specified multiple times.
    #[arg(long = "fetch-header", value_name = "RULE")]
    fetch_headers: Vec<String>,

    /// Path to a JSON file with header injection rules.
    /// Format: [{"host": "api.github.com", "methods": ["GET","POST"], "headers": {"Authorization": "Bearer ..."}}]
    #[arg(long = "fetch-header-config", value_name = "PATH")]
    fetch_header_config: Option<String>,

    // ── Module import options ──────────────────────────────────────────

    /// Allow external module imports (npm:, jsr:, and URL imports).
    /// When disabled (the default), code using import declarations for external
    /// packages will be rejected. Enable with --allow-external-modules.
    #[arg(long = "allow-external-modules", default_value = "false")]
    allow_external_modules: bool,

    /// OPA policy path for auditing module imports (appended to /v1/data/).
    /// When set (requires --opa-url and --allow-external-modules), each module
    /// specifier is checked against this policy before being fetched.
    /// The policy receives {specifier, specifier_type, resolved_url} as input
    /// and must return {"allow": true} to permit the import.
    #[arg(long = "opa-module-policy", value_name = "PATH", requires_all = ["opa_url", "allow_external_modules"])]
    opa_module_policy: Option<String>,

    // ── Policy configuration ─────────────────────────────────────────────

    /// JSON policy configuration (inline JSON or path to a JSON file).
    /// Enables fetch() and/or module policy gating via local Rego files
    /// and/or remote OPA servers. Replaces --opa-url and related flags.
    ///
    /// Example: --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}'
    ///
    /// Schema: { "fetch": { "mode": "all"|"any", "policies": [{"url": "...", "policy_path": "...", "rule": "..."}] }, "modules": { ... } }
    #[arg(long = "policies-json", value_name = "JSON_OR_PATH", conflicts_with = "opa_url")]
    policies_json: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    initialize_v8();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();

    tracing::info!(?cli, "Starting MCP server with CLI arguments");

    let heap_memory_max_bytes = (cli.heap_memory_max as usize) * 1024 * 1024;
    let execution_timeout_secs = cli.execution_timeout;
    tracing::info!("V8 heap memory limit: {} MB ({} bytes)", cli.heap_memory_max, heap_memory_max_bytes);
    tracing::info!("V8 execution timeout: {} seconds", execution_timeout_secs);
    tracing::info!("Max concurrent V8 executions: {}", cli.max_concurrent_executions);

    // Cluster mode requires --http-port or --sse-port.
    if cli.cluster_port.is_some() && cli.http_port.is_none() && cli.sse_port.is_none() {
        anyhow::bail!(
            "Cluster mode requires --http-port or --sse-port (stdio transport is not supported in cluster mode)"
        );
    }

    // Parse peer list (supports both "host:port" and "id@host:port" formats).
    let (peer_addrs_list, peer_addrs_map) = ClusterConfig::parse_peers(&cli.peers);

    // Start cluster node if cluster_port is specified.
    let cluster_node: Option<Arc<ClusterNode>> = if let Some(cluster_port) = cli.cluster_port {
        let cluster_config = ClusterConfig {
            node_id: cli.node_id.clone(),
            peers: peer_addrs_list,
            peer_addrs: peer_addrs_map,
            cluster_port,
            advertise_addr: cli.advertise_addr.clone().or_else(|| Some(format!("{}:{}", cli.node_id, cluster_port))),
            heartbeat_interval: std::time::Duration::from_millis(cli.heartbeat_interval),
            election_timeout_min: std::time::Duration::from_millis(cli.election_timeout_min),
            election_timeout_max: std::time::Duration::from_millis(cli.election_timeout_max),
        };

        let cluster_db_path = format!("{}/cluster-{}", cli.session_db_path, cli.node_id);
        let cluster_db = sled::open(&cluster_db_path)
            .expect("Failed to open cluster sled database");

        let node = ClusterNode::new(cluster_config, cluster_db);
        node.start().await;
        tracing::info!("Cluster node {} started on port {}", cli.node_id, cluster_port);

        // If --join is specified, register with an existing cluster member.
        if let Some(ref seed_addr) = cli.join {
            let my_addr = cli.advertise_addr.clone().unwrap_or_else(|| format!("{}:{}", cli.node_id, cluster_port));
            tracing::info!("Joining cluster via seed node {}", seed_addr);
            let join_req = cluster::JoinRequest {
                node_id: cli.node_id.clone(),
                addr: my_addr,
            };
            let client = reqwest::Client::new();
            let url = format!("http://{}/raft/join", seed_addr);
            match client.post(&url).json(&join_req).send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::info!("Successfully joined cluster via {}", seed_addr);
                }
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!("Join request returned error: {}", body);
                }
                Err(e) => {
                    tracing::warn!("Failed to join cluster via {}: {}", seed_addr, e);
                }
            }
        }

        Some(node)
    } else {
        None
    };

    // ── WASM configuration ─────────────────────────────────────────────
    let wasm_default_max_bytes = parse_memory_size(&cli.wasm_default_max_memory)
        .map_err(|e| anyhow::anyhow!("Invalid --wasm-default-max-memory: {}", e))?;
    tracing::info!("WASM default max memory: {} bytes ({} MiB)", wasm_default_max_bytes, wasm_default_max_bytes / 1024 / 1024);

    let wasm_modules = load_wasm_modules(&cli.wasm_modules, &cli.wasm_config)?;
    if !wasm_modules.is_empty() {
        tracing::info!("Loaded {} WASM module(s): {}", wasm_modules.len(),
            wasm_modules.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", "));
    }

    // ── Build Engine ────────────────────────────────────────────────────
    let engine = if cli.stateless {
        tracing::info!("Creating stateless engine");
        Engine::new_stateless(heap_memory_max_bytes, execution_timeout_secs, cli.max_concurrent_executions)
    } else {
        let heap_storage = if let Some(bucket) = cli.s3_bucket {
            if let Some(cache_dir) = cli.cache_dir {
                tracing::info!("Using S3 storage with FS write-through cache at {}", cache_dir);
                AnyHeapStorage::S3WithFsCache(WriteThroughCacheHeapStorage::new(
                    S3HeapStorage::new(bucket).await,
                    cache_dir,
                ))
            } else {
                AnyHeapStorage::S3(S3HeapStorage::new(bucket).await)
            }
        } else if let Some(dir) = cli.directory_path {
            AnyHeapStorage::File(FileHeapStorage::new(dir))
        } else {
            AnyHeapStorage::File(FileHeapStorage::new("/tmp/mcp-v8-heaps"))
        };

        let session_log = match SessionLog::new(&cli.session_db_path) {
            Ok(log) => {
                tracing::info!("Session log opened at {}", cli.session_db_path);
                let log = if let Some(ref cn) = cluster_node {
                    tracing::info!("Session log will use Raft cluster for replication");
                    log.with_cluster(cn.clone())
                } else {
                    log
                };
                Some(log)
            }
            Err(e) => {
                tracing::warn!("Failed to open session log at {}: {}. Session logging disabled.", cli.session_db_path, e);
                None
            }
        };

        let heap_tag_db_path = format!("{}/heap-tags", cli.session_db_path);
        let heap_tag_store = match HeapTagStore::new(&heap_tag_db_path) {
            Ok(store) => {
                tracing::info!("Heap tag store opened at {}", heap_tag_db_path);
                let store = if let Some(ref cn) = cluster_node {
                    store.with_cluster(cn.clone())
                } else {
                    store
                };
                Some(store)
            }
            Err(e) => {
                tracing::warn!("Failed to open heap tag store at {}: {}. Heap tagging disabled.", heap_tag_db_path, e);
                None
            }
        };

        tracing::info!("Creating stateful engine");
        Engine::new_stateful(heap_storage, session_log, heap_tag_store, heap_memory_max_bytes, execution_timeout_secs, cli.max_concurrent_executions)
    };

    let engine = engine.with_wasm_default_max_bytes(wasm_default_max_bytes);
    let engine = if wasm_modules.is_empty() { engine } else { engine.with_wasm_modules(wasm_modules) };

    // ── Policy configuration ─────────────────────────────────────────────
    // Parse --policies-json if provided (inline JSON or file path).
    let policies_config: Option<PoliciesConfig> = if let Some(ref json_or_path) = cli.policies_json {
        let json_str = if json_or_path.trim_start().starts_with('{') {
            json_or_path.clone()
        } else {
            std::fs::read_to_string(json_or_path)
                .map_err(|e| anyhow::anyhow!("Failed to read policies config '{}': {}", json_or_path, e))?
        };
        let config: PoliciesConfig = serde_json::from_str(&json_str)
            .map_err(|e| anyhow::anyhow!("Invalid policies JSON: {}", e))?;
        tracing::info!("Loaded policies configuration from --policies-json");
        Some(config)
    } else {
        None
    };

    // Build policy chains from the parsed config.
    let fetch_policy_chain = if let Some(ref config) = policies_config {
        if let Some(ref fetch_policies) = config.fetch {
            let chain = build_policy_chain(fetch_policies, "mcp/fetch", "data.mcp.fetch.allow")
                .map_err(|e| anyhow::anyhow!("Failed to build fetch policy chain: {}", e))?;
            tracing::info!("Fetch policy chain: {} evaluator(s), mode={:?}", fetch_policies.policies.len(), fetch_policies.mode);
            Some(Arc::new(chain))
        } else {
            None
        }
    } else {
        None
    };

    let modules_policy_chain = if let Some(ref config) = policies_config {
        if let Some(ref module_policies) = config.modules {
            let chain = build_policy_chain(module_policies, "mcp/modules", "data.mcp.modules.allow")
                .map_err(|e| anyhow::anyhow!("Failed to build modules policy chain: {}", e))?;
            tracing::info!("Modules policy chain: {} evaluator(s), mode={:?}", module_policies.policies.len(), module_policies.mode);
            Some(Arc::new(chain))
        } else {
            None
        }
    } else {
        None
    };

    // ── OPA / fetch ─────────────────────────────────────────────────────
    let header_rules = load_fetch_header_rules(&cli.fetch_headers, &cli.fetch_header_config)?;
    if !header_rules.is_empty() {
        tracing::info!("Loaded {} fetch header injection rule(s)", header_rules.len());
    }

    let engine = if let Some(ref opa_url) = cli.opa_url {
        // Legacy --opa-url path (deprecated).
        tracing::warn!("--opa-url is deprecated. Use --policies-json instead. See --help for the new format.");
        tracing::info!("OPA enabled (legacy): {} (fetch policy: {})", opa_url, cli.opa_fetch_policy);
        let fetch_config = FetchConfig::new(opa_url.clone(), cli.opa_fetch_policy.clone())
            .with_header_rules(header_rules);
        engine.with_fetch_config(fetch_config)
    } else if let Some(chain) = fetch_policy_chain {
        // --policies-json fetch chain.
        let fetch_config = FetchConfig::new_with_chain(chain)
            .with_header_rules(header_rules);
        engine.with_fetch_config(fetch_config)
    } else {
        engine
    };

    // ── Module loader config ─────────────────────────────────────────────
    let module_loader_config = ModuleLoaderConfig {
        allow_external: cli.allow_external_modules,
        opa_client: cli.opa_module_policy.as_ref().map(|_| {
            OpaClient::new(cli.opa_url.clone().expect("--opa-module-policy requires --opa-url"))
        }),
        opa_module_policy: cli.opa_module_policy.clone(),
        policy_chain: modules_policy_chain,
    };
    if cli.allow_external_modules {
        tracing::info!("External module imports: ENABLED");
        if let Some(ref policy) = cli.opa_module_policy {
            tracing::info!("OPA module audit policy: {}", policy);
        }
        if module_loader_config.policy_chain.is_some() {
            tracing::info!("Module policy chain: ENABLED");
        }
    } else {
        tracing::info!("External module imports: DISABLED (use --allow-external-modules to enable)");
    }
    let engine = engine.with_module_loader_config(module_loader_config);

    // ── Execution registry ──────────────────────────────────────────────
    // Use session_db_path for both stateless and stateful modes.
    // For stateless with http_port, add port suffix to avoid sled lock
    // contention when multiple nodes run on the same machine.
    let exec_db_path = match cli.http_port {
        Some(port) => format!("{}/executions-{}", cli.session_db_path, port),
        None => format!("{}/executions", cli.session_db_path),
    };
    let engine = match ExecutionRegistry::new(&exec_db_path) {
        Ok(registry) => {
            tracing::info!("Execution registry opened at {}", exec_db_path);
            engine.with_execution_registry(Arc::new(registry))
        }
        Err(e) => {
            tracing::warn!("Failed to open execution registry at {}: {}. Async execution disabled.", exec_db_path, e);
            engine
        }
    };

    // ── Start transport ─────────────────────────────────────────────────
    if let Some(port) = cli.http_port {
        tracing::info!("Starting Streamable HTTP transport on port {}", port);
        if engine.is_stateful() {
            start_streamable_http(engine, port, |e| McpService::new(e)).await?;
        } else {
            start_streamable_http(engine, port, |e| StatelessMcpService::new(e)).await?;
        }
    } else if let Some(port) = cli.sse_port {
        tracing::info!("Starting SSE transport on port {}", port);
        if engine.is_stateful() {
            start_sse_server(engine, port, |e| McpService::new(e)).await?;
        } else {
            start_sse_server(engine, port, |e| StatelessMcpService::new(e)).await?;
        }
    } else {
        tracing::info!("Starting stdio transport");
        if engine.is_stateful() {
            let service = McpService::new(engine)
                .serve(stdio())
                .await
                .inspect_err(|e| {
                    tracing::error!("serving error: {:?}", e);
                })?;
            service.waiting().await?;
        } else {
            let service = StatelessMcpService::new(engine)
                .serve(stdio())
                .await
                .inspect_err(|e| {
                    tracing::error!("serving error: {:?}", e);
                })?;
            service.waiting().await?;
        }
    }

    Ok(())
}

// ── Streamable HTTP transport (--http-port) ─────────────────────────────

async fn start_streamable_http<S, F>(engine: Engine, port: u16, make_service: F) -> Result<()>
where
    S: ServerHandler + Send + Sync + 'static,
    F: Fn(Engine) -> S + Send + Sync + Clone + 'static,
{
    let bind: std::net::SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let ct = CancellationToken::new();

    let config = StreamableHttpServerConfig {
        bind,
        path: "/mcp".to_string(),
        ct: ct.clone(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (server, mcp_router) = StreamableHttpServer::new(config);

    // Merge MCP router with plain HTTP API router
    let app = mcp_router.merge(api::api_router(engine.clone()));

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("Streamable HTTP server listening on {}", bind);

    let ct_shutdown = ct.child_token();
    let axum_server = axum::serve(listener, app).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("Streamable HTTP server shutting down");
    });
    tokio::spawn(async move {
        if let Err(e) = axum_server.await {
            tracing::error!("Streamable HTTP server error: {:?}", e);
        }
    });

    server.with_service(move || make_service(engine.clone()));

    tokio::signal::ctrl_c().await?;
    tracing::info!("Received Ctrl+C, shutting down");
    ct.cancel();

    Ok(())
}

// ── SSE transport (--sse-port) ──────────────────────────────────────────

async fn start_sse_server<S, F>(engine: Engine, port: u16, make_service: F) -> Result<()>
where
    S: ServerHandler + Send + Sync + 'static,
    F: Fn(Engine) -> S + Send + Sync + Clone + 'static,
{
    let addr = format!("0.0.0.0:{}", port).parse()?;

    let config = SseServerConfig {
        bind: addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (sse_server, sse_router) = SseServer::new(config);

    // Merge SSE router with plain HTTP API router
    let app = sse_router.merge(api::api_router(engine.clone()));

    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    tracing::info!("SSE server listening on {}", sse_server.config.bind);

    let ct = sse_server.config.ct.clone();
    let ct_shutdown = ct.child_token();

    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("SSE server shutting down");
    });

    sse_server.with_service(move || make_service(engine.clone()));

    tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!("SSE server error: {:?}", e);
        }
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("Received Ctrl+C, shutting down SSE server");
    ct.cancel();

    Ok(())
}

// ── WASM module loading ──────────────────────────────────────────────────

/// Parse `--wasm-module name=/path` flags and optional `--wasm-config` JSON file,
/// read `.wasm` bytes from disk, and return validated `WasmModule` entries.
fn load_wasm_modules(
    cli_modules: &[String],
    config_path: &Option<String>,
) -> Result<Vec<WasmModule>> {
    let mut modules = Vec::new();

    // Parse CLI --wasm-module flags (format: name=/path/to/file.wasm[:max_memory])
    for entry in cli_modules {
        let (name, rest) = entry.split_once('=')
            .ok_or_else(|| anyhow::anyhow!(
                "Invalid --wasm-module format: '{}'. Expected name=/path/to/file.wasm[:max_memory]", entry
            ))?;
        let name = name.trim().to_string();
        let rest = rest.trim();
        validate_wasm_name(&name)?;

        // Split path and optional :max_memory suffix.
        // Scan from the right for ':' that isn't part of a Windows drive letter (e.g. C:\).
        let (path, max_memory_bytes) = match rest.rfind(':') {
            Some(pos) if pos > 0 && !rest[..pos].ends_with(|c: char| c.is_ascii_alphabetic() && pos == 1) => {
                let suffix = &rest[pos + 1..];
                if suffix.is_empty() {
                    (rest, None)
                } else {
                    match parse_memory_size(suffix) {
                        Ok(size) => (&rest[..pos], Some(size)),
                        Err(_) => {
                            // Not a valid size suffix — treat entire rest as path
                            (rest, None)
                        }
                    }
                }
            }
            _ => (rest, None),
        };

        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("Failed to read WASM file '{}': {}", path, e))?;
        modules.push(WasmModule { name, bytes, max_memory_bytes });
    }

    // Parse --wasm-config JSON file.
    // String value: {"name": "/path/to/file.wasm"}
    // Object value: {"name": {"path": "/path/to/file.wasm", "max_memory_bytes": 16777216}}
    if let Some(config_path) = config_path {
        let config_str = std::fs::read_to_string(config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read WASM config '{}': {}", config_path, e))?;
        let config: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&config_str)
            .map_err(|e| anyhow::anyhow!("Invalid JSON in WASM config '{}': {}", config_path, e))?;
        for (name, value) in config {
            let (path, max_memory_bytes) = if let Some(s) = value.as_str() {
                (s.to_string(), None)
            } else if let Some(obj) = value.as_object() {
                let path = obj.get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!(
                        "WASM config object for '{}' must have a \"path\" string field", name
                    ))?
                    .to_string();
                let max_mem = obj.get("max_memory_bytes")
                    .map(|v| v.as_u64().ok_or_else(|| anyhow::anyhow!(
                        "WASM config \"max_memory_bytes\" for '{}' must be a positive integer", name
                    )))
                    .transpose()?
                    .map(|v| v as usize);
                (path, max_mem)
            } else {
                anyhow::bail!(
                    "WASM config value for '{}' must be a string path or object, got: {}", name, value
                );
            };
            validate_wasm_name(&name)?;
            let bytes = std::fs::read(&path)
                .map_err(|e| anyhow::anyhow!("Failed to read WASM file '{}': {}", path, e))?;
            modules.push(WasmModule { name, bytes, max_memory_bytes });
        }
    }

    // Check for duplicate names
    let mut seen = std::collections::HashSet::new();
    for m in &modules {
        if !seen.insert(&m.name) {
            anyhow::bail!("Duplicate WASM module name: '{}'", m.name);
        }
    }

    Ok(modules)
}

/// Parse a human-readable memory size string into bytes.
/// Supports raw bytes ("1048576") and suffixes: k/K (KiB), m/M (MiB), g/G (GiB).
fn parse_memory_size(s: &str) -> Result<usize> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("Empty memory size");
    }
    let (num_str, multiplier) = match s.as_bytes().last() {
        Some(b'k' | b'K') => (&s[..s.len() - 1], 1024usize),
        Some(b'm' | b'M') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'g' | b'G') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    let num: usize = num_str.parse()
        .map_err(|_| anyhow::anyhow!("Invalid memory size: '{}'", s))?;
    num.checked_mul(multiplier)
        .ok_or_else(|| anyhow::anyhow!("Memory size overflow: '{}'", s))
}

/// Validate that a WASM module name is a valid JS identifier.
fn validate_wasm_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("WASM module name cannot be empty");
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        anyhow::bail!("WASM module name '{}' must start with a letter, underscore, or dollar sign", name);
    }
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '_' && c != '$' {
            anyhow::bail!("WASM module name '{}' contains invalid character '{}'", name, c);
        }
    }
    Ok(())
}

// ── Fetch header injection rule loading ──────────────────────────────────

/// Load fetch header injection rules from CLI flags and/or a JSON config file.
fn load_fetch_header_rules(
    cli_rules: &[String],
    config_path: &Option<String>,
) -> Result<Vec<engine::fetch::HeaderRule>> {
    let mut rules = Vec::new();

    for entry in cli_rules {
        rules.push(parse_fetch_header_cli(entry)?);
    }

    if let Some(path) = config_path {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read fetch header config '{}': {}", path, e))?;
        let mut file_rules: Vec<engine::fetch::HeaderRule> = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Invalid JSON in fetch header config '{}': {}", path, e))?;
        // Normalize methods to uppercase
        for rule in &mut file_rules {
            rule.methods = rule.methods.iter().map(|m| m.to_uppercase()).collect();
        }
        rules.extend(file_rules);
    }

    Ok(rules)
}

/// Parse a `--fetch-header` CLI string into a `HeaderRule`.
/// Format: host=<host>,header=<name>,value=<val>[,methods=GET;POST]
fn parse_fetch_header_cli(s: &str) -> Result<engine::fetch::HeaderRule> {
    let mut host = None;
    let mut methods = Vec::new();
    let mut header_name = None;
    let mut header_value = None;

    for part in s.split(',') {
        let (key, val) = part.split_once('=')
            .ok_or_else(|| anyhow::anyhow!(
                "Invalid --fetch-header segment '{}'. Expected key=value", part
            ))?;
        match key.trim() {
            "host" => host = Some(val.trim().to_string()),
            "methods" => {
                methods = val.split(';')
                    .map(|m| m.trim().to_uppercase())
                    .filter(|m| !m.is_empty())
                    .collect();
            }
            "header" => header_name = Some(val.trim().to_string()),
            "value" => header_value = Some(val.to_string()),
            other => anyhow::bail!(
                "Unknown key '{}' in --fetch-header. Expected: host, methods, header, value", other
            ),
        }
    }

    let name = header_name.ok_or_else(|| anyhow::anyhow!("--fetch-header missing 'header'"))?;
    let value = header_value.ok_or_else(|| anyhow::anyhow!("--fetch-header missing 'value'"))?;
    let mut headers = std::collections::HashMap::new();
    headers.insert(name, value);

    Ok(engine::fetch::HeaderRule {
        host: host.ok_or_else(|| anyhow::anyhow!("--fetch-header missing 'host'"))?,
        methods,
        headers,
    })
}
