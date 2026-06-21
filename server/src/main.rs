use anyhow::Result;
use rmcp::{ServerHandler, ServiceExt, transport::stdio};
use tracing_subscriber::{self};
use clap::CommandFactory;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
// Legacy HTTP+SSE server transport, vendored from rmcp 0.1.5 (dropped in 1.x).
use rmcp_legacy::transport::sse_server::{SseServer, SseServerConfig};
use serde::{Deserialize, de::{self, MapAccess, Visitor}};
use tokio_util::sync::CancellationToken;
use std::sync::Arc;
use utoipa::OpenApi as _;
use std::fmt;
mod engine;
mod mcp;
mod mcp_dispatch;
mod mcp_sse;
mod api;
mod cluster;
mod cli;
mod session;
use cli::{Cli, FetchHeaderKey, StoreKind};
use engine::{initialize_v8, Engine, WasmModule};
use engine::fetch::FetchConfig;
use engine::fs::FsConfig;
use engine::execution::ExecutionRegistry;
use engine::module_loader::ModuleLoaderConfig;
use engine::subprocess::SubprocessConfig;
use engine::run_js_file::RunJsFilePolicy;
use engine::opa::{PoliciesConfig, build_policy_chain};
use engine::heap_storage::{AnyHeapStorage, HeapStorage, S3HeapStorage, WriteThroughCacheHeapStorage, FileHeapStorage};
use engine::fs_store::FsStore;
use engine::fs_labels::LabelStore;
use engine::opa::{EvalMode, PolicyChain};
use engine::heap_tags::HeapTagStore;
use engine::session_log::SessionLog;
use mcp::{McpService, StatelessMcpService};
use session::{SessionVerifier, JwksKeyStore};
use cluster::{ClusterConfig, ClusterNode};

