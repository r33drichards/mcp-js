# Use Rust Client

The Rust client is a typed client for the HTTP API. Use it when you want a
programmatic fallback or automation path outside MCP.

Add the dependency:

```toml
[dependencies]
mcp-v8-client = "0.1.0"
tokio = { version = "1", features = ["full"] }
```

Submit code through the generated client:

```rust
use mcp_v8_client::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new("http://localhost:3000");
    let body = mcp_v8_client::types::ExecRequest {
        code: "1 + 1".to_string(),
        heap: None,
        session: None,
        heap_memory_max_mb: None,
        execution_timeout_secs: None,
        tags: None,
    };

    let resp = client.exec_handler(&body).await?;
    println!("{}", resp.into_inner().execution_id);
    Ok(())
}
```

This client targets the HTTP API, not the MCP transport. For the primary
integration model, use `mcp-v8` as an MCP server instead.
