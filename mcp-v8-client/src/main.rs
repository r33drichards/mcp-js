//! mcp-v8-cli — command-line interface for the mcp-v8 HTTP API
//!
//! Wraps the auto-generated `mcp_v8_client::Client` with human-friendly
//! subcommands and pretty-printed output.

use clap::{Parser, Subcommand};
use std::collections::HashMap;

/// CLI for the mcp-v8 JavaScript execution server HTTP API.
#[derive(Parser)]
#[command(
    name = "mcp-v8-cli",
    version,
    about = "Command-line client for the mcp-v8 HTTP API",
    long_about = None
)]
struct Cli {
    /// Base URL of the mcp-v8 server.
    #[arg(long, env = "MCP_V8_URL", default_value = "http://localhost:3000")]
    url: String,

    /// Output raw JSON instead of pretty-printed text.
    #[arg(long, short = 'j', global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Submit JavaScript code for asynchronous execution.
    Exec {
        /// JavaScript (or TypeScript) code to execute. Omit when using --file.
        #[arg(value_name = "CODE")]
        code: Option<String>,

        /// Read the code from a local file instead of the CODE argument.
        /// The file is read on this machine and its contents are submitted as
        /// the code (provide either CODE or --file, not both).
        #[arg(long, short = 'f', value_name = "PATH")]
        file: Option<String>,

        /// Heap snapshot key to restore before execution.
        #[arg(long)]
        heap: Option<String>,

        /// Filesystem snapshot to mount: a label name or 64-hex CA id.
        #[arg(long)]
        fs: Option<String>,

        /// Session identifier for tagging / logging.
        #[arg(long)]
        session: Option<String>,

        /// Per-execution V8 heap memory cap in megabytes.
        #[arg(long)]
        heap_memory_max_mb: Option<u64>,

        /// Per-execution timeout in seconds.
        #[arg(long)]
        execution_timeout_secs: Option<i64>,

        /// Key=value tags (repeatable). Example: --tag env=prod
        #[arg(long = "tag", value_name = "KEY=VALUE")]
        tags: Vec<String>,
    },
    /// Commands for inspecting and managing executions.
    #[command(subcommand)]
    Executions(ExecutionsCmd),
    /// Commands for content-addressed filesystem snapshots.
    #[command(subcommand)]
    Fs(FsCmd),
}

#[derive(Subcommand)]
enum FsCmd {
    /// List filesystem snapshot labels and their head CA ids.
    Ls,
    /// Resolve a label to its current head CA id.
    Pull {
        /// Label name.
        label: String,
    },
    /// Create or repoint a label to a CA id.
    Label {
        /// Label name.
        name: String,
        /// CA id (hex) to point at.
        ca_id: String,
        /// Optional note to record on the reflog entry (like a commit message).
        #[arg(long, short = 'm')]
        message: Option<String>,
    },
    /// Show a label's reflog (move history), oldest first.
    Log {
        /// Label name.
        label: String,
        /// Return only the most recent N entries (bounds the scan over long histories).
        #[arg(long)]
        limit: Option<u64>,
    },
    /// Advance a label to a CA id (reject-and-rebase by default).
    Push {
        /// CA id (hex) to point the label at (e.g. an execution's `fs` value).
        ca_id: String,
        /// Label to advance (omit only with --detach).
        #[arg(long)]
        label: Option<String>,
        /// The head you pulled; the push fails if the label moved since.
        #[arg(long)]
        expected: Option<String>,
        /// Override the conflict check and move the label unconditionally.
        #[arg(long)]
        force: bool,
        /// Do not touch any label; just echo the CA id back.
        #[arg(long)]
        detach: bool,
        /// Optional note to record on the reflog entry (like a commit message).
        #[arg(long, short = 'm')]
        message: Option<String>,
    },
    /// Reset a label to an earlier CA id from its reflog (rollback).
    Reset {
        /// Label name.
        label: String,
        /// CA id (hex) from the reflog to reset to.
        ca_id: String,
        /// Allow resetting to a CA id not present in the reflog.
        #[arg(long)]
        allow_unlogged: bool,
        /// Optional note to record on the reflog entry (like a commit message).
        #[arg(long, short = 'm')]
        message: Option<String>,
    },
    /// Three-way merge two snapshots into a new one.
    Merge {
        /// One side (CA id).
        ours: String,
        /// The other side (CA id).
        theirs: String,
        /// Common ancestor both sides diverged from (omit for a 2-way merge).
        #[arg(long)]
        base: Option<String>,
        /// Auto-resolve conflicts to a side: ours or theirs.
        #[arg(long)]
        prefer: Option<String>,
    },
}

