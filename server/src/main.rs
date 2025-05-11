use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{self};
use clap::Parser;

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

    let heap_storage = if cli.s3_bucket.is_some() {
        AnyHeapStorage::S3(S3HeapStorage::new(cli.s3_bucket.unwrap()).await)
    } else if cli.directory_path.is_some() {
        AnyHeapStorage::File(FileHeapStorage::new(cli.directory_path.unwrap()))
    } else {
        // default to s3
        AnyHeapStorage::S3(S3HeapStorage::new("test-mcp-js-bucket").await)
    };


    // Create an instance of our counter router
    let service = GenericService::new(heap_storage)
        .await
        .serve(stdio())
        .await
        .inspect_err(|e| {
            tracing::error!("serving error: {:?}", e);
        })?;

    service.waiting().await?;
    Ok(())
}
