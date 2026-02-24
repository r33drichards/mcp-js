# mcp-v8: V8 JavaScript MCP Server

A Rust-based Model Context Protocol (MCP) server that exposes a V8 JavaScript runtime as a tool for AI agents like Claude and Cursor. Supports persistent heap snapshots via S3 or local filesystem, and is ready for integration with modern AI development environments.

## Features

- **V8 JavaScript Execution**: Run arbitrary JavaScript code in a secure, isolated V8 engine.
- **TypeScript Support**: Run TypeScript code directly — types are stripped before execution using [SWC](https://swc.rs/). This is type removal only, not type checking.
- **WebAssembly Support**: Compile and run WASM modules using the standard `WebAssembly` JavaScript API (`WebAssembly.Module`, `WebAssembly.Instance`, `WebAssembly.validate`).
- **Content-Addressed Heap Snapshots**: Persist and restore V8 heap state between runs using content-addressed storage, supporting both S3 and local file storage.
- **Stateless Mode**: Optional mode for fresh executions without heap persistence, ideal for serverless environments.
- **MCP Protocol**: Implements the Model Context Protocol for seamless tool integration with Claude, Cursor, and other MCP clients.
- **Configurable Storage**: Choose between S3, local directory, or stateless mode at runtime.
- **Multiple Transports**: Supports stdio, Streamable HTTP (MCP 2025-03-26+), and SSE (Server-Sent Events) transport protocols.
- **Clustering**: Optional Raft-based clustering for distributed coordination, replicated session logging, and horizontal scaling.
- **Concurrency Control**: Configurable concurrent V8 execution limits with semaphore-based throttling.

## Installation

Install `mcp-v8` using the provided install script:

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This will automatically download and install the latest release for your platform to `/usr/local/bin/mcp-v8` (you may be prompted for your password).

---

*Advanced users: If you prefer to build from source, see the [Build from Source](#build-from-source) section at the end of this document.*

## Command Line Arguments

`mcp-v8` supports the following command line arguments:

### Storage Options

- `--s3-bucket <bucket>`: Use AWS S3 for heap snapshots. Specify the S3 bucket name. (Conflicts with `--stateless`)
- `--cache-dir <path>`: Local filesystem cache directory for S3 write-through caching. Reduces latency by caching snapshots locally. (Requires `--s3-bucket`)
- `--directory-path <path>`: Use a local directory for heap snapshots. Specify the directory path. (Conflicts with `--stateless`)
- `--stateless`: Run in stateless mode - no heap snapshots are saved or loaded. Each JavaScript execution starts with a fresh V8 isolate. (Conflicts with `--s3-bucket` and `--directory-path`)

**Note:** For heap storage, if neither `--s3-bucket`, `--directory-path`, nor `--stateless` is provided, the server defaults to using `/tmp/mcp-v8-heaps` as the local directory.

### Transport Options

- `--http-port <port>`: Enable Streamable HTTP transport (MCP 2025-03-26+) on the specified port. Serves the MCP endpoint at `/mcp` and a plain API at `/api/exec`. If not provided, the server uses stdio transport (default). (Conflicts with `--sse-port`)
- `--sse-port <port>`: Enable SSE (Server-Sent Events) transport on the specified port. Exposes `/sse` for the event stream and `/message` for client requests. (Conflicts with `--http-port`)

### Execution Limits

- `--heap-memory-max <megabytes>`: Maximum V8 heap memory per isolate in megabytes (1–64, default: 8).
- `--execution-timeout <seconds>`: Maximum execution timeout in seconds (1–300, default: 30).
- `--max-concurrent-executions <n>`: Maximum number of concurrent V8 executions (default: CPU core count). Controls how many JavaScript executions can run in parallel.

### Session Logging

- `--session-db-path <path>`: Path to the sled database used for session logging (default: `/tmp/mcp-v8-sessions`). Only applies in stateful mode. (Conflicts with `--stateless`)

### Cluster Options

These options enable Raft-based clustering for distributed coordination and replicated session logging.

- `--cluster-port <port>`: Port for the Raft cluster HTTP server. Enables cluster mode when set. (Requires `--http-port` or `--sse-port`)
- `--node-id <id>`: Unique node identifier within the cluster (default: `node1`).
- `--peers <peers>`: Comma-separated list of seed peer addresses. Format: `id@host:port` or `host:port`. Peers can also join dynamically via `POST /raft/join`.
- `--join <address>`: Join an existing cluster by contacting this seed address (`host:port`). The node registers itself with the cluster leader.
- `--advertise-addr <addr>`: Advertise address for this node (`host:port`). Used for peer discovery and write forwarding. Defaults to `<node-id>:<cluster-port>`.
- `--heartbeat-interval <ms>`: Raft heartbeat interval in milliseconds (default: 100).
- `--election-timeout-min <ms>`: Minimum election timeout in milliseconds (default: 300).
- `--election-timeout-max <ms>`: Maximum election timeout in milliseconds (default: 500).

### WASM Module Options

Pre-load WebAssembly modules that are available as global variables in every JavaScript execution.

- `--wasm-module <name>=<path>`: Pre-load a `.wasm` file and expose its exports as a global variable with the given name. Can be specified multiple times for multiple modules.
- `--wasm-config <path>`: Path to a JSON config file mapping global names to `.wasm` file paths. Format: `{"name": "/path/to/module.wasm", ...}`.

Both options can be used together. CLI flags and config file entries are merged; duplicate names cause an error.

**Example — CLI flags:**
```bash
mcp-v8 --stateless --wasm-module math=/path/to/math.wasm --wasm-module crypto=/path/to/crypto.wasm
```

**Example — Config file** (`wasm-modules.json`):
```json
{
  "math": "/path/to/math.wasm",
  "crypto": "/path/to/crypto.wasm"
}
```
```bash
mcp-v8 --stateless --wasm-config wasm-modules.json
```

After loading, the module exports are available directly in JavaScript:
```javascript
math.add(21, 21); // → 42
```

**Note:** Only self-contained WASM modules (no imports) are supported. Modules requiring imported memory, tables, or other imports will fail to instantiate.

## Quick Start

After installation, you can run the server directly. Choose one of the following options:

### Stdio Transport (Default)

```bash
# Use S3 for heap storage (recommended for cloud/persistent use)
mcp-v8 --s3-bucket my-bucket-name

# Use local filesystem directory for heap storage (recommended for local development)
mcp-v8 --directory-path /tmp/mcp-v8-heaps

# Use stateless mode - no heap persistence (recommended for one-off computations)
mcp-v8 --stateless
```

### HTTP Transport (Streamable HTTP)

The HTTP transport uses the Streamable HTTP protocol (MCP 2025-03-26+), which supports bidirectional communication over standard HTTP. The MCP endpoint is served at `/mcp`:

```bash
# Start HTTP server on port 8080 with local filesystem storage
mcp-v8 --directory-path /tmp/mcp-v8-heaps --http-port 8080

# Start HTTP server on port 8080 with S3 storage
mcp-v8 --s3-bucket my-bucket-name --http-port 8080

# Start HTTP server on port 8080 in stateless mode
mcp-v8 --stateless --http-port 8080
```

The HTTP transport also exposes a plain HTTP API at `POST /api/exec` for direct JavaScript execution without MCP framing.

The HTTP transport is useful for:
- Network-based MCP clients
- Load-balanced and horizontally-scaled deployments
- Testing and debugging with tools like the MCP Inspector
- Containerized deployments
- Remote MCP server access

### SSE Transport

Server-Sent Events (SSE) transport for streaming responses:

```bash
# Start SSE server on port 8081 with local filesystem storage
mcp-v8 --directory-path /tmp/mcp-v8-heaps --sse-port 8081

# Start SSE server on port 8081 in stateless mode
mcp-v8 --stateless --sse-port 8081
```

## Stateless vs Stateful Mode

### Stateless Mode (`--stateless`)

Stateless mode runs each JavaScript execution in a fresh V8 isolate without any heap persistence.

**Benefits:**
- **Faster execution**: No snapshot creation/serialization overhead
- **No storage I/O**: Doesn't read or write heap files
- **Fresh isolates**: Every JS execution starts clean
- **Perfect for**: One-off computations, stateless functions, serverless environments

**Example use case:** Simple calculations, data transformations, or any scenario where you don't need to persist state between executions.

### Stateful Mode (default)

Stateful mode persists the V8 heap state between executions using content-addressed storage backed by either S3 or local filesystem.

Each execution returns a `heap` content hash (a 64-character SHA-256 hex string) that identifies the snapshot. Pass this hash in the next `run_js` call to resume from that state. Omit `heap` to start a fresh session.

**Benefits:**
- **State persistence**: Variables and objects persist between runs
- **Content-addressed**: Snapshots are keyed by their SHA-256 hash — no naming collisions, safe concurrent access, and natural deduplication
- **Immutable snapshots**: Once written, a snapshot at a given hash never changes
- **Perfect for**: Interactive sessions, building up complex state over time

**Example use case:** Building a data structure incrementally, maintaining session state, or reusing expensive computations.

#### Named Sessions

You can tag executions with a human-readable **session name** by passing the `session` parameter to `run_js`. When a session name is provided, the server logs each execution (input heap, output heap, code, and timestamp) to an embedded sled database.

Two additional tools are available in stateful mode for browsing session history:

- **`list_sessions`** — Returns an array of all session names that have been used.
- **`list_session_snapshots`** — Returns the log entries for a given session. Accepts a required `session` parameter and an optional `fields` parameter (comma-separated) to select specific fields: `index`, `input_heap`, `output_heap`, `code`, `timestamp`.

The session database path defaults to `/tmp/mcp-v8-sessions` and can be overridden with `--session-db-path`.

**Example workflow:**

1. Call `run_js` with `code: "var x = 1; x;"` and `session: "my-project"` — the execution is logged.
2. Pass the returned `heap` hash and `session: "my-project"` in subsequent calls to continue and log the session.
3. Call `list_sessions` to see `["my-project"]`.
4. Call `list_session_snapshots` with `session: "my-project"` to see the full execution history.

## Integration

### Claude for Desktop

1. Install the server as above.
2. Open Claude Desktop → Settings → Developer → Edit Config.
3. Add your server to `claude_desktop_config.json`:

**Stateful mode with S3:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--s3-bucket", "my-bucket-name"]
    }
  }
}
```

**Stateful mode with local filesystem:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--directory-path", "/tmp/mcp-v8-heaps"]
    }
  }
}
```

