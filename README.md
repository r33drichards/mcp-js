# mcp-v8: V8 JavaScript MCP Server

A Rust-based Model Context Protocol (MCP) server that exposes a V8 JavaScript runtime as a tool for AI agents like Claude and Cursor. Supports persistent heap snapshots via S3 or local filesystem, and is ready for integration with modern AI development environments.

## Features

- **V8 JavaScript Execution**: Run arbitrary JavaScript code in a secure, isolated V8 engine.
- **Content-Addressed Heap Snapshots**: Persist and restore V8 heap state between runs using content-addressed storage, supporting both S3 and local file storage.
- **Stateless Mode**: Optional mode for fresh executions without heap persistence, ideal for serverless environments.
- **MCP Protocol**: Implements the Model Context Protocol for seamless tool integration with Claude, Cursor, and other MCP clients.
- **Configurable Storage**: Choose between S3, local directory, or stateless mode at runtime.
- **Multiple Transports**: Supports stdio, HTTP, and SSE (Server-Sent Events) transport protocols.

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

- `--s3-bucket <bucket>`: Use AWS S3 for heap snapshots. Specify the S3 bucket name. (Conflicts with `--stateless`)
- `--directory-path <path>`: Use a local directory for heap snapshots. Specify the directory path. (Conflicts with `--stateless`)
- `--stateless`: Run in stateless mode - no heap snapshots are saved or loaded. Each JavaScript execution starts with a fresh V8 isolate. (Conflicts with `--s3-bucket` and `--directory-path`)
- `--http-port <port>`: Enable HTTP transport on the specified port. If not provided, the server uses stdio transport (default).
- `--sse-port <port>`: Enable SSE (Server-Sent Events) transport on the specified port. (Conflicts with `--http-port`)
- `--heap-memory-max <megabytes>`: Maximum V8 heap memory per isolate in megabytes (1–64, default: 8).
- `--execution-timeout <seconds>`: Maximum execution timeout in seconds (1–300, default: 30).
- `--session-db-path <path>`: Path to the sled database used for session logging (default: `/tmp/mcp-v8-sessions`). Only applies in stateful mode. (Conflicts with `--stateless`)

**Note:** For heap storage, if neither `--s3-bucket`, `--directory-path`, nor `--stateless` is provided, the server defaults to using `/tmp/mcp-v8-heaps` as the local directory.

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

### HTTP Transport

The HTTP transport uses the HTTP/1.1 upgrade mechanism to switch from HTTP to the MCP protocol:

```bash
# Start HTTP server on port 8080 with local filesystem storage
mcp-v8 --directory-path /tmp/mcp-v8-heaps --http-port 8080

# Start HTTP server on port 8080 with S3 storage
mcp-v8 --s3-bucket my-bucket-name --http-port 8080

# Start HTTP server on port 8080 in stateless mode
mcp-v8 --stateless --http-port 8080
```

The HTTP transport is useful for:
- Network-based MCP clients
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

Each execution returns a `heap` content hash (e.g., `"a1b2c3d4"`) that identifies the snapshot. Pass this hash in the next `run_js` call to resume from that state. Omit `heap` to start a fresh session.

**Benefits:**
- **State persistence**: Variables and objects persist between runs
- **Content-addressed**: Snapshots are keyed by their FNV-1a hash — no naming collisions, safe concurrent access, and natural deduplication
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
      "command": "/usr/local/bin/mcp-v8 --s3-bucket my-bucket-name"
    }
  }
}
```

**Stateless mode:**
```json
{
  "mcpServers": {
    "js": {
      "command": "/usr/local/bin/mcp-v8 --stateless"
    }
  }
}
```

4. Restart Claude Desktop. The new tools will appear under the hammer icon.

### Cursor

1. Install the server as above.
2. Create or edit `.cursor/mcp.json` in your project root:

**Stateful mode with local filesystem:**
```json
{
  "mcpServers": {
    "js": {
      "command": "/usr/local/bin/mcp-v8 --directory-path /tmp/mcp-v8-heaps"
    }
  }
}
```

**Stateless mode:**
```json
{
  "mcpServers": {
    "js": {
      "command": "/usr/local/bin/mcp-v8 --stateless"
    }
  }
}
```

3. Restart Cursor. The MCP tools will be available in the UI.

### Claude (Web/Cloud) via Railway

You can also use the hosted version on Railway without installing anything locally:

**Option 1: Using Claude Settings**

1. Go to Claude's connectors settings page
2. Add a new custom connector:
   - **Name**: "mcp-v8"
   - **URL**: `https://mcp-js-production.up.railway.app/sse`

**Option 2: Using Claude Code CLI**

```bash
claude mcp add mcp-v8 -t sse https://mcp-js-production.up.railway.app/sse
```

Then test by running `claude` and asking: "Run this JavaScript: `[1,2,3].map(x => x * 2)`"

## Example Usage

- Ask Claude or Cursor: "Run this JavaScript: `1 + 2`"
- Use content-addressed heap snapshots to persist state between runs. The `run_js` tool returns a `heap` content hash after each execution — pass it back in the next call to resume that session.

## Heap Storage Options

You can configure heap storage using the following command line arguments:

- **S3**: `--s3-bucket <bucket>`
  - Example: `mcp-v8 --s3-bucket my-bucket-name`
  - Requires AWS credentials in your environment.
  - Ideal for cloud deployments and sharing state across instances.
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
# MCP-V8 Load Test Benchmark Report

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

