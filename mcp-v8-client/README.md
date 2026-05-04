# mcp-v8-client

Auto-generated Rust API client and CLI for the [mcp-v8](https://github.com/r33drichards/mcp-js) JavaScript execution server.

The client is generated at build time from `openapi.json` using [progenitor](https://crates.io/crates/progenitor).

## Installation (CLI binary)

Download the pre-built binary from the [GitHub Releases](https://github.com/r33drichards/mcp-js/releases) page, or use the install script:

```bash
# Install both mcp-v8 server and mcp-v8-cli in one go
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | bash
```

Or install just the CLI:

```bash
MCP_V8_INSTALL=cli curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | bash
```

Or install via cargo (once published):

```bash
cargo install mcp-v8-client
```

## Library usage

Add to your `Cargo.toml`:

```toml
[dependencies]
mcp-v8-client = "0.1.0"
```

```rust
use mcp_v8_client::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new("http://localhost:3000");

    // Submit JS for async execution
    let body = mcp_v8_client::types::ExecRequest {
        code: "1 + 1".to_string(),
        heap: None,
        session: None,
        heap_memory_max_mb: None,
        execution_timeout_secs: None,
        tags: None,
    };
    let resp = client.exec_handler(&body).await?;
    println!("execution_id: {}", resp.into_inner().execution_id);
    Ok(())
}
```

## CLI usage

```
mcp-v8-cli [OPTIONS] <COMMAND>

Options:
  --url <URL>    Base URL of the mcp-v8 server [env: MCP_V8_URL] [default: http://localhost:3000]
  -j, --json     Output raw JSON

Commands:
  exec                  Submit JavaScript code for asynchronous execution
  executions list       List all known executions
  executions get <ID>   Get status and result of an execution
  executions output <ID> Read paginated console output
  executions cancel <ID> Cancel a running execution
```

### Examples

```bash
# Submit code
mcp-v8-cli exec "1 + 1"
# → execution_id: 550e8400-e29b-41d4-a716-446655440000

# Poll status
mcp-v8-cli executions get 550e8400-e29b-41d4-a716-446655440000

# Read output
mcp-v8-cli executions output 550e8400-e29b-41d4-a716-446655440000

# Cancel a running execution
mcp-v8-cli executions cancel 550e8400-e29b-41d4-a716-446655440000

# List all executions
mcp-v8-cli executions list

# Point at a remote server
mcp-v8-cli --url https://my-server.example.com executions list

# Or via env var
export MCP_V8_URL=https://my-server.example.com
mcp-v8-cli executions list

# JSON output (pipe-friendly)
mcp-v8-cli --json exec "42" | jq .execution_id
```

## How the client is generated

1. The server binary is built and run with `--print-openapi` to emit `openapi.json`.
2. That file is committed to the repo root and copied into `mcp-v8-client/openapi.json`.
3. `build.rs` calls [progenitor](https://github.com/oxidecomputer/progenitor) at compile time to generate a fully typed client into `OUT_DIR/codegen.rs`.
4. `src/lib.rs` `include!`s the generated file.