**Stateless mode:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--stateless"]
    }
  }
}
```

4. Restart Claude Desktop. The new tools will appear under the hammer icon.

### Claude Code CLI

Add the MCP server to Claude Code using the `claude mcp add` command:

**Stdio transport (local):**
```bash
# Stateful mode with local filesystem
claude mcp add mcp-v8 -- mcp-v8 --directory-path /tmp/mcp-v8-heaps

# Stateless mode
claude mcp add mcp-v8 -- mcp-v8 --stateless
```

**SSE transport (remote):**
```bash
claude mcp add mcp-v8 -t sse https://mcp-js-production.up.railway.app/sse
```

Then test by running `claude` and asking: "Run this JavaScript: `[1,2,3].map(x => x * 2)`"

### Cursor

1. Install the server as above.
2. Create or edit `.cursor/mcp.json` in your project root:

**Stateful mode with local filesystem:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--directory-path", "/tmp/mcp-v8-heaps"]
    }
  }
}
```

**Stateless mode:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--stateless"]
    }
  }
}
```

3. Restart Cursor. The MCP tools will be available in the UI.

### Claude (Web/Cloud) via Railway

You can also use the hosted version on Railway without installing anything locally:

1. Go to Claude's connectors settings page
2. Add a new custom connector:
   - **Name**: "mcp-v8"
   - **URL**: `https://mcp-js-production.up.railway.app/sse`