#[tokio::main]
async fn main() -> Result<()> {
    initialize_v8();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = cli::parse();

    // ── --print-openapi: dump spec and exit ─────────────────────────────
    if cli.print_openapi {
        let spec = api::ApiDoc::openapi();
        println!("{}", serde_json::to_string_pretty(&spec).expect("serialize OpenAPI spec"));
        return Ok(());
    }

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
            learner: cli.join_as_learner,
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
                as_learner: cli.join_as_learner,
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

    let wasm_modules = load_wasm_modules(&cli.wasm_modules, &cli.wasm_config, &cli.wasm_stub_descriptions)?;
    if !wasm_modules.is_empty() {
        tracing::info!("Loaded {} WASM module(s): {}", wasm_modules.len(),
            wasm_modules.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", "));
    }

    let heap_enabled = cli.heap_enabled();
    let fs_enabled = cli.fs_enabled();

    // Heap snapshots run in a V8 SnapshotCreator isolate, which disables
    // WebAssembly. Reject heap + WASM at startup rather than failing at the
    // first execution with "WebAssembly is not an object".
    if heap_enabled && !wasm_modules.is_empty() {
        use clap::CommandFactory;
        Cli::command()
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                "--wasm-module/--wasm-config is incompatible with heap persistence \
                 (--heap-store other than 'none'). V8 heap snapshots require a \
                 SnapshotCreator isolate that disables WebAssembly. Use filesystem \
                 persistence (--fs-store) instead, or drop the WASM modules.",
            )
            .exit();
    }

    // Captured before the engine build consumes them, so fs snapshots can reuse
    // the same shared S3 backend (see the fs-snapshots block below).
    let fs_s3_bucket = cli.s3_bucket.clone();
    let fs_cache_dir = cli.cache_dir.clone();

    // ── Build Engine ────────────────────────────────────────────────────
    // Heap and filesystem persistence are independent axes (see cli::StoreKind).
    // The base engine carries the heap axis; the session log + fs are attached
    // by builders so any combination (neither/heap-only/fs-only/both) is valid.
    let engine = if heap_enabled {
        let heap_storage = match cli.heap_store {
            StoreKind::S3 => {
                let bucket = cli
                    .s3_bucket
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("--heap-store s3 requires --s3-bucket"))?;
                if let Some(cache_dir) = cli.cache_dir.clone() {
                    tracing::info!("Heap: S3 bucket '{}' with write-through cache at {}", bucket, cache_dir);
                    AnyHeapStorage::S3WithFsCache(WriteThroughCacheHeapStorage::new(
                        S3HeapStorage::new(bucket).await,
                        cache_dir,
                    ))
                } else {
                    tracing::info!("Heap: S3 bucket '{}'", bucket);
                    AnyHeapStorage::S3(S3HeapStorage::new(bucket).await)
                }
            }
            StoreKind::Dir => {
                let dir = cli.heap_dir.clone().unwrap_or_else(|| "/tmp/mcp-v8-heaps".to_string());
                tracing::info!("Heap: directory store at {}", dir);
                AnyHeapStorage::File(FileHeapStorage::new(dir))
            }
            StoreKind::None => unreachable!("heap_enabled implies heap_store != none"),
        };
        tracing::info!("Heap persistence: ENABLED");
        Engine::new_stateful(heap_storage, None, None, heap_memory_max_bytes, execution_timeout_secs, cli.max_concurrent_executions)
    } else {
        tracing::info!("Heap persistence: disabled");
        Engine::new_stateless(heap_memory_max_bytes, execution_timeout_secs, cli.max_concurrent_executions)
    };

    // Session log: keys per-session state for EITHER axis (heap resume and/or
    // fs resume), so create it whenever heap or fs persistence is enabled.
    let engine = if heap_enabled || fs_enabled {
        match SessionLog::new(&cli.session_db_path) {
            Ok(log) => {
                tracing::info!("Session log opened at {}", cli.session_db_path);
                let log = if let Some(ref cn) = cluster_node {
                    tracing::info!("Session log will use Raft cluster for replication");
                    log.with_cluster(cn.clone())
                } else {
                    log
                };
                engine.with_session_log(log)
            }
            Err(e) => {
                tracing::warn!("Failed to open session log at {}: {}. Session logging disabled.", cli.session_db_path, e);
                engine
            }
        }
    } else {
        engine
    };

    // Heap-tag store: heap axis only.
    let engine = if heap_enabled {
        let heap_tag_db_path = format!("{}/heap-tags", cli.session_db_path);
        match HeapTagStore::new(&heap_tag_db_path) {
            Ok(store) => {
                tracing::info!("Heap tag store opened at {}", heap_tag_db_path);
                let store = if let Some(ref cn) = cluster_node {
                    store.with_cluster(cn.clone())
                } else {
                    store
                };
                engine.with_heap_tag_store(store)
            }
            Err(e) => {
                tracing::warn!("Failed to open heap tag store at {}: {}. Heap tagging disabled.", heap_tag_db_path, e);
                engine
            }
        }
    } else {
        engine
    };

    let engine = engine.with_wasm_default_max_bytes(wasm_default_max_bytes);
    // Sandbox hardening: all mitigations are opt-in (OFF by default → unhardened).
    let engine = engine.with_hardening(engine::HardeningConfig {
        freeze_ops: cli.harden_freeze_ops,
        neutralize_proxy_details: cli.harden_neutralize_proxy_details,
        neutralize_introspection: cli.harden_neutralize_introspection,
        remove_bootstrap: cli.harden_remove_bootstrap,
        remove_shared_memory: cli.harden_remove_shared_memory,
    });
    let has_wasm_modules = !wasm_modules.is_empty();
    let engine = if has_wasm_modules { engine.with_wasm_modules(wasm_modules) } else { engine };
    let engine = if has_wasm_modules {
        let wasm_stub_config = engine::wasm_stub::WasmStubConfig {
            prefix: cli.wasm_stub_prefix.clone(),
            enabled: cli.wasm_stubs,
        };
        tracing::info!(
            stubs = wasm_stub_config.enabled,
            prefix = %wasm_stub_config.prefix,
            "WASM module stubbing"
        );
        engine.with_wasm_stub_config(wasm_stub_config)
    } else {
        engine
    };

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

    let fs_policy_chain = if let Some(ref config) = policies_config {
        if let Some(ref fs_policies) = config.filesystem {
            let chain = build_policy_chain(fs_policies, "mcp/filesystem", "data.mcp.filesystem.allow")
                .map_err(|e| anyhow::anyhow!("Failed to build filesystem policy chain: {}", e))?;
            tracing::info!("Filesystem policy chain: {} evaluator(s), mode={:?}", fs_policies.policies.len(), fs_policies.mode);
            Some(Arc::new(chain))
        } else {
            None
        }
    } else {
        None
    };

    let fs_snapshot_policy_chain = if let Some(ref config) = policies_config {
        if let Some(ref snap_policies) = config.fs_snapshot {
            let chain = build_policy_chain(snap_policies, "mcp/fs_snapshot", "data.mcp.fs_snapshot.allow")
                .map_err(|e| anyhow::anyhow!("Failed to build fs_snapshot policy chain: {}", e))?;
            tracing::info!("FS snapshot policy chain: {} evaluator(s), mode={:?}", snap_policies.policies.len(), snap_policies.mode);
            Some(Arc::new(chain))
        } else {
            None
        }
    } else {
        None
    };

    let mcp_tools_policy_chain = if let Some(ref config) = policies_config {
        if let Some(ref mcp_tools_policies) = config.mcp_tools {
            let chain = build_policy_chain(mcp_tools_policies, "mcp/tools", "data.mcp.tools.allow")
                .map_err(|e| anyhow::anyhow!("Failed to build MCP tools policy chain: {}", e))?;
            tracing::info!("MCP tools policy chain: {} evaluator(s), mode={:?}", mcp_tools_policies.policies.len(), mcp_tools_policies.mode);
            Some(Arc::new(chain))
        } else {
            None
        }
    } else {
        None
    };

    let subprocess_policy_chain = if let Some(ref config) = policies_config {
        if let Some(ref subprocess_policies) = config.subprocess {
            let chain = build_policy_chain(subprocess_policies, "mcp/subprocess", "data.mcp.subprocess.allow")
                .map_err(|e| anyhow::anyhow!("Failed to build subprocess policy chain: {}", e))?;
            tracing::info!("Subprocess policy chain: {} evaluator(s), mode={:?}", subprocess_policies.policies.len(), subprocess_policies.mode);
            Some(Arc::new(chain))
        } else {
            None
        }
    } else {
        None
    };

    let run_js_file_policy_chain = if let Some(ref config) = policies_config {
        if let Some(ref run_js_file_policies) = config.run_js_file {
            let chain = build_policy_chain(run_js_file_policies, "mcp/run_js_file", "data.mcp.run_js_file.allow")
                .map_err(|e| anyhow::anyhow!("Failed to build run_js_file policy chain: {}", e))?;
            tracing::info!("run_js file policy chain: {} evaluator(s), mode={:?}", run_js_file_policies.policies.len(), run_js_file_policies.mode);
            Some(Arc::new(chain))
        } else {
            None
        }
    } else {
        None
    };

    // ── Fetch policy ───────────────────────────────────────────────────
    let header_rules = load_fetch_header_rules(&cli.fetch_headers, &cli.fetch_header_config)?;
    if !header_rules.is_empty() {
        tracing::info!("Loaded {} fetch header injection rule(s)", header_rules.len());
    }

    let engine = if let Some(chain) = fetch_policy_chain {
        let fetch_config = FetchConfig::new_with_chain(chain)
            .with_header_rules(header_rules);
        engine.with_fetch_config(fetch_config)
    } else {
        engine
    };

    // ── Filesystem policy ────────────────────────────────────────────────
    // A mount needs the fs surface present, so when snapshots are enabled but
    // no fs policy was supplied, default to an allow-all policy chain.
    let engine = if let Some(chain) = fs_policy_chain {
        engine.with_fs_config(FsConfig::new(chain).with_passthrough(cli.fs_passthrough))
    } else if fs_enabled {
        engine.with_fs_config(
            FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All)))
                .with_passthrough(cli.fs_passthrough),
        )
    } else {
        engine
    };

    // ── Filesystem snapshots (content-addressed store + label/reflog) ─────
    let engine = if fs_enabled {
        let store_dir = cli
            .fs_dir
            .clone()
            .unwrap_or_else(|| format!("{}/fs-blobs", cli.session_db_path));
        let labels_db = cli
            .fs_labels_db
            .clone()
            .unwrap_or_else(|| format!("{}/fs-labels", cli.session_db_path));

        let fs_on_s3 = cli.fs_store == StoreKind::S3;

        // Labels replicate cluster-wide, but blobs/manifests are only shared if
        // they sit on shared storage. Node-local file blobs are single-node
        // only: in a cluster a label advanced on one node would resolve on
        // another to a manifest that node is missing. Refuse that combination up
        // front rather than failing later after a rebalance.
        if cluster_node.is_some() && !fs_on_s3 {
            Cli::command()
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "--fs-store s3 (with --s3-bucket) is required in cluster mode: \
                     fs snapshot blobs must live on shared storage. Node-local \
                     (--fs-store dir) blobs are single-node only.",
                )
                .exit();
        }

        // Back the blob store with shared S3 when --fs-store s3, so mounts
        // resolve on any node; otherwise use node-local files (single-node).
        let backend: Arc<dyn HeapStorage> = if fs_on_s3 {
            let bucket = fs_s3_bucket
                .clone()
                .unwrap_or_else(|| {
                    Cli::command()
                        .error(
                            clap::error::ErrorKind::MissingRequiredArgument,
                            "--fs-store s3 requires --s3-bucket.",
                        )
                        .exit()
                });
            if let Some(cache_dir) = &fs_cache_dir {
                tracing::info!(
                    "FS snapshots: shared S3 blob storage (bucket {}) with write-through cache at {}",
                    bucket,
                    cache_dir
                );
                Arc::new(WriteThroughCacheHeapStorage::new(
                    S3HeapStorage::new(bucket.clone()).await,
                    cache_dir.clone(),
                ))
            } else {
                tracing::info!("FS snapshots: shared S3 blob storage (bucket {})", bucket);
                Arc::new(S3HeapStorage::new(bucket.clone()).await)
            }
        } else {
            Arc::new(FileHeapStorage::new(&store_dir))
        };
        let store = Arc::new(FsStore::new(backend));
        match LabelStore::new(&labels_db) {
            Ok(labels) => {
                tracing::info!(
                    "FS snapshots: ENABLED (blobs at {}, labels at {})",
                    store_dir,
                    labels_db
                );
                let labels = if let Some(ref cn) = cluster_node {
                    tracing::info!("FS label writes will route through the Raft cluster leader");
                    labels.with_cluster(cn.clone())
                } else {
                    labels
                };
                let engine = engine.with_fs_snapshots(store, Arc::new(labels));
                if let Some(chain) = fs_snapshot_policy_chain {
                    tracing::info!("FS snapshot pointer moves are policy-gated");
                    engine.with_fs_snapshot_policy(chain)
                } else {
                    engine
                }
            }
            Err(e) => {
                tracing::error!("FS snapshots: failed to open label store at {}: {}. Disabled.", labels_db, e);
                engine
            }
        }
    } else {
        engine
    };

    // ── Module loader config ─────────────────────────────────────────────
    let module_loader_config = ModuleLoaderConfig {
        allow_external: cli.allow_external_modules,
        policy_chain: modules_policy_chain,
    };
    if cli.allow_external_modules {
        tracing::info!("External module imports: ENABLED");
        if module_loader_config.policy_chain.is_some() {
            tracing::info!("Module policy chain: ENABLED");
        }
    } else {
        tracing::info!("External module imports: DISABLED (use --allow-external-modules to enable)");
    }
    let engine = engine.with_module_loader_config(module_loader_config);

    // ── Subprocess policy ──────────────────────────────────────────────
    let engine = if let Some(chain) = subprocess_policy_chain {
        engine.with_subprocess_config(SubprocessConfig::new(chain))
    } else {
        engine
    };

    // ── run_js file-path reads ─────────────────────────────────────────
    // OFF by default. `--allow-run-js-file` allows any server-readable path;
    // a `run_js_file` policy in --policies-json gates reads per path. The flag
    // wins over a configured policy (it is the explicit "allow all" switch).
    let engine = if cli.allow_run_js_file {
        if run_js_file_policy_chain.is_some() {
            tracing::warn!("--allow-run-js-file overrides the configured run_js_file policy (all paths allowed)");
        }
        tracing::info!("run_js file-path reads: ENABLED (allow all server-readable paths)");
        engine.with_run_js_file_policy(RunJsFilePolicy::AllowAll)
    } else if let Some(chain) = run_js_file_policy_chain {
        tracing::info!("run_js file-path reads: ENABLED (policy-gated)");
        engine.with_run_js_file_policy(RunJsFilePolicy::Policy(chain))
    } else {
        tracing::info!("run_js file-path reads: DISABLED (enable with --allow-run-js-file or a run_js_file policy)");
        engine
    };


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

    // ── MCP server modules ────────────────────────────────────────────────
    let mcp_server_configs = load_mcp_server_configs(&cli.mcp_servers, &cli.mcp_config)?;
    let engine = if !mcp_server_configs.is_empty() {
        tracing::info!("Connecting to {} MCP server(s)...", mcp_server_configs.len());
        let stub_config = engine::mcp_client::StubConfig {
            prefix: cli.mcp_stub_prefix.clone(),
            enabled: cli.mcp_stubs,
        };
        tracing::info!(
            stubs = stub_config.enabled,
            prefix = %stub_config.prefix,
            "Upstream MCP tool stubbing"
        );
        let manager = engine::mcp_client::McpClientManager::connect(mcp_server_configs).await
            .map_err(|e| anyhow::anyhow!("MCP server connection failed: {}", e))?
            .with_stub_config(stub_config);
        tracing::info!("All MCP servers connected. JS code can use mcp.callTool(), mcp.listTools(), mcp.servers");
        let engine = engine.with_mcp_client_manager(manager);
        if let Some(chain) = mcp_tools_policy_chain {
            tracing::info!("MCP tools policy: ENABLED");
            engine.with_mcp_tools_policy_chain(chain)
        } else {
            engine
        }
    } else {
        engine
    };

    // ── Prompt / tool description overrides ──────────────────────────────
    // Both flags accept inline text or, with a leading `@`, a path to a file.
    let engine = if let Some(ref value) = cli.instructions {
        let text = resolve_text_or_file(value, "--instructions")?;
        tracing::info!("Overriding MCP server instructions ({} chars)", text.len());
        engine.with_instructions_override(text)
    } else {
        engine
    };
    let engine = if let Some(ref value) = cli.run_js_description {
        let text = resolve_text_or_file(value, "--run-js-description")?;
        tracing::info!("Overriding run_js tool description ({} chars)", text.len());
        engine.with_run_js_description_override(text)
    } else {
        engine
    };

    // ── Build session verifier (if --jwks-url) ─────────────────────────
    let session_verifier: Option<Arc<SessionVerifier>> = if let Some(ref jwks_url) = cli.jwks_url {
        tracing::info!("Fetching JWKS keys from {}", jwks_url);
        let key_store = JwksKeyStore::new(jwks_url.clone()).await
            .map_err(|e| anyhow::anyhow!("Failed to initialize JWKS key store: {}", e))?;
        Some(Arc::new(SessionVerifier::new(Arc::new(key_store))))
    } else {
        None
    };

    // ── Start transport ─────────────────────────────────────────────────
    // McpService (session-capable) is used whenever any per-session state axis
    // is on (heap and/or fs); StatelessMcpService only when neither is.
    let bind_host = cli.bind_host.clone();
    if let Some(port) = cli.http_port {
        tracing::info!("Starting Streamable HTTP transport on port {}", port);
        if engine.session_capable() {
            let verifier = session_verifier.clone();
            start_streamable_http(engine, bind_host, port, move |e| McpService::new(e, verifier.clone())).await?;
        } else {
            let verifier = session_verifier.clone();
            start_streamable_http(engine, bind_host, port, move |e| StatelessMcpService::new(e, verifier.clone())).await?;
        }
    } else if let Some(port) = cli.sse_port {
        // Legacy HTTP+SSE transport, served by the vendored rmcp 0.1.5 SSE
        // server. No MCP tasks support here — use --http-port for tasks.
        tracing::info!("Starting legacy HTTP+SSE transport on port {} (no MCP tasks; use --http-port for tasks)", port);
        let verifier = session_verifier.clone();
        start_sse_server(engine, bind_host, port, verifier).await?;
    } else {
        tracing::info!("Starting stdio transport");
        if engine.session_capable() {
            let service = McpService::new(engine, None)
                .serve(stdio())
                .await
                .inspect_err(|e| {
                    tracing::error!("serving error: {:?}", e);
                })?;
            service.waiting().await?;
        } else {
            let service = StatelessMcpService::new(engine, None)
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

// Resolve the listen address for the HTTP transports from `--bind-host`
// (env MCP_V8_BIND_HOST). Defaults to all IPv4 interfaces (0.0.0.0); set "::"
// for a dual-stack IPv6 listener, required to be reachable over private
// networks (e.g. Railway) that resolve service hostnames to IPv6.
fn resolve_bind_addr(host: &str, port: u16) -> Result<std::net::SocketAddr> {
    let ip: std::net::IpAddr = host
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --bind-host '{host}': {e}"))?;
    Ok(std::net::SocketAddr::new(ip, port))
}

// ── Streamable HTTP transport (--http-port) ─────────────────────────────

async fn start_streamable_http<S, F>(engine: Engine, host: String, port: u16, make_service: F) -> Result<()>
where
    S: ServerHandler + Send + Sync + 'static,
    F: Fn(Engine) -> S + Send + Sync + Clone + 'static,
{
    let bind: std::net::SocketAddr = resolve_bind_addr(&host, port)?;
    let ct = CancellationToken::new();

    // The Streamable HTTP transport (rmcp 1.x) is a tower service mounted at
    // /mcp. It natively serves the MCP `tasks/*` utility (SEP-1319) for tools
    // marked `execution(task_support = ...)` — here, `run_js`. A fresh service
    // (and thus a fresh per-connection session id) is created per session.
    let factory_engine = engine.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(make_service(factory_engine.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );

    // Serve OpenAPI JSON spec at /api-doc/openapi.json
    let openapi_spec = api::ApiDoc::openapi();
    let openapi_json = serde_json::to_string(&openapi_spec).unwrap_or_default();
    let openapi_route = axum::Router::new()
        .route("/api-doc/openapi.json", axum::routing::get(move || {
            let json = openapi_json.clone();
            async move {
                axum::response::Response::builder()
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(json))
                    .unwrap()
            }
        }));

    // Mount the MCP service at /mcp alongside the plain HTTP API and openapi route.
    let app = axum::Router::new()
        .nest_service("/mcp", mcp_service)
        .merge(api::api_router(engine.clone()))
        .merge(openapi_route);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("Streamable HTTP server listening on {}", bind);

    let ct_shutdown = ct.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("Received Ctrl+C, shutting down");
            ct_shutdown.cancel();
        })
        .await?;

    Ok(())
}

