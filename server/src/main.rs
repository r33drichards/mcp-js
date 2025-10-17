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
use mcp::{GenericService, initialize_v8};
use mcp::heap_storage::{AnyHeapStorage, S3HeapStorage, FileHeapStorage};

/// Command line arguments for configuring heap storage
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {

    /// S3 bucket name (required if --use-s3)
    #[arg(long, conflicts_with = "directory_path")]
    s3_bucket: Option<String>,

    /// Directory path for filesystem storage (required if --use-filesystem)
    #[arg(long, conflicts_with = "s3_bucket")]
    directory_path: Option<String>,

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

    let heap_storage = if let Some(bucket) = cli.s3_bucket {
        AnyHeapStorage::S3(S3HeapStorage::new(bucket).await)
    } else if let Some(dir) = cli.directory_path {
        AnyHeapStorage::File(FileHeapStorage::new(dir))
    } else {
        // default to file /tmp/mcp-v8-heaps
        AnyHeapStorage::File(FileHeapStorage::new("/tmp/mcp-v8-heaps"))
    };

    // Choose transport based on CLI arguments
    if let Some(port) = cli.http_port {
        tracing::info!("Starting HTTP transport on port {}", port);
        start_http_server(heap_storage, port).await?;
    } else if let Some(port) = cli.sse_port {
        tracing::info!("Starting SSE transport on port {}", port);
        start_sse_server(heap_storage, port).await?;
    } else {
        tracing::info!("Starting stdio transport");
        let service = GenericService::new(heap_storage)
            .serve(stdio())
            .await
            .inspect_err(|e| {
                tracing::error!("serving error: {:?}", e);
            })?;

        service.waiting().await?;
    }

    Ok(())
}

async fn http_handler(req: Request<Incoming>, heap_storage: AnyHeapStorage) -> Result<hyper::Response<String>, hyper::Error> {
    tokio::spawn(async move {
        let upgraded = hyper::upgrade::on(req).await?;
        let service = GenericService::new(heap_storage)
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

async fn start_http_server(heap_storage: AnyHeapStorage, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let tcp_listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("HTTP server listening on {}", addr);

    loop {
        let (stream, addr) = tcp_listener.accept().await?;
        tracing::info!("Accepted connection from: {}", addr);
        let heap_storage_clone = heap_storage.clone();

        let service = hyper::service::service_fn(move |req| {
            http_handler(req, heap_storage_clone.clone())
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

async fn start_sse_server(heap_storage: AnyHeapStorage, port: u16) -> Result<()> {
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
    tracing::info!("SSE server listening on {}", sse_server.config.bind);

    let ct = sse_server.config.ct.clone();
    let ct_shutdown = ct.child_token();

    // Start the HTTP server with graceful shutdown
    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        ct_shutdown.cancelled().await;
        tracing::info!("SSE server shutting down");
    });

    // Register the service BEFORE spawning the server task
    sse_server.with_service(move || {
        GenericService::new(heap_storage.clone())
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
