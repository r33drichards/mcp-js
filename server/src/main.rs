use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{self};
use clap::Parser;
use rmcp::transport::sse_server::{SseServer, SseServerConfig};
use rmcp::transport::StreamableHttpServer;
use rmcp::transport::streamable_http_server::axum::StreamableHttpServerConfig;
use tokio_util::sync::CancellationToken;
use std::sync::Arc;
use axum::{Router, Json, extract::State, http::StatusCode, routing::post};

mod mcp;
mod cluster;
use mcp::{StatelessService, StatefulService, initialize_v8, execute_stateless, DEFAULT_EXECUTION_TIMEOUT_SECS};
use mcp::heap_storage::{AnyHeapStorage, S3HeapStorage, WriteThroughCacheHeapStorage, FileHeapStorage};
use mcp::session_log::SessionLog;
use cluster::{ClusterConfig, ClusterNode};

// ── Plain HTTP /api/exec endpoint ───────────────────────────────────────

#[derive(serde::Deserialize)]
struct ExecRequest {
    code: String,
}

#[derive(serde::Serialize)]
struct ExecResponse {
    output: String,
}

#[derive(Clone)]
struct ExecState {
    heap_memory_max_bytes: usize,
    execution_timeout_secs: u64,
}

async fn exec_handler(
    State(state): State<ExecState>,
    Json(req): Json<ExecRequest>,
) -> (StatusCode, Json<ExecResponse>) {
    let max_bytes = state.heap_memory_max_bytes;
    let timeout = state.execution_timeout_secs;

    match tokio::task::spawn_blocking(move || execute_stateless(req.code, max_bytes, timeout)).await {
        Ok(Ok(output)) => (StatusCode::OK, Json(ExecResponse { output })),
        Ok(Err(e)) => (StatusCode::OK, Json(ExecResponse { output: format!("V8 error: {}", e) })),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ExecResponse { output: format!("Task error: {}", e) })),
    }
}

fn exec_router(heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Router {
    Router::new()
        .route("/api/exec", post(exec_handler))
        .with_state(ExecState { heap_memory_max_bytes, execution_timeout_secs })
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

    /// Path to the sled database for session logging (default: /tmp/mcp-v8-sessions)
    #[arg(long, default_value = "/tmp/mcp-v8-sessions", conflicts_with = "stateless")]
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

    if cli.stateless {
        // Stateless mode - no heap persistence
        if let Some(port) = cli.http_port {
            tracing::info!("Starting Streamable HTTP transport in stateless mode on port {}", port);
            start_streamable_http_stateless(port, heap_memory_max_bytes, execution_timeout_secs).await?;
        } else if let Some(port) = cli.sse_port {
            tracing::info!("Starting SSE transport in stateless mode on port {}", port);
            start_sse_server_stateless(port, heap_memory_max_bytes, execution_timeout_secs).await?;
        } else {
            tracing::info!("Starting stdio transport in stateless mode");
            let service = StatelessService::new(heap_memory_max_bytes, execution_timeout_secs)
                .serve(stdio())
                .await
                .inspect_err(|e| {
                    tracing::error!("serving error: {:?}", e);
                })?;
            service.waiting().await?;
        }
    } else {
        // Stateful mode - with heap persistence
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

        if let Some(port) = cli.http_port {
            tracing::info!("Starting Streamable HTTP transport in stateful mode on port {}", port);
            start_streamable_http_stateful(heap_storage, session_log, port, heap_memory_max_bytes, execution_timeout_secs).await?;
        } else if let Some(port) = cli.sse_port {
            tracing::info!("Starting SSE transport in stateful mode on port {}", port);
            start_sse_server_stateful(heap_storage, session_log, port, heap_memory_max_bytes, execution_timeout_secs).await?;
        } else {
            tracing::info!("Starting stdio transport in stateful mode");
            let service = StatefulService::new(heap_storage, session_log, heap_memory_max_bytes, execution_timeout_secs)
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

async fn start_streamable_http_stateless(port: u16, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Result<()> {
    let bind: std::net::SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let ct = CancellationToken::new();

    let config = StreamableHttpServerConfig {
        bind,
        path: "/mcp".to_string(),
        ct: ct.clone(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (server, mcp_router) = StreamableHttpServer::new(config);
    let merged = mcp_router.merge(exec_router(heap_memory_max_bytes, execution_timeout_secs));

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("Streamable HTTP server (stateless) listening on {}", bind);

    let ct_shutdown = ct.child_token();
    let axum_server = axum::serve(listener, merged).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("Streamable HTTP server shutting down");
    });
    tokio::spawn(async move {
        if let Err(e) = axum_server.await {
            tracing::error!("Streamable HTTP server error: {:?}", e);
        }
    });

    server.with_service(move || {
        StatelessService::new(heap_memory_max_bytes, execution_timeout_secs)
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("Received Ctrl+C, shutting down");
    ct.cancel();

    Ok(())
}

async fn start_streamable_http_stateful(heap_storage: AnyHeapStorage, session_log: Option<SessionLog>, port: u16, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Result<()> {
    let bind: std::net::SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let ct = CancellationToken::new();

    let config = StreamableHttpServerConfig {
        bind,
        path: "/mcp".to_string(),
        ct: ct.clone(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (server, mcp_router) = StreamableHttpServer::new(config);
    let merged = mcp_router.merge(exec_router(heap_memory_max_bytes, execution_timeout_secs));

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("Streamable HTTP server (stateful) listening on {}", bind);

    let ct_shutdown = ct.child_token();
    let axum_server = axum::serve(listener, merged).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("Streamable HTTP server shutting down");
    });
    tokio::spawn(async move {
        if let Err(e) = axum_server.await {
            tracing::error!("Streamable HTTP server error: {:?}", e);
        }
    });

    server.with_service(move || {
        StatefulService::new(heap_storage.clone(), session_log.clone(), heap_memory_max_bytes, execution_timeout_secs)
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("Received Ctrl+C, shutting down");
    ct.cancel();

    Ok(())
}

// ── SSE transport (--sse-port) ──────────────────────────────────────────

async fn start_sse_server_stateless(port: u16, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port).parse()?;

    let config = SseServerConfig {
        bind: addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (sse_server, router) = SseServer::new(config);

    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    tracing::info!("SSE server (stateless) listening on {}", sse_server.config.bind);

    let ct = sse_server.config.ct.clone();
    let ct_shutdown = ct.child_token();

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("SSE server shutting down");
    });

    sse_server.with_service(move || {
        StatelessService::new(heap_memory_max_bytes, execution_timeout_secs)
    });

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

async fn start_sse_server_stateful(heap_storage: AnyHeapStorage, session_log: Option<SessionLog>, port: u16, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port).parse()?;

    let config = SseServerConfig {
        bind: addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: Some(std::time::Duration::from_secs(15)),
    };

    let (sse_server, router) = SseServer::new(config);

    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    tracing::info!("SSE server (stateful) listening on {}", sse_server.config.bind);

    let ct = sse_server.config.ct.clone();
    let ct_shutdown = ct.child_token();

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("SSE server shutting down");
    });

    sse_server.with_service(move || {
        StatefulService::new(heap_storage.clone(), session_log.clone(), heap_memory_max_bytes, execution_timeout_secs)
    });

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