// ── Legacy HTTP+SSE transport (--sse-port) ──────────────────────────────
//
// Served by the vendored rmcp 0.1.5 SSE server, which rmcp 1.x removed. One
// `SseService` handles both stateful and stateless modes; it does not advertise
// MCP tasks (use the Streamable HTTP transport for those).

async fn start_sse_server(
    engine: Engine,
    host: String,
    port: u16,
    verifier: Option<Arc<SessionVerifier>>,
) -> Result<()> {
    let addr = resolve_bind_addr(&host, port)?;

    let config = SseServerConfig {
        bind: addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (sse_server, sse_router) = SseServer::new(config);

    // Serve OpenAPI JSON spec at /api-doc/openapi.json
    let openapi_json = serde_json::to_string(&api::ApiDoc::openapi()).unwrap_or_default();
    let openapi_route = axum::Router::new()
        .route("/api-doc/openapi.json", axum::routing::get(move || {
            let json = openapi_json.clone();
            async move {
                axum::response::Response::builder()
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(json))
                    .unwrap()
            }
        }));

    let app = sse_router
        .merge(api::api_router(engine.clone()))
        .merge(openapi_route);

    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    tracing::info!("SSE server listening on {}", sse_server.config.bind);

    let ct = sse_server.config.ct.clone();
    let ct_shutdown = ct.child_token();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("SSE server shutting down");
    });

    sse_server.with_service(move || mcp_sse::SseService::new(engine.clone(), verifier.clone()));

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
    stub_descriptions: &[String],
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
        modules.push(WasmModule { name, bytes, max_memory_bytes, description: None });
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
            let (path, max_memory_bytes, description) = if let Some(s) = value.as_str() {
                (s.to_string(), None, None)
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
                let description = obj.get("description")
                    .map(|v| v.as_str().ok_or_else(|| anyhow::anyhow!(
                        "WASM config \"description\" for '{}' must be a string", name
                    )))
                    .transpose()?
                    .map(|s| s.to_string());
                (path, max_mem, description)
            } else {
                anyhow::bail!(
                    "WASM config value for '{}' must be a string path or object, got: {}", name, value
                );
            };
            validate_wasm_name(&name)?;
            let bytes = std::fs::read(&path)
                .map_err(|e| anyhow::anyhow!("Failed to read WASM file '{}': {}", path, e))?;
            modules.push(WasmModule { name, bytes, max_memory_bytes, description });
        }
    }

    // Apply --wasm-stub-description overrides (format: name=description text).
    // These take precedence over a description set inline in --wasm-config.
    for entry in stub_descriptions {
        let (name, desc) = entry.split_once('=')
            .ok_or_else(|| anyhow::anyhow!(
                "Invalid --wasm-stub-description format: '{}'. Expected name=description", entry
            ))?;
        let name = name.trim();
        let module = modules.iter_mut().find(|m| m.name == name)
            .ok_or_else(|| anyhow::anyhow!(
                "--wasm-stub-description refers to unknown WASM module '{}'. \
                 Load it first with --wasm-module or --wasm-config.", name
            ))?;
        module.description = Some(desc.to_string());
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

/// Resolve a CLI value that may be either inline text or a `@path` file
/// reference. A leading `@` means "read the rest as a file path"; `@@` escapes
/// to a literal value beginning with `@`. Any other value is returned verbatim.
fn resolve_text_or_file(value: &str, flag: &str) -> Result<String> {
    match value.strip_prefix('@') {
        Some(rest) if rest.starts_with('@') => Ok(rest.to_string()),
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read {} file '{}': {}", flag, path, e)),
        None => Ok(value.to_string()),
    }
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchHeaderConfigRule {
    host: String,
    #[serde(default)]
    methods: Vec<String>,
    #[serde(default)]
    headers: Option<StaticHeadersConfig>,
    #[serde(default)]
    auth: Option<FetchHeaderAuthConfig>,
}

#[derive(Debug)]
struct StaticHeadersConfig(std::collections::HashMap<String, String>);

impl<'de> Deserialize<'de> for StaticHeadersConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StaticHeadersVisitor;

        impl<'de> Visitor<'de> for StaticHeadersVisitor {
            type Value = StaticHeadersConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object mapping header names to header values")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut headers = std::collections::HashMap::new();

                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if headers.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!("duplicate field `{}`", key)));
                    }
                }

                Ok(StaticHeadersConfig(headers))
            }
        }

        deserializer.deserialize_map(StaticHeadersVisitor)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchHeaderAuthConfig {
    #[serde(rename = "type")]
    auth_type: String,
    header: String,
    token_url: String,
    client_id: String,
    client_secret: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default = "engine::fetch::default_refresh_buffer_secs")]
    refresh_buffer_secs: u64,
}

