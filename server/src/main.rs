use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{self};
use clap::Parser;
use hyper::{
    Request, StatusCode,
    body::Incoming,
    header::{HeaderValue, UPGRADE},
};
use hyper_util::rt::TokioIo;
use rmcp::transport::sse_server::{SseServer, SseServerConfig};
use tokio_util::sync::CancellationToken;

mod mcp;
mod cluster;
use mcp::{StatelessService, StatefulService, initialize_v8, DEFAULT_EXECUTION_TIMEOUT_SECS};
use mcp::heap_storage::{AnyHeapStorage, S3HeapStorage, WriteThroughCacheHeapStorage, FileHeapStorage};
use mcp::session_log::SessionLog;
use cluster::{ClusterConfig, ClusterNode};

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

    /// HTTP port to listen on (if not specified, uses stdio transport)
    #[arg(long, conflicts_with = "sse_port")]
    http_port: Option<u16>,

    /// SSE port to listen on (if not specified, uses stdio transport)
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

    /// Comma-separated list of peer addresses (host:port)
    #[arg(long, value_delimiter = ',')]
    peers: Vec<String>,

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

/// npx @modelcontextprotocol/inspector cargo run -p mcp-server-examples --example std_io
#[tokio::main]
async fn main() -> Result<()> {
    initialize_v8();
    // Initialize the tracing subscriber with file and stdout logging
    tracing_subscriber::fmt()
        // .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();

    tracing::info!(?cli, "Starting MCP server with CLI arguments");

    let heap_memory_max_bytes = (cli.heap_memory_max as usize) * 1024 * 1024;
    let execution_timeout_secs = cli.execution_timeout;
    tracing::info!("V8 heap memory limit: {} MB ({} bytes)", cli.heap_memory_max, heap_memory_max_bytes);
    tracing::info!("V8 execution timeout: {} seconds", execution_timeout_secs);

    // Cluster mode requires --http-port or --sse-port (stdio has no stdin as a service).
    if cli.cluster_port.is_some() && cli.http_port.is_none() && cli.sse_port.is_none() {
        anyhow::bail!(
            "Cluster mode requires --http-port or --sse-port (stdio transport is not supported in cluster mode)"
        );
    }

    // Start cluster node if cluster_port is specified
    if let Some(cluster_port) = cli.cluster_port {
        let cluster_config = ClusterConfig {
            node_id: cli.node_id.clone(),
            peers: cli.peers.clone(),
            cluster_port,
            heartbeat_interval: std::time::Duration::from_millis(cli.heartbeat_interval),
            election_timeout_min: std::time::Duration::from_millis(cli.election_timeout_min),
            election_timeout_max: std::time::Duration::from_millis(cli.election_timeout_max),
        };

        let cluster_db_path = format!("{}/cluster-{}", cli.session_db_path, cli.node_id);
        let cluster_db = sled::open(&cluster_db_path)
            .expect("Failed to open cluster sled database");

        let cluster_node = ClusterNode::new(cluster_config, cluster_db);
        cluster_node.start().await;
        tracing::info!("Cluster node {} started on port {}", cli.node_id, cluster_port);
    }

    if cli.stateless {
        // Stateless mode - no heap persistence
        if let Some(port) = cli.http_port {
            tracing::info!("Starting HTTP transport in stateless mode on port {}", port);
            start_http_server_stateless(port, heap_memory_max_bytes, execution_timeout_secs).await?;
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
            // default to file /tmp/mcp-v8-heaps
            AnyHeapStorage::File(FileHeapStorage::new("/tmp/mcp-v8-heaps"))
        };

        let session_log = match SessionLog::new(&cli.session_db_path) {
            Ok(log) => {
                tracing::info!("Session log opened at {}", cli.session_db_path);
                Some(log)
            }
            Err(e) => {
                tracing::warn!("Failed to open session log at {}: {}. Session logging disabled.", cli.session_db_path, e);
                None
            }
        };

        if let Some(port) = cli.http_port {
            tracing::info!("Starting HTTP transport in stateful mode on port {}", port);
            start_http_server_stateful(heap_storage, session_log, port, heap_memory_max_bytes, execution_timeout_secs).await?;
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

// Stateless HTTP handlers
async fn http_handler_stateless(req: Request<Incoming>, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Result<hyper::Response<String>, hyper::Error> {
    tokio::spawn(async move {
        let upgraded = hyper::upgrade::on(req).await?;
        let service = StatelessService::new(heap_memory_max_bytes, execution_timeout_secs)
            .serve(TokioIo::new(upgraded))
            .await?;
        service.waiting().await?;
        anyhow::Result::<()>::Ok(())
    });
    let mut response = hyper::Response::new(String::new());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    response
        .headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("mcp"));
    Ok(response)
}

async fn start_http_server_stateless(port: u16, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let tcp_listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("HTTP server (stateless) listening on {}", addr);

    loop {
        let (stream, addr) = tcp_listener.accept().await?;
        tracing::info!("Accepted connection from: {}", addr);

        let service = hyper::service::service_fn(move |req| {
            http_handler_stateless(req, heap_memory_max_bytes, execution_timeout_secs)
        });

        let conn = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .with_upgrades();

        tokio::spawn(async move {
            if let Err(err) = conn.await {
                tracing::error!("Connection error: {:?}", err);
            }
        });
    }
}

// Stateful HTTP handlers
async fn http_handler_stateful(req: Request<Incoming>, heap_storage: AnyHeapStorage, session_log: Option<SessionLog>, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Result<hyper::Response<String>, hyper::Error> {
    tokio::spawn(async move {
        let upgraded = hyper::upgrade::on(req).await?;
        let service = StatefulService::new(heap_storage, session_log, heap_memory_max_bytes, execution_timeout_secs)
            .serve(TokioIo::new(upgraded))
            .await?;
        service.waiting().await?;
        anyhow::Result::<()>::Ok(())
    });
    let mut response = hyper::Response::new(String::new());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    response
        .headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("mcp"));
    Ok(response)
}

async fn start_http_server_stateful(heap_storage: AnyHeapStorage, session_log: Option<SessionLog>, port: u16, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let tcp_listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("HTTP server (stateful) listening on {}", addr);

    loop {
        let (stream, addr) = tcp_listener.accept().await?;
        tracing::info!("Accepted connection from: {}", addr);
        let heap_storage_clone = heap_storage.clone();
        let session_log_clone = session_log.clone();

        let service = hyper::service::service_fn(move |req| {
            http_handler_stateful(req, heap_storage_clone.clone(), session_log_clone.clone(), heap_memory_max_bytes, execution_timeout_secs)
        });

        let conn = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .with_upgrades();

        tokio::spawn(async move {
            if let Err(err) = conn.await {
                tracing::error!("Connection error: {:?}", err);
            }
        });
    }
}

// Stateless SSE server
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

    // Start the HTTP server with graceful shutdown
    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("SSE server shutting down");
    });

    // Register the service BEFORE spawning the server task
    sse_server.with_service(move || {
        StatelessService::new(heap_memory_max_bytes, execution_timeout_secs)
    });

    // Spawn the server task AFTER registering the service
    tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!("SSE server error: {:?}", e);
        }
    });

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    tracing::info!("Received Ctrl+C, shutting down SSE server");
    ct.cancel();

    Ok(())
}

// Stateful SSE server
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

    // Start the HTTP server with graceful shutdown
    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("SSE server shutting down");
    });

    // Register the service BEFORE spawning the server task
    sse_server.with_service(move || {
        StatefulService::new(heap_storage.clone(), session_log.clone(), heap_memory_max_bytes, execution_timeout_secs)
    });

    // Spawn the server task AFTER registering the service
    tokio::spawn(async move {
        if let Err(e) = server.await {
            tracing::error!("SSE server error: {:?}", e);
        }
    });

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    tracing::info!("Received Ctrl+C, shutting down SSE server");
    ct.cancel();

    Ok(())
}