#[derive(Subcommand)]
enum ExecutionsCmd {
    /// List all known executions.
    List,
    /// Get the status and result of an execution.
    Get {
        /// Execution ID.
        id: String,
    },
    /// Read paginated console output from an execution.
    Output {
        /// Execution ID.
        id: String,
        /// Start reading from this line number (0-indexed).
        #[arg(long)]
        line_offset: Option<i64>,
        /// Maximum number of lines to return.
        #[arg(long)]
        line_limit: Option<i64>,
        /// Start reading from this byte offset.
        #[arg(long)]
        byte_offset: Option<i64>,
        /// Maximum number of bytes to return.
        #[arg(long)]
        byte_limit: Option<i64>,
    },
    /// Cancel a running execution.
    Cancel {
        /// Execution ID.
        id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = mcp_v8_client::Client::new(&cli.url);

    match cli.command {
        Commands::Exec {
            code,
            file,
            heap,
            fs,
            session,
            heap_memory_max_mb,
            execution_timeout_secs,
            tags,
        } => {
            // Resolve the code from either the positional CODE or --file.
            let code = match (code, file) {
                (Some(_), Some(_)) => {
                    anyhow::bail!("provide either the CODE argument or --file, not both");
                }
                (None, None) => {
                    anyhow::bail!("no code provided: pass a CODE argument or --file <PATH>");
                }
                (Some(code), None) => code,
                (None, Some(path)) => std::fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("failed to read file '{}': {}", path, e))?,
            };

            let tag_map: Option<HashMap<String, String>> = if tags.is_empty() {
                None
            } else {
                let mut m = HashMap::new();
                for kv in &tags {
                    let (k, v) = kv.split_once('=').unwrap_or((kv.as_str(), ""));
                    m.insert(k.to_string(), v.to_string());
                }
                Some(m)
            };

            let body = mcp_v8_client::types::ExecRequest {
                code,
                heap,
                fs,
                session,
                heap_memory_max_mb,
                execution_timeout_secs,
                tags: tag_map,
            };

            let result = client.exec_handler(&body).await
                .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
            } else {
                let inner = result.into_inner();
                println!("✅ Execution queued");
                println!("   execution_id: {}", inner.execution_id);
                println!();
                println!("Poll status:  mcp-v8-cli --url {} executions get {}", cli.url, inner.execution_id);
                println!("Read output:  mcp-v8-cli --url {} executions output {}", cli.url, inner.execution_id);
            }
        }

        Commands::Executions(exec_cmd) => match exec_cmd {
            ExecutionsCmd::List => {
                let result = client.list_executions_handler().await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
                } else {
                    let inner = result.into_inner();
                    let execs = inner.executions;
                    if execs.is_empty() {
                        println!("No executions found.");
                    } else {
                        println!("{:<38} {:<12} {:<26} {}", "EXECUTION ID", "STATUS", "STARTED AT", "COMPLETED AT");
                        println!("{}", "-".repeat(100));
                        for ex in &execs {
                            println!(
                                "{:<38} {:<12} {:<26} {}",
                                ex.get("execution_id").and_then(|v| v.as_str()).unwrap_or("-"),
                                ex.get("status").and_then(|v| v.as_str()).unwrap_or("-"),
                                ex.get("started_at").and_then(|v| v.as_str()).unwrap_or("-"),
                                ex.get("completed_at").and_then(|v| v.as_str()).unwrap_or("-"),
                            );
                        }
                    }
                }
            }

            ExecutionsCmd::Get { id } => {
                let result = client.get_execution_handler(&id).await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
                } else {
                    let ex = result.into_inner();
                    println!("execution_id : {}", ex.execution_id);
                    println!("status       : {}", ex.status);
                    println!("started_at   : {}", ex.started_at);
                    if let Some(ref c) = ex.completed_at {
                        println!("completed_at : {}", c);
                    }
                    if let Some(ref r) = ex.result {
                        println!("result       : {}", r);
                    }
                    if let Some(ref h) = ex.heap {
                        println!("heap         : {}", h);
                    }
                    if let Some(ref f) = ex.fs {
                        println!("fs           : {}", f);
                    }
                    if let Some(ref e) = ex.error {
                        println!("error        : {}", e);
                    }
                }
            }

            ExecutionsCmd::Output {
                id,
                line_offset,
                line_limit,
                byte_offset,
                byte_limit,
            } => {
                let result = client
                    .get_execution_output_handler(
                        &id,
                        line_offset,
                        line_limit,
                        byte_offset,
                        byte_limit,
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
                } else {
                    let page = result.into_inner();
                    print!("{}", page.data);
                    if page.has_more {
                        eprintln!(
                            "\n[more output available — next_line_offset={} next_byte_offset={}]",
                            page.next_line_offset, page.next_byte_offset
                        );
                    }
                }
            }

            ExecutionsCmd::Cancel { id } => {
                let result = client.cancel_execution_handler(&id).await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
                } else {
                    let inner = result.into_inner();
                    if inner.ok {
                        println!("✅ Execution {} cancelled.", id);
                    } else {
                        let msg = inner.error.unwrap_or_else(|| "unknown error".to_string());
                        println!("❌ Could not cancel {}: {}", id, msg);
                    }
                }
            }
        },

        Commands::Fs(fs_cmd) => match fs_cmd {
            FsCmd::Ls => {
                let result = client.fs_labels_handler().await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
                println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
            }
            FsCmd::Pull { label } => {
                let result = client.fs_resolve_handler(&label).await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
                println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
            }
            FsCmd::Label { name, ca_id, message } => {
                let body = mcp_v8_client::types::FsLabelRequest { name, ca_id, message };
                let result = client.fs_set_label_handler(&body).await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
                println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
            }
            FsCmd::Log { label, limit } => {
                let result = client.fs_log_handler(&label, limit).await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
                println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
            }
            FsCmd::Push { ca_id, label, expected, force, detach, message } => {
                let body = mcp_v8_client::types::FsPushRequest {
                    ca_id,
                    label,
                    expected,
                    force: Some(force),
                    detach: Some(detach),
                    message,
                };
                let result = client.fs_push_handler(&body).await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
                println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
            }
            FsCmd::Reset { label, ca_id, allow_unlogged, message } => {
                let body = mcp_v8_client::types::FsResetRequest {
                    label,
                    ca_id,
                    allow_unlogged: Some(allow_unlogged),
                    message,
                };
                let result = client.fs_reset_handler(&body).await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
                println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
            }
            FsCmd::Merge { ours, theirs, base, prefer } => {
                let body = mcp_v8_client::types::FsMergeRequest {
                    ours,
                    theirs,
                    base,
                    prefer,
                };
                let result = client.fs_merge_handler(&body).await
                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
                println!("{}", serde_json::to_string_pretty(&result.into_inner())?);
            }
        },
    }

    Ok(())
}