impl FetchHeaderConfigRule {
    fn into_runtime_rule(self) -> Result<engine::fetch::HeaderRule> {
        let host = self.host;
        let methods = self.methods;

        match (self.headers, self.auth) {
            (Some(_), Some(_)) => anyhow::bail!(
                "Fetch header config rule for host '{}' cannot define both 'headers' and 'auth'",
                host
            ),
            (None, None) => anyhow::bail!(
                "Fetch header config rule for host '{}' must define either 'headers' or 'auth'",
                host
            ),
            (Some(headers), None) => engine::fetch::HeaderRule::new(
                host,
                methods,
                engine::fetch::HeaderInjection::Static { headers: headers.0 },
            ),
            (None, Some(auth)) => {
                if auth.auth_type != "oauth_client_credentials" {
                    anyhow::bail!(
                        "Unsupported fetch auth type '{}' for host '{}'",
                        auth.auth_type,
                        host
                    );
                }

                engine::fetch::HeaderRule::oauth_client_credentials(
                    host,
                    methods,
                    engine::fetch::OAuthClientCredentialsConfig {
                        header_name: auth.header,
                        token_url: auth.token_url,
                        client_id: auth.client_id,
                        client_secret: auth.client_secret,
                        scope: auth.scope,
                        refresh_buffer_secs: auth.refresh_buffer_secs,
                    },
                )
            }
        }
    }
}

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
        let file_rules: Vec<FetchHeaderConfigRule> = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Invalid JSON in fetch header config '{}': {}", path, e))?;
        for rule in file_rules {
            rules.push(rule.into_runtime_rule()?);
        }
    }

    Ok(rules)
}

