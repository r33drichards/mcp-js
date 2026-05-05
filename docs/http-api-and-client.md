# HTTP API, CLI & Rust SDK

The mcp-v8 server exposes a plain REST API alongside its MCP transport. This
document covers:

1. [API reference](#1-api-reference)
2. [Installing and using the CLI (`mcp-v8-cli`)](#2-cli-mcp-v8-cli)
3. [Using the Rust SDK (`mcp-v8-client`)](#3-rust-sdk-mcp-v8-client)
4. [Generating a client for another language](#4-generating-a-client-for-another-language)
5. [Keeping `openapi.json` up to date](#5-keeping-openapijson-up-to-date)

---

## Prerequisites

Start the server in HTTP mode. The simplest form — no persistent heap storage:

```bash
cargo build --release -p server
./target/release/server --stateless --http-port 3000
```

All examples below assume the server is reachable at `http://localhost:3000`.
Set the env var once to avoid repeating `--url`:

```bash
export MCP_V8_URL=http://localhost:3000
```

---

## 1. API reference

| Endpoint | Method | Description |
|---|---|---|
| `/api/version` | GET | Server version |
| `/api/exec` | POST | Submit JS code for async execution |
| `/api/executions` | GET | List all executions |
| `/api/executions/{id}` | GET | Get status + result of an execution |
| `/api/executions/{id}/output` | GET | Read paginated console output |
| `/api/executions/{id}/cancel` | POST | Cancel a running execution |
| `/api/cli` | GET | CLI download index (version + per-platform URLs) |
| `/api/cli/{platform}` | GET | Download the CLI binary for a platform |
| `/api-doc/openapi.json` | GET | OpenAPI 3.0 specification |

### GET /api/version

```bash
$ curl http://localhost:3000/api/version
{"version":"0.1.0"}
```

### GET /api/cli

Returns the server version and direct download URLs for each platform. URLs
point to this server — the binary served is the exact CLI that shipped with
this server version.

```bash
$ curl http://localhost:3000/api/cli
{
  "version": "0.1.0",
  "assets": [
    { "platform": "linux-x86_64",  "url": "http://localhost:3000/api/cli/linux-x86_64",  "available": true  },
    { "platform": "linux-aarch64", "url": "http://localhost:3000/api/cli/linux-aarch64", "available": true  },
    { "platform": "macos-aarch64", "url": "http://localhost:3000/api/cli/macos-aarch64", "available": true  }
  ]
}
```

`available: false` means the server was built without embedded CLI binaries
(local/dev builds). Release builds always have `available: true`.

### GET /api/cli/{platform}

Streams the CLI binary directly (no redirect, no GitHub dependency).

Supported platforms: `linux-x86_64`, `linux-aarch64`, `macos-aarch64`.

```bash
# Download and run immediately
curl -fL http://my-server:3000/api/cli/linux-x86_64 -o mcp-v8-cli
chmod +x mcp-v8-cli
./mcp-v8-cli --url http://my-server:3000 exec 'console.log("hello")'
```

Returns `404` with a JSON error for unknown platforms or dev builds without
embedded binaries.

---

## 2. CLI (`mcp-v8-cli`)

### Installation

**Directly from your server (recommended — always matches the server version):**

```bash
# Linux x86_64
curl -fL http://my-server:3000/api/cli/linux-x86_64 -o mcp-v8-cli && chmod +x mcp-v8-cli

# Linux ARM64
curl -fL http://my-server:3000/api/cli/linux-aarch64 -o mcp-v8-cli && chmod +x mcp-v8-cli

# macOS ARM64 (Apple Silicon)
curl -fL http://my-server:3000/api/cli/macos-aarch64 -o mcp-v8-cli && chmod +x mcp-v8-cli
```

**From a GitHub Release:**

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install-cli.sh | sudo bash
```

**From source:**

```bash
cargo install mcp-v8-client   # crates.io
# or from the repo root:
cargo build --release -p mcp-v8-client
# binary is at ./target/release/mcp-v8-cli
```

### Reference

```
mcp-v8-cli [OPTIONS] <COMMAND>

Options:
  --url <URL>   Base URL of the server [env: MCP_V8_URL] [default: http://localhost:3000]
  -j, --json    Output raw JSON instead of pretty-printed text

Commands:
  exec                        Submit JavaScript for async execution
  executions list             List all executions
  executions get <ID>         Get status and result of an execution
  executions output <ID>      Read console output (paginated)
  executions cancel <ID>      Cancel a running execution
```

### Walkthrough

#### Submit code

```bash
$ mcp-v8-cli exec 'console.log("hello"); 1 + 1'
✅ Execution queued
   execution_id: 6b54786c-12f3-4a28-8504-6061be815c95

Poll status:  mcp-v8-cli executions get 6b54786c-12f3-4a28-8504-6061be815c95
Read output:  mcp-v8-cli executions output 6b54786c-12f3-4a28-8504-6061be815c95
```

#### Poll status

```bash
$ mcp-v8-cli executions get 6b54786c-12f3-4a28-8504-6061be815c95
execution_id : 6b54786c-12f3-4a28-8504-6061be815c95
status       : completed
started_at   : 2026-05-04T23:44:03.859292833+00:00
completed_at : 2026-05-04T23:44:03.904357639+00:00
result       : 2
```

Statuses: `running` → `completed` / `failed` / `timed_out` / `cancelled`.

#### Read console output

```bash
$ mcp-v8-cli executions output 6b54786c-12f3-4a28-8504-6061be815c95
hello
```

Paginate large output with `--line-offset` / `--line-limit` (or byte-based
`--byte-offset` / `--byte-limit`). When `[more output available ...]` appears
on stderr, pass the printed `next_line_offset` value:

```bash
$ mcp-v8-cli executions output <ID> --line-offset 0 --line-limit 50
... (lines 0-49) ...
[more output available — next_line_offset=50 next_byte_offset=2048]

$ mcp-v8-cli executions output <ID> --line-offset 50 --line-limit 50
... (lines 50-99) ...
```

#### List executions

```bash
$ mcp-v8-cli executions list
EXECUTION ID                           STATUS       STARTED AT                 COMPLETED AT
----------------------------------------------------------------------------------------------------
6b54786c-12f3-4a28-8504-6061be815c95   completed    2026-05-04T23:44:03...     2026-05-04T23:44:03...
```

#### Cancel a running execution

```bash
$ mcp-v8-cli exec 'while(true){}'
✅ Execution queued
   execution_id: 23b9135e-c9d9-4e95-a1a1-0d6ed3977775

$ mcp-v8-cli executions cancel 23b9135e-c9d9-4e95-a1a1-0d6ed3977775
✅ Execution 23b9135e-c9d9-4e95-a1a1-0d6ed3977775 cancelled.
```

#### JSON output (pipe-friendly)

Every command accepts `--json` for machine-readable output:

```bash
# Grab the execution_id directly
ID=$(mcp-v8-cli --json exec 'Math.sqrt(16)' | jq -r .execution_id)

# Poll until done, then read output
until [ "$(mcp-v8-cli --json executions get "$ID" | jq -r .status)" = "completed" ]; do
  sleep 0.5
done
mcp-v8-cli executions output "$ID"
# 4
```

#### One-liner: download CLI, run it, get output

```bash
SERVER=http://my-server:3000

# Download CLI matching this server's version
curl -fL $SERVER/api/cli/linux-x86_64 -o mcp-v8-cli && chmod +x mcp-v8-cli

# Submit, poll, print output
ID=$(./mcp-v8-cli --url $SERVER --json exec 'console.log(1+1)' | jq -r .execution_id)
until [ "$(./mcp-v8-cli --url $SERVER --json executions get $ID | jq -r .status)" = "completed" ]; do sleep 0.2; done
./mcp-v8-cli --url $SERVER executions output $ID
# 2
```

### Optional flags for `exec`

```bash
# Restore a heap snapshot before running
mcp-v8-cli exec --heap my-snapshot-key 'state.counter += 1'

# Cap memory and timeout for this execution
mcp-v8-cli exec --heap-memory-max-mb 32 --execution-timeout-secs 10 'heavyWork()'

# Tag an execution for later filtering
mcp-v8-cli exec --tag env=prod --tag user=alice 'doSomething()'
```

---

## 3. Rust SDK (`mcp-v8-client`)

The crate wraps the auto-generated [progenitor](https://github.com/oxidecomputer/progenitor)
client with a thin async `Client` struct.

### Add the dependency

```toml
# Cargo.toml
[dependencies]
mcp-v8-client = "0.1.0"
tokio = { version = "1", features = ["full"] }
```

### Submit and poll

```rust
use mcp_v8_client::Client;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Client::new("http://localhost:3000");

    // 1. Submit code
    let body = mcp_v8_client::types::ExecRequest {
        code: r#"console.log("hello from Rust"); 2 + 2"#.to_string(),
        heap: None,
        session: None,
        heap_memory_max_mb: None,
        execution_timeout_secs: None,
        tags: None,
    };
    let exec_id = client.exec_handler(&body).await?.into_inner().execution_id;
    println!("queued: {exec_id}");

    // 2. Poll until terminal
    loop {
        let info = client.get_execution_handler(&exec_id).await?.into_inner();
        match info.status.as_str() {
            "completed" => {
                println!("result: {:?}", info.result);
                break;
            }
            "failed" | "timed_out" | "cancelled" => {
                eprintln!("execution ended: {}", info.status);
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }

    // 3. Read console output
    let page = client
        .get_execution_output_handler(&exec_id, None, None, None, None)
        .await?
        .into_inner();
    print!("{}", page.data); // "hello from Rust\n"

    Ok(())
}
```

### List and cancel

```rust
// List all executions
let list = client.list_executions_handler().await?.into_inner();
for exec in &list.executions {
    println!(
        "{} — {}",
        exec["execution_id"].as_str().unwrap_or("-"),
        exec["status"].as_str().unwrap_or("-"),
    );
}

// Cancel by ID
let result = client.cancel_execution_handler(&exec_id).await?.into_inner();
if result.ok {
    println!("cancelled");
} else {
    eprintln!("cancel failed: {}", result.error.unwrap_or_default());
}
```

### Paginating output

```rust
let mut line_offset: Option<i64> = None;

loop {
    let page = client
        .get_execution_output_handler(
            &exec_id,
            None,          // byte_limit
            None,          // byte_offset
            Some(50),      // line_limit
            line_offset,   // line_offset
        )
        .await?
        .into_inner();

    print!("{}", page.data);

    if !page.has_more {
        break;
    }
    line_offset = Some(page.next_line_offset as i64);
}
```

---

## 4. Generating a client for another language

The server emits an OpenAPI 3.0.3 spec. Any standard OpenAPI generator works.

### Step 1 — obtain the spec

```bash
# From a running server:
curl http://localhost:3000/api-doc/openapi.json -o openapi.json

# Or from the committed copy in the repo:
cp openapi.json my-project/openapi.json
```

### Step 2 — generate

**TypeScript / JavaScript** (using `openapi-typescript-codegen`):

```bash
npx openapi-typescript-codegen \
  --input openapi.json \
  --output ./src/generated \
  --client fetch
```

**Python** (using `openapi-python-client`):

```bash
pip install openapi-python-client
openapi-python-client generate --path openapi.json
```

**Go** (using `oapi-codegen`):

```bash
go install github.com/oapi-codegen/oapi-codegen/v2/cmd/oapi-codegen@latest
oapi-codegen -package mcp openapi.json > mcp/client.gen.go
```

**Any language** — OpenAPI Generator supports 50+ targets:

```bash
docker run --rm -v "$PWD:/out" openapitools/openapi-generator-cli generate \
  -i /out/openapi.json \
  -g <language>          \  # java, kotlin, csharp, ruby, swift6, ...
  -o /out/generated
```

Browse the full list: `openapi-generator-cli list`

### Step 3 — regenerate when the API changes

The committed `openapi.json` is always the canonical source. When the server
changes, regenerate:

```bash
cargo build --release -p server
./target/release/server --print-openapi > openapi.json
cp openapi.json mcp-v8-client/openapi.json
# re-run your generator here
```

CI enforces this — see the next section.

---

## 5. Keeping `openapi.json` up to date

A dedicated workflow (`.github/workflows/openapi-drift.yml`) runs on every
push and PR. It:

1. Builds the server binary.
2. Runs `--print-openapi` to regenerate the spec.
3. Fails with a clear message if either `openapi.json` or
   `mcp-v8-client/openapi.json` differs from what is committed.

If CI fails with **"openapi.json is out of date"**, run:

```bash
cargo build --release -p server
./target/release/server --print-openapi > openapi.json
cp openapi.json mcp-v8-client/openapi.json
git add openapi.json mcp-v8-client/openapi.json
git commit -m "chore: regenerate openapi.json"
```

---

## How the CLI binary embedding works

The CLI binary is embedded directly into the server binary at compile time
using `include_bytes!`. This means:

- No GitHub API calls at download time — the binary is served from memory
- The CLI always matches the server version exactly
- `GET /api/cli/{platform}` is a plain HTTP download, no redirects

### For release builds (CI)

The release workflow uses a two-pass build:

1. **Pass 1:** Build server → generate `openapi.json` → build CLI
2. **Pass 2:** Rebuild server with `MCP_V8_CLI_<PLATFORM>=/path/to/cli` env
   vars set → `server/build.rs` copies them into `OUT_DIR` → `include_bytes!`
   embeds them

### For local/dev builds

No env vars are set, so `build.rs` writes empty placeholder files. The
`/api/cli/{platform}` endpoint returns 404 with a message explaining how to
embed:

```
{"error": "CLI binary for 'linux-x86_64' is not embedded in this build.
           Set MCP_V8_CLI_LINUX_X86_64 at build time to embed it."}
```

To test the full flow locally:

```bash
# Build the CLI first
cargo build --release -p mcp-v8-client

# Rebuild the server with it embedded
MCP_V8_CLI_LINUX_X86_64=$(pwd)/target/release/mcp-v8-cli \
  cargo build --release -p server

# Now the server serves the CLI directly
./target/release/server --stateless --http-port 3000 &
curl -fL http://localhost:3000/api/cli/linux-x86_64 -o mcp-v8-cli
chmod +x mcp-v8-cli
./mcp-v8-cli --url http://localhost:3000 exec 'console.log(1+1)'
# → output: 2
```