## Example Usage

- Ask Claude or Cursor: "Run this JavaScript: `1 + 2`"
- Use content-addressed heap snapshots to persist state between runs. The `run_js` tool returns a `heap` content hash after each execution — pass it back in the next call to resume that session.

### WebAssembly

You can compile and run WebAssembly modules using the standard `WebAssembly` JavaScript API:

```javascript
const wasmBytes = new Uint8Array([
  0x00,0x61,0x73,0x6d, // magic
  0x01,0x00,0x00,0x00, // version
  0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f, // type: (i32,i32)->i32
  0x03,0x02,0x01,0x00, // function section
  0x07,0x07,0x01,0x03,0x61,0x64,0x64,0x00,0x00, // export "add"
  0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6a,0x0b // body: local.get 0, local.get 1, i32.add
]);
const mod = new WebAssembly.Module(wasmBytes);
const inst = new WebAssembly.Instance(mod);
inst.exports.add(21, 21); // → 42
```

This uses synchronous compilation (`WebAssembly.Module` / `WebAssembly.Instance`), which works within the V8 runtime. The async APIs (`WebAssembly.compile`, `WebAssembly.instantiate` returning Promises) are not supported since the runtime does not support async/await.

Alternatively, you can pre-load `.wasm` files at server startup using `--wasm-module` or `--wasm-config` so they are available as globals in every execution without inline byte arrays. See [WASM Module Options](#wasm-module-options) for details.

## Heap Storage Options

You can configure heap storage using the following command line arguments:

- **S3**: `--s3-bucket <bucket>`
  - Example: `mcp-v8 --s3-bucket my-bucket-name`
  - Requires AWS credentials in your environment.
  - Ideal for cloud deployments and sharing state across instances.
- **S3 with write-through cache**: `--s3-bucket <bucket> --cache-dir <path>`
  - Example: `mcp-v8 --s3-bucket my-bucket-name --cache-dir /tmp/mcp-v8-cache`
  - Reads from local cache first, writes to both local cache and S3.
  - Reduces latency for repeated snapshot access.
- **Filesystem**: `--directory-path <path>`
  - Example: `mcp-v8 --directory-path /tmp/mcp-v8-heaps`
  - Stores heap snapshots locally on disk.
  - Ideal for local development and testing.
- **Stateless**: `--stateless`
  - Example: `mcp-v8 --stateless`
  - No heap persistence - each execution starts fresh.
  - Ideal for one-off computations and serverless environments.

**Note:** Only one storage option can be used at a time. If multiple are provided, the server will return an error.

## Limitations

While `mcp-v8` provides a powerful and persistent JavaScript execution environment, there are limitations to its runtime. 

- **No `async`/`await` or Promises**: Asynchronous JavaScript is not supported. All code must be synchronous.
- **No `fetch` or network access**: There is no built-in way to make HTTP requests or access the network.
- **No `console.log` or standard output**: Output from `console.log` or similar functions will not appear. To return results, ensure the value you want is the last line of your code.
- **No file system access**: The runtime does not provide access to the local file system or environment variables.
- **No `npm install` or external packages**: You cannot install or import npm packages. Only standard JavaScript (ECMAScript) built-ins are available.
- **No timers**: Functions like `setTimeout` and `setInterval` are not available.
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.
- **WebAssembly: synchronous API only**: `WebAssembly.Module`, `WebAssembly.Instance`, and `WebAssembly.validate` work. The async APIs (`WebAssembly.compile`, `WebAssembly.instantiate` returning Promises) are not supported.
- **TypeScript: type removal only**: TypeScript type annotations are stripped before execution. No type checking is performed — invalid types are silently removed, not reported as errors.

---

## Build from Source (Advanced)

If you prefer to build from source instead of using the install script:

### Prerequisites
- Rust (nightly toolchain recommended)
- (Optional) AWS credentials for S3 storage

### Build the Server

```bash
cd server
cargo build --release
```

The built binary will be located at `server/target/release/server`. You can use this path in the integration steps above instead of `/usr/local/bin/mcp-v8` if desired.

<!-- load-test-report -->
# MCP-V8 Load Test Benchmark Report v0.1.0

Comparison of single-node vs 3-node cluster at various request rates.

## Results

ran on railway gha runners on [pr](https://github.com/r33drichards/mcp-js/pull/36#issuecomment-3946074130)

| Topology | Target Rate | Actual Iter/s | HTTP Req/s | Exec Avg (ms) | Exec p95 (ms) | Exec p99 (ms) | Success % | Dropped | Max VUs |
|----------|-------------|---------------|------------|----------------|----------------|----------------|-----------|---------|---------|
| cluster-stateful | 100/s | 99.5 | 99.5 | 44.9 | 196.88 | 416.99 | 100% | 31 | 41 |
| cluster-stateful | 200/s | 199.6 | 199.6 | 23.22 | 79.32 | 131.13 | 100% | 13 | 33 |
| cluster-stateless | 1000/s | 999.9 | 999.9 | 3.82 | 7.72 | 13.09 | 100% | 0 | 100 |
| cluster-stateless | 100/s | 100 | 100 | 3.67 | 5.65 | 8.03 | 100% | 0 | 10 |
| cluster-stateless | 200/s | 200 | 200 | 3.56 | 5.9 | 8.61 | 100% | 0 | 20 |
| cluster-stateless | 500/s | 500 | 500 | 3.42 | 5.85 | 9.2 | 100% | 0 | 50 |
| single-stateful | 100/s | 99.1 | 99.1 | 215.12 | 362.5 | 376.6 | 100% | 32 | 42 |
| single-stateful | 200/s | 97.8 | 97.8 | 1948.82 | 2212.55 | 2960.96 | 100% | 5939 | 200 |
| single-stateless | 1000/s | 977.1 | 977.1 | 60.98 | 482.98 | 602.38 | 100% | 843 | 561 |
| single-stateless | 100/s | 100 | 100 | 3.71 | 5.73 | 8.73 | 100% | 0 | 10 |
| single-stateless | 200/s | 200 | 200 | 3.61 | 5.43 | 7.74 | 100% | 0 | 20 |
| single-stateless | 500/s | 500 | 500 | 4.67 | 8.49 | 27.98 | 100% | 0 | 50 |

## P95 Latency

| Topology | Rate | P95 (ms) | |
|----------|------|----------|-|
| cluster-stateful | 100/s | 196.88 | `█████████████████████` |
| cluster-stateful | 200/s | 79.32 | `█████████████████` |
| cluster-stateless | 100/s | 5.65 | `███████` |
| cluster-stateless | 200/s | 5.9 | `███████` |
| cluster-stateless | 500/s | 5.85 | `███████` |
| cluster-stateless | 1000/s | 7.72 | `████████` |
| single-stateful | 100/s | 362.5 | `███████████████████████` |
| single-stateful | 200/s | 2212.55 | `██████████████████████████████` |
| single-stateless | 100/s | 5.73 | `███████` |
| single-stateless | 200/s | 5.43 | `██████` |
| single-stateless | 500/s | 8.49 | `████████` |
| single-stateless | 1000/s | 482.98 | `████████████████████████` |

## Notes

- **Target Rate**: The configured constant-arrival-rate (requests/second k6 attempts)
- **Actual Iter/s**: Achieved iterations per second (each iteration = 1 POST /api/exec)
- **HTTP Req/s**: Total HTTP requests per second (1 per iteration)
- **Dropped**: Iterations k6 couldn't schedule because VUs were exhausted (indicates server saturation)
- **Topology**: `single` = 1 MCP-V8 node; `cluster` = 3 MCP-V8 nodes with Raft