/// Parse a `--fetch-header` CLI string into a `HeaderRule`.
/// Format: host=<host>,header=<name>,value=<val>[,methods=GET;POST]
/// Or:     host=<host>,header=<name>,token_url=<url>,client_id=<id>,client_secret=<secret>[,scope=<scope>][,methods=GET;POST][,refresh_buffer_secs=30]
fn parse_fetch_header_cli(s: &str) -> Result<engine::fetch::HeaderRule> {
    let mut host = None;
    let mut methods = Vec::new();
    let mut header_name = None;
    let mut header_value = None;
    let mut token_url = None;
    let mut client_id = None;
    let mut client_secret = None;
    let mut scope = None;
    let mut refresh_buffer_secs = None;

    for part in s.split(',') {
        let (key, val) = part.split_once('=')
            .ok_or_else(|| anyhow::anyhow!(
                "Invalid --fetch-header segment '{}'. Expected key=value", part
            ))?;
        let parsed_key = FetchHeaderKey::from_key(key.trim()).ok_or_else(|| anyhow::anyhow!(
            "Unknown key '{}' in --fetch-header. Expected: {}",
            key.trim(),
            FetchHeaderKey::expected()
        ))?;
        match parsed_key {
            FetchHeaderKey::Host => host = Some(val.trim().to_string()),
            FetchHeaderKey::Methods => methods = val.split(';').map(|m| m.to_string()).collect(),
            FetchHeaderKey::Header => header_name = Some(val.trim().to_string()),
            FetchHeaderKey::Value => header_value = Some(val.to_string()),
            FetchHeaderKey::TokenUrl => token_url = Some(val.trim().to_string()),
            FetchHeaderKey::ClientId => client_id = Some(val.trim().to_string()),
            FetchHeaderKey::ClientSecret => client_secret = Some(val.to_string()),
            FetchHeaderKey::Scope => scope = Some(val.trim().to_string()),
            FetchHeaderKey::RefreshBufferSecs => {
                refresh_buffer_secs = Some(val.trim().parse::<u64>().map_err(|e| anyhow::anyhow!(
                    "Invalid 'refresh_buffer_secs' value '{}': {}",
                    val.trim(),
                    e
                ))?)
            }
        }
    }

    let host = host.ok_or_else(|| anyhow::anyhow!("--fetch-header missing 'host'"))?;
    let header_name = header_name.ok_or_else(|| anyhow::anyhow!("--fetch-header missing 'header'"))?;
    let has_dynamic_keys = token_url.is_some()
        || client_id.is_some()
        || client_secret.is_some()
        || scope.is_some()
        || refresh_buffer_secs.is_some();

    match (header_value, has_dynamic_keys) {
        (Some(_), true) => anyhow::bail!(
            "--fetch-header cannot mix static 'value' with dynamic oauth keys"
        ),
        (Some(value), false) => engine::fetch::HeaderRule::static_header(
            host,
            methods,
            header_name,
            value,
        ),
        (None, true) => engine::fetch::HeaderRule::oauth_client_credentials(
            host,
            methods,
            engine::fetch::OAuthClientCredentialsConfig {
                header_name,
                token_url: token_url.ok_or_else(|| anyhow::anyhow!(
                    "--fetch-header missing 'token_url' for dynamic oauth rule"
                ))?,
                client_id: client_id.ok_or_else(|| anyhow::anyhow!(
                    "--fetch-header missing 'client_id' for dynamic oauth rule"
                ))?,
                client_secret: client_secret.ok_or_else(|| anyhow::anyhow!(
                    "--fetch-header missing 'client_secret' for dynamic oauth rule"
                ))?,
                scope,
                refresh_buffer_secs: refresh_buffer_secs
                    .unwrap_or_else(engine::fetch::default_refresh_buffer_secs),
            },
        ),
        (None, false) => anyhow::bail!(
            "--fetch-header must provide either 'value' for a static rule or the full dynamic oauth key set: token_url, client_id, client_secret"
        ),
    }
}

