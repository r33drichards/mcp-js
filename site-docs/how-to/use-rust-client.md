# Use Rust Client

Add the generated Rust client crate:

```toml
[dependencies]
mcp-v8-client = "0.1.0"
```

Basic usage:

```rust
use mcp_v8_client::Client;

let client = Client::new("http://localhost:3000");
```

Use the Rust client when you want typed access to the HTTP API from Rust code.
