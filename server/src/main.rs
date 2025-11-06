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
use mcp::{StatelessService, StatefulService, initialize_v8};
use mcp::heap_storage::{AnyHeapStorage, S3HeapStorage, FileHeapStorage, InMemoryHeapStorage};

/// Command line arguments for configuring heap storage
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {

    /// S3 bucket name (required if --use-s3)
    #[arg(long, conflicts_with_all = ["directory_path", "stateless", "in_memory"])]
    s3_bucket: Option<String>,

    /// Directory path for filesystem storage (required if --use-filesystem)
    #[arg(long, conflicts_with_all = ["s3_bucket", "stateless", "in_memory"])]
    directory_path: Option<String>,

    /// Use in-memory heap storage (perfect for debugging - heaps stored in RAM)
    #[arg(long, conflicts_with_all = ["s3_bucket", "directory_path", "stateless"])]
    in_memory: bool,

    /// Run in stateless mode - no heap snapshots are saved or loaded
    #[arg(long, conflicts_with_all = ["s3_bucket", "directory_path", "in_memory"])]
    stateless: bool,

    /// HTTP port to listen on (if not specified, uses stdio transport)
    #[arg(long, conflicts_with = "sse_port")]
    http_port: Option<u16>,

    /// SSE port to listen on (if not specified, uses stdio transport)
    #[arg(long, conflicts_with = "http_port")]
    sse_port: Option<u16>,
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

    if cli.stateless {
        // Stateless mode - no heap persistence
        if let Some(port) = cli.http_port {
            tracing::info!("Starting HTTP transport in stateless mode on port {}", port);
            start_http_server_stateless(port).await?;
        } else if let Some(port) = cli.sse_port {
            tracing::info!("Starting SSE transport in stateless mode on port {}", port);
            start_sse_server_stateless(port).await?;
        } else {
            tracing::info!("Starting stdio transport in stateless mode");
            let service = StatelessService::new()
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
            tracing::info!("Using S3 heap storage with bucket: {}", bucket);
            AnyHeapStorage::S3(S3HeapStorage::new(bucket).await)
        } else if let Some(dir) = cli.directory_path {
            tracing::info!("Using file-based heap storage at: {}", dir);
            AnyHeapStorage::File(FileHeapStorage::new(dir))
        } else if cli.in_memory {
            tracing::info!("Using in-memory heap storage (all heaps stored in RAM)");
            AnyHeapStorage::InMemory(InMemoryHeapStorage::new())
        } else {
            // default to file /tmp/mcp-v8-heaps
            tracing::info!("Using default file-based heap storage at: /tmp/mcp-v8-heaps");
            AnyHeapStorage::File(FileHeapStorage::new("/tmp/mcp-v8-heaps"))
        };

        if let Some(port) = cli.http_port {
            tracing::info!("Starting HTTP transport in stateful mode on port {}", port);
            start_http_server_stateful(heap_storage, port).await?;
        } else if let Some(port) = cli.sse_port {
            tracing::info!("Starting SSE transport in stateful mode on port {}", port);
            start_sse_server_stateful(heap_storage, port).await?;
        } else {
            tracing::info!("Starting stdio transport in stateful mode");
            let service = StatefulService::new(heap_storage)
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
async fn http_handler_stateless(req: Request<Incoming>) -> Result<hyper::Response<String>, hyper::Error> {
    tokio::spawn(async move {
        let upgraded = hyper::upgrade::on(req).await?;
        let service = StatelessService::new()
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

async fn start_http_server_stateless(port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let tcp_listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("HTTP server (stateless) listening on {}", addr);

    loop {
        let (stream, addr) = tcp_listener.accept().await?;
        tracing::info!("Accepted connection from: {}", addr);

        let service = hyper::service::service_fn(move |req| {
            http_handler_stateless(req)
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
async fn http_handler_stateful(req: Request<Incoming>, heap_storage: AnyHeapStorage) -> Result<hyper::Response<String>, hyper::Error> {
    tokio::spawn(async move {
        let upgraded = hyper::upgrade::on(req).await?;
        let service = StatefulService::new(heap_storage)
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

async fn start_http_server_stateful(heap_storage: AnyHeapStorage, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let tcp_listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("HTTP server (stateful) listening on {}", addr);

    loop {
        let (stream, addr) = tcp_listener.accept().await?;
        tracing::info!("Accepted connection from: {}", addr);
        let heap_storage_clone = heap_storage.clone();

        let service = hyper::service::service_fn(move |req| {
            http_handler_stateful(req, heap_storage_clone.clone())
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
async fn start_sse_server_stateless(port: u16) -> Result<()> {
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
        StatelessService::new()
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
async fn start_sse_server_stateful(heap_storage: AnyHeapStorage, port: u16) -> Result<()> {
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
        StatefulService::new(heap_storage.clone())
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