// ── MCP server module loading ────────────────────────────────────────────

/// Parse `--mcp-server` flags and optional `--mcp-config` JSON file into
/// `McpServerConfig` entries.
fn load_mcp_server_configs(
    cli_servers: &[String],
    config_path: &Option<String>,
) -> Result<Vec<engine::mcp_client::McpServerConfig>> {
    use engine::mcp_client::{McpServerConfig, McpServerTransport};
    let mut configs = Vec::new();

    // Parse CLI --mcp-server flags
    for entry in cli_servers {
        let (name, rest) = entry.split_once('=')
            .ok_or_else(|| anyhow::anyhow!(
                "Invalid --mcp-server format: '{}'. Expected name=transport:...", entry
            ))?;
        let name = name.trim().to_string();

        if let Some(cmd_args) = rest.strip_prefix("stdio:") {
            // stdio:command:arg1:arg2
            let parts: Vec<&str> = cmd_args.split(':').collect();
            if parts.is_empty() || parts[0].is_empty() {
                anyhow::bail!("--mcp-server stdio transport requires a command");
            }
            configs.push(McpServerConfig {
                name,
                transport: McpServerTransport::Stdio {
                    command: parts[0].to_string(),
                    args: parts[1..].iter().map(|s| s.to_string()).collect(),
                    env: std::collections::HashMap::new(),
                },
            });
        } else if let Some(url) = rest.strip_prefix("sse:") {
            configs.push(McpServerConfig {
                name,
                transport: McpServerTransport::Sse {
                    url: url.to_string(),
                },
            });
        } else {
            anyhow::bail!(
                "Invalid --mcp-server transport for '{}': must start with 'stdio:' or 'sse:'. Got: '{}'",
                name, rest
            );
        }
    }

    // Parse --mcp-config JSON file
    if let Some(config_path) = config_path {
        let content = std::fs::read_to_string(config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read MCP config '{}': {}", config_path, e))?;
        let file_configs: Vec<McpServerConfig> = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Invalid JSON in MCP config '{}': {}", config_path, e))?;
        configs.extend(file_configs);
    }

    // Check for duplicate names
    let mut seen = std::collections::HashSet::new();
    for c in &configs {
        if !seen.insert(&c.name) {
            anyhow::bail!("Duplicate MCP server name: '{}'", c.name);
        }
    }

    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::{
        load_fetch_header_rules, load_mcp_server_configs, load_wasm_modules,
        parse_fetch_header_cli, resolve_text_or_file,
    };

    // ── Systematic structured-flag drift guard ──────────────────────────
    // Every flag registered in `cli::structured_args()` has its --help generated
    // from a Grammar; each must also round-trip its documented shape through the
    // real parser below. The two registries must list the same flags, so a
    // structured flag cannot ship generated help without a parser round-trip
    // (or vice versa) — that mismatch fails `every_structured_arg_has_grammar_and_parses`.

    fn check_fetch_headers() -> anyhow::Result<()> {
        parse_fetch_header_cli("host=api.example.com,header=Authorization,value=Bearer x")?;
        parse_fetch_header_cli(
            "host=api.example.com,header=Authorization,\
             token_url=https://issuer.example.com/token,client_id=a,client_secret=b",
        )?;
        Ok(())
    }

    fn check_mcp_servers() -> anyhow::Result<()> {
        use crate::engine::mcp_client::McpServerTransport;

        let configs = load_mcp_server_configs(
            &[
                "weather=stdio:python:server.py:--verbose".to_string(),
                "remote=sse:http://127.0.0.1:9000/sse".to_string(),
            ],
            &None,
        )?;
        anyhow::ensure!(matches!(configs[0].transport, McpServerTransport::Stdio { .. }));
        anyhow::ensure!(matches!(configs[1].transport, McpServerTransport::Sse { .. }));
        Ok(())
    }

    fn check_wasm_modules() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("module.wasm");
        // load_wasm_modules only reads the bytes here; contents are not validated.
        std::fs::write(&path, b"\0asm")?;
        let path = path.display();

        let modules = load_wasm_modules(
            &[
                format!("plain={path}"),
                format!("capped={path}:16m"),
                format!("rawbytes={path}:1048576"),
            ],
            &None,
            &[],
        )?;
        anyhow::ensure!(modules[0].max_memory_bytes.is_none());
        anyhow::ensure!(modules[1].max_memory_bytes == Some(16 * 1024 * 1024));
        anyhow::ensure!(modules[2].max_memory_bytes == Some(1_048_576));
        Ok(())
    }

    fn check_wasm_stub_descriptions() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("module.wasm");
        std::fs::write(&path, b"\0asm")?;

        let modules = load_wasm_modules(
            &[format!("math={}", path.display())],
            &None,
            &["math=Adds two numbers and returns the sum".to_string()],
        )?;
        anyhow::ensure!(
            modules[0].description.as_deref() == Some("Adds two numbers and returns the sum")
        );
        Ok(())
    }

    fn check_peers() -> anyhow::Result<()> {
        let (peers, peer_addrs) = crate::cluster::ClusterConfig::parse_peers(&[
            "node2@10.0.0.2:4000".to_string(),
            "10.0.0.3:4000".to_string(),
        ]);
        anyhow::ensure!(peers == ["10.0.0.2:4000", "10.0.0.3:4000"]);
        anyhow::ensure!(peer_addrs.get("node2").map(String::as_str) == Some("10.0.0.2:4000"));
        Ok(())
    }

    /// Parser round-trip per structured flag. Must list the same arg ids as
    /// `cli::structured_args()` (enforced by the test below).
    fn structured_arg_checks() -> Vec<(&'static str, fn() -> anyhow::Result<()>)> {
        vec![
            ("fetch_headers", check_fetch_headers),
            ("mcp_servers", check_mcp_servers),
            ("wasm_modules", check_wasm_modules),
            ("wasm_stub_descriptions", check_wasm_stub_descriptions),
            ("peers", check_peers),
        ]
    }

    #[test]
    fn every_structured_arg_has_grammar_and_parses() {
        use std::collections::BTreeSet;

        let checks = structured_arg_checks();
        let check_ids: BTreeSet<&str> = checks.iter().map(|(id, _)| *id).collect();
        let grammar_ids: BTreeSet<&str> = crate::cli::Cli::structured_arg_ids().into_iter().collect();

        // Help registry (cli.rs) and parse-check registry (here) must cover the
        // exact same flags — so neither side can grow without the other.
        assert_eq!(
            check_ids, grammar_ids,
            "structured-arg registries disagree; every flag needs BOTH a Grammar (cli.rs) and a parse-check (main.rs)"
        );

        for (arg_id, check) in checks {
            check().unwrap_or_else(|err| {
                panic!("documented grammar for --{} must parse: {err}", arg_id.replace('_', "-"))
            });
        }
    }

    #[test]
    fn resolve_text_or_file_returns_inline_text_verbatim() {
        let resolved = resolve_text_or_file("just some text", "--instructions")
            .expect("inline text should resolve");
        assert_eq!(resolved, "just some text");
    }

    #[test]
    fn resolve_text_or_file_reads_file_for_at_prefix() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("prompt.txt");
        std::fs::write(&path, "from a file\n").expect("file should be written");

        let arg = format!("@{}", path.display());
        let resolved = resolve_text_or_file(&arg, "--instructions")
            .expect("file value should resolve");
        assert_eq!(resolved, "from a file\n");
    }

    #[test]
    fn resolve_text_or_file_escapes_double_at_to_literal() {
        let resolved = resolve_text_or_file("@@literal", "--instructions")
            .expect("escaped value should resolve");
        assert_eq!(resolved, "@literal");
    }

    #[test]
    fn resolve_text_or_file_errors_on_missing_file() {
        let err = resolve_text_or_file("@/no/such/file/here.txt", "--run-js-description")
            .expect_err("missing file should error");
        assert!(err.to_string().contains("Failed to read --run-js-description file"));
    }

    #[test]
    fn parse_fetch_header_cli_supports_static_rules() {
        let rule = parse_fetch_header_cli(
            "host=api.example.com,header=Authorization,value=Bearer fixed,methods=get;post",
        )
        .expect("static rule should parse");

        let headers = rule.static_headers().expect("static headers expected");
        assert_eq!(
            headers.get("Authorization").map(|value| value.as_str()),
            Some("Bearer fixed"),
        );
        assert!(rule.dynamic_auth().is_none());
        assert_eq!(rule.methods(), &["GET".to_string(), "POST".to_string()]);
    }

    #[test]
    fn parse_fetch_header_cli_supports_dynamic_rules() {
        let rule = parse_fetch_header_cli(
            "host=api.example.com,header=Authorization,token_url=https://issuer/token,client_id=abc,client_secret=xyz,scope=read:all,methods=GET;POST,refresh_buffer_secs=45",
        )
        .expect("dynamic rule should parse");

        let auth = rule.dynamic_auth().expect("dynamic auth expected");
        assert_eq!(auth.header_name, "Authorization");
        assert_eq!(auth.token_url, "https://issuer/token");
        assert_eq!(auth.client_id, "abc");
        assert_eq!(auth.client_secret, "xyz");
        assert_eq!(auth.scope.as_deref(), Some("read:all"));
        assert_eq!(auth.refresh_buffer_secs, 45);
        assert_eq!(rule.methods(), &["GET".to_string(), "POST".to_string()]);
    }

    #[test]
    fn parse_fetch_header_cli_rejects_unknown_key() {
        let err = parse_fetch_header_cli(
            "host=api.example.com,header=Authorization,value=Bearer fixed,bogus=1",
        )
        .expect_err("unknown key should fail");

        let message = err.to_string();
        assert!(message.contains("Unknown key 'bogus'"));
        // The accepted-key list in the error is generated from the canonical
        // table, so every key the parser accepts is advertised on failure.
        assert!(message.contains(&super::FetchHeaderKey::expected()));
    }

    #[test]
    fn parse_fetch_header_cli_rejects_mixed_static_and_dynamic_keys() {
        let err = parse_fetch_header_cli(
            "host=api.example.com,header=Authorization,value=Bearer nope,token_url=https://issuer/token,client_id=abc,client_secret=xyz",
        )
        .expect_err("mixed rule should fail");

        assert!(err.to_string().contains("cannot mix"));
    }

    #[test]
    fn parse_fetch_header_cli_rejects_empty_host() {
        let err = parse_fetch_header_cli(
            "host=   ,header=Authorization,value=Bearer fixed",
        )
        .expect_err("blank host should fail");

        assert!(err.to_string().contains("'host' cannot be empty"));
    }

    #[test]
    fn parse_fetch_header_cli_rejects_empty_header_name() {
        let err = parse_fetch_header_cli(
            "host=api.example.com,header=   ,value=Bearer fixed",
        )
        .expect_err("blank header name should fail");

        assert!(err.to_string().contains("'header' cannot be empty"));
    }

    #[test]
    fn parse_fetch_header_cli_rejects_empty_dynamic_required_values() {
        let token_url_err = parse_fetch_header_cli(
            "host=api.example.com,header=Authorization,token_url=   ,client_id=abc,client_secret=xyz",
        )
        .expect_err("blank token_url should fail");
        assert!(token_url_err.to_string().contains("'token_url' cannot be empty"));

        let client_id_err = parse_fetch_header_cli(
            "host=api.example.com,header=Authorization,token_url=https://issuer/token,client_id=   ,client_secret=xyz",
        )
        .expect_err("blank client_id should fail");
        assert!(client_id_err.to_string().contains("'client_id' cannot be empty"));

        let client_secret_err = parse_fetch_header_cli(
            "host=api.example.com,header=Authorization,token_url=https://issuer/token,client_id=abc,client_secret=   ",
        )
        .expect_err("blank client_secret should fail");
        assert!(client_secret_err.to_string().contains("'client_secret' cannot be empty"));
    }

    #[test]
    fn load_fetch_header_rules_supports_dynamic_json_rules_and_normalizes_methods() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("fetch-rules.json");
        std::fs::write(
            &path,
            r#"[{
                "host": "api.example.com",
                "methods": ["get", "post"],
                "auth": {
                    "type": "oauth_client_credentials",
                    "header": "Authorization",
                    "token_url": "https://issuer/token",
                    "client_id": "abc",
                    "client_secret": "xyz",
                    "scope": "read:all"
                }
            }]"#,
        )
        .expect("config should be written");

        let rules = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect("json rules should parse");

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].methods(), &["GET".to_string(), "POST".to_string()]);

        let auth = rules[0].dynamic_auth().expect("dynamic auth expected");
        assert_eq!(auth.header_name, "Authorization");
        assert_eq!(auth.scope.as_deref(), Some("read:all"));
        assert_eq!(auth.refresh_buffer_secs, 30);
    }

    #[test]
    fn load_fetch_header_rules_rejects_mixed_headers_and_auth_json_rules() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("fetch-rules.json");
        std::fs::write(
            &path,
            r#"[{
                "host": "api.example.com",
                "headers": {"Authorization": "Bearer fixed"},
                "auth": {
                    "type": "oauth_client_credentials",
                    "header": "Authorization",
                    "token_url": "https://issuer/token",
                    "client_id": "abc",
                    "client_secret": "xyz"
                }
            }]"#,
        )
        .expect("config should be written");

        let err = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect_err("mixed headers/auth json rule should fail");

        assert!(err.to_string().contains("cannot define both 'headers' and 'auth'"));
    }

    #[test]
    fn load_fetch_header_rules_rejects_unsupported_auth_type() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("fetch-rules.json");
        std::fs::write(
            &path,
            r#"[{
                "host": "api.example.com",
                "auth": {
                    "type": "basic",
                    "header": "Authorization",
                    "token_url": "https://issuer/token",
                    "client_id": "abc",
                    "client_secret": "xyz"
                }
            }]"#,
        )
        .expect("config should be written");

        let err = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect_err("unsupported auth type should fail");

        assert!(err.to_string().contains("Unsupported fetch auth type"));
    }

    #[test]
    fn load_fetch_header_rules_rejects_unknown_json_fields() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("fetch-rules.json");
        std::fs::write(
            &path,
            r#"[{
                "host": "api.example.com",
                "headers": {"Authorization": "Bearer fixed"},
                "unexpected": true
            }]"#,
        )
        .expect("config should be written");

        let err = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect_err("unknown json fields should fail");

        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn load_fetch_header_rules_rejects_empty_static_headers_map() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("fetch-rules.json");
        std::fs::write(
            &path,
            r#"[{
                "host": "api.example.com",
                "headers": {}
            }]"#,
        )
        .expect("config should be written");

        let err = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect_err("empty static headers map should fail");

        assert!(err.to_string().contains("static headers cannot be empty"));
    }

    #[test]
    fn load_fetch_header_rules_rejects_blank_static_header_values() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("blank-static-value.json");
        std::fs::write(
            &path,
            r#"[{
                "host": "api.example.com",
                "headers": {"Authorization": "   "}
            }]"#,
        )
        .expect("config should be written");

        let err = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect_err("blank static header value should fail");

        assert!(err.to_string().contains("'value' cannot be empty"));
    }

    #[test]
    fn load_fetch_header_rules_rejects_case_variant_duplicate_static_header_names() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("duplicate-static-headers.json");
        std::fs::write(
            &path,
            r#"[{
                "host": "api.example.com",
                "headers": {
                    "Authorization": "Bearer one",
                    "authorization": "Bearer two"
                }
            }]"#,
        )
        .expect("config should be written");

        let err = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect_err("case-variant duplicate headers should fail");

        assert!(err.to_string().contains("duplicate static header name"));
    }

    #[test]
    fn load_fetch_header_rules_rejects_exact_duplicate_static_header_names() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("exact-duplicate-static-headers.json");
        std::fs::write(
            &path,
            r#"[{
                "host": "api.example.com",
                "headers": {
                    "Authorization": "Bearer one",
                    "Authorization": "Bearer two"
                }
            }]"#,
        )
        .expect("config should be written");

        let err = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect_err("exact duplicate headers should fail");

        assert!(err.to_string().contains("duplicate field"));
        assert!(err.to_string().contains("Authorization"));
    }

    #[test]
    fn load_fetch_header_rules_rejects_blank_json_required_values() {
        let dir = tempfile::tempdir().expect("tempdir should be created");

        let empty_host = dir.path().join("empty-host.json");
        std::fs::write(
            &empty_host,
            r#"[{
                "host": "   ",
                "headers": {"Authorization": "Bearer fixed"}
            }]"#,
        )
        .expect("config should be written");
        let host_err = load_fetch_header_rules(&[], &Some(empty_host.display().to_string()))
            .expect_err("blank host should fail");
        assert!(host_err.to_string().contains("'host' cannot be empty"));

        let empty_token_url = dir.path().join("empty-token-url.json");
        std::fs::write(
            &empty_token_url,
            r#"[{
                "host": "api.example.com",
                "auth": {
                    "type": "oauth_client_credentials",
                    "header": "Authorization",
                    "token_url": "   ",
                    "client_id": "abc",
                    "client_secret": "xyz"
                }
            }]"#,
        )
        .expect("config should be written");
        let token_url_err = load_fetch_header_rules(&[], &Some(empty_token_url.display().to_string()))
            .expect_err("blank token_url should fail");
        assert!(token_url_err.to_string().contains("'token_url' cannot be empty"));
    }

    #[test]
    fn load_fetch_header_rules_trims_dynamic_json_required_values() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("trimmed-auth.json");
        std::fs::write(
            &path,
            r#"[{
                "host": " api.example.com ",
                "methods": [" get "],
                "auth": {
                    "type": "oauth_client_credentials",
                    "header": " Authorization ",
                    "token_url": " https://issuer/token ",
                    "client_id": " abc ",
                    "client_secret": " xyz "
                }
            }]"#,
        )
        .expect("config should be written");

        let rules = load_fetch_header_rules(&[], &Some(path.display().to_string()))
            .expect("trimmed auth values should parse");

        let auth = rules[0].dynamic_auth().expect("dynamic auth expected");
        assert_eq!(rules[0].methods(), &["GET".to_string()]);
        assert_eq!(auth.header_name, "Authorization");
        assert_eq!(auth.token_url, "https://issuer/token");
        assert_eq!(auth.client_id, "abc");
        assert_eq!(auth.client_secret, "xyz");
    }
}
