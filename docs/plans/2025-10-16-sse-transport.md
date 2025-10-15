# SSE Transport Implementation Plan

> **For Claude:** Use `${SUPERPOWERS_SKILLS_ROOT}/skills/collaboration/executing-plans/SKILL.md` to implement this plan task-by-task.

**Goal:** Add SSE (Server-Sent Events) transport support to the MCP server using the minimal `SseServer::serve()` approach from rust-sdk examples.

**Architecture:** Add `--sse-port` CLI flag (mutually exclusive with `--http-port`). When provided, use `SseServer::serve()` to bind SSE transport instead of stdio or HTTP. Reuse existing `GenericService` with heap storage.

**Tech Stack:** Rust, rmcp crate (SSE server features), tokio, axum (already dependencies)

---

## Task 1: Update Dependencies

**Files:**
- Modify: `server/Cargo.toml:9`

**Step 1: Check current rmcp dependency features**

Current line 9 in Cargo.toml:
```toml
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main", features = ["transport-io"] }
```

**Step 2: Add SSE server feature to rmcp dependency**

Update line 9 to include SSE transport features:
```toml
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main", features = ["transport-io", "transport-sse-server"] }
```

**Step 3: Verify dependency resolves**

Run: `cargo check`
Expected: Success (or compilation errors about missing code, not dependency errors)

**Step 4: Commit**

```bash
git add server/Cargo.toml
git commit -m "feat: add rmcp SSE server transport feature"
```

---

## Task 2: Add SSE Port CLI Argument

**Files:**
- Modify: `server/src/main.rs:16-32`

**Step 1: Add sse_port field to Cli struct**

In `server/src/main.rs`, update the `Cli` struct (lines 16-32) to add the SSE port option:

```rust
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
```

**Step 2: Verify compilation**

Run: `cargo check`
Expected: Success (CLI struct compiles, conflicts work)

**Step 3: Test mutual exclusivity**

Run: `cargo run -- --http-port 8080 --sse-port 8081`
Expected: Error message about conflicting arguments

**Step 4: Commit**

```bash
git add server/src/main.rs
git commit -m "feat: add --sse-port CLI argument"
```

---

## Task 3: Add SSE Transport Handler Function

**Files:**
- Modify: `server/src/main.rs` (add new function after `start_http_server`)

**Step 1: Add import for SseServer**

Add to the imports at the top of `server/src/main.rs` (around line 2):

```rust
use rmcp::{ServiceExt, transport::{stdio, SseServer}};
```

**Step 2: Implement start_sse_server function**

Add this function after `start_http_server` (after line 120):

```rust
async fn start_sse_server(heap_storage: AnyHeapStorage, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port).parse()?;
    tracing::info!("Starting SSE transport on {}", addr);

    let ct = SseServer::serve(addr)
        .await?
        .with_service_directly(|| async move {
            GenericService::new(heap_storage.clone()).await
        });

    tokio::signal::ctrl_c().await?;
    ct.cancel();
    Ok(())
}
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Success

**Step 4: Commit**

```bash
git add server/src/main.rs
git commit -m "feat: add start_sse_server function"
```

---

## Task 4: Wire SSE Handler into Main Function

**Files:**
- Modify: `server/src/main.rs:58-76` (main function transport selection)

**Step 1: Update main function to handle sse_port**

Update the transport selection logic in `main()` (lines 58-76):

```rust
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
            .await
            .serve(stdio())
            .await
            .inspect_err(|e| {
                tracing::error!("serving error: {:?}", e);
            })?;

        service.waiting().await?;
    }
```

**Step 2: Verify compilation**

Run: `cargo build`
Expected: Success (full build)

**Step 3: Commit**

```bash
git add server/src/main.rs
git commit -m "feat: wire SSE transport into main function"
```

---

## Task 5: Manual Testing

**Files:**
- None (testing only)

**Step 1: Start SSE server with local storage**

Run: `cargo run -- --sse-port 8080 --directory-path /tmp/test-heaps`
Expected: Server starts, logs "Starting SSE transport on 0.0.0.0:8080"

**Step 2: Test with MCP Inspector (if available)**

In another terminal, run:
```bash
npx @modelcontextprotocol/inspector sse http://localhost:8080
```
Expected: Inspector connects, shows available tools, can execute JavaScript

**Step 3: Test with curl (basic connectivity)**

In another terminal:
```bash
curl http://localhost:8080/sse
```
Expected: SSE connection established (keeps connection open, may see keep-alive messages)

**Step 4: Test graceful shutdown**

In the server terminal, press Ctrl+C
Expected: Clean shutdown with "sse server cancelled" log

**Step 5: Document test results**

Create `docs/sse-test-results.md` with:
- What worked
- What didn't work
- Any error messages
- Next steps if issues found

No commit needed for this step (just verification).

---

## Task 6: Update README Documentation

**Files:**
- Modify: `README.md`

**Step 1: Add SSE transport section**

After the "HTTP Transport" section (around line 60), add:

```markdown
### SSE Transport

The SSE (Server-Sent Events) transport provides a unidirectional streaming connection over HTTP, useful for browser-based clients and the MCP Inspector:

```bash
# Start SSE server on port 8080 with local filesystem storage
mcp-v8 --directory-path /tmp/mcp-v8-heaps --sse-port 8080

# Start SSE server on port 8080 with S3 storage
mcp-v8 --s3-bucket my-bucket-name --sse-port 8080
```

The SSE transport is useful for:
- Browser-based MCP clients
- Testing with the MCP Inspector
- Environments that prefer SSE over WebSocket upgrades
- Simplified one-way streaming from server to client

**Note:** SSE and HTTP transports are mutually exclusive. Only one can be used at a time.
```

**Step 2: Update transport selection note**

Update the "Command Line Arguments" section (around line 32) to mention SSE:

```markdown
- `--sse-port <port>`: Enable SSE transport on the specified port. (Conflicts with `--http-port`)
```

**Step 3: Verify markdown formatting**

Run: `cat README.md` (or use a markdown preview)
Expected: Properly formatted markdown

**Step 4: Commit**

```bash
git add README.md
git commit -m "docs: add SSE transport documentation to README"
```

---

## Task 7: Final Verification

**Files:**
- None (verification only)

**Step 1: Run full build**

Run: `cargo build --release`
Expected: Clean build, no warnings

**Step 2: Test all three transport modes**

Test stdio:
```bash
echo '{"jsonrpc":"2.0","method":"initialize","id":1}' | cargo run --quiet
```
Expected: JSON response

Test HTTP (if not already tested):
```bash
cargo run -- --http-port 8081 --directory-path /tmp/test-heaps
# In another terminal: test with curl or inspector
```
Expected: Works

Test SSE:
```bash
cargo run -- --sse-port 8082 --directory-path /tmp/test-heaps
# In another terminal: test with curl or inspector
```
Expected: Works

**Step 3: Verify mutual exclusivity**

Run: `cargo run -- --http-port 8080 --sse-port 8080`
Expected: Clear error about conflicting options

**Step 4: Review all commits**

Run: `git log --oneline`
Expected: Clean, well-structured commit history with descriptive messages

No commit needed - verification complete!

---

## Notes

- **DRY:** Reused existing `GenericService` and heap storage infrastructure
- **YAGNI:** Minimal SSE implementation - no auth, no custom paths (can add later if needed)
- **TDD:** Manual testing approach (no automated tests for transport layer in original codebase)
- **Dependencies:** Only added SSE feature to existing rmcp dependency

## Reference

Based on `/tmp/rust-sdk/examples/servers/src/counter_sse_directly.rs` - the minimal SSE example using `SseServer::serve()`.
