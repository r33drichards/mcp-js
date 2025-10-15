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
    #[arg(long)]
    http_port: Option<u16>,
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
    } else {
        tracing::info!("Starting stdio transport");
        let service = GenericService::new(heap_storage)
            .await
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
            .await
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
