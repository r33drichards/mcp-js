# mcp-v8: V8 JavaScript MCP Server

A Rust-based Model Context Protocol (MCP) server that exposes a V8 JavaScript runtime as a tool for AI agents like Claude and Cursor. Supports persistent heap snapshots via S3 or local filesystem, and is ready for integration with modern AI development environments.

## Features

- **V8 JavaScript Execution**: Run arbitrary JavaScript code in a secure, isolated V8 engine.
- **Resource Functions**: Built-in file system operations (`resource.read()`, `resource.list()`, `resource.write()`, `resource.delete()`) for managed file access within JavaScript.
- **Heap Snapshots**: Persist and restore V8 heap state between runs, supporting both S3 and local file storage.
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
- `--resource-directory <path>`: Directory path for resource storage (file system access from JavaScript). Defaults to `/tmp/mcp-v8-resources`.
- `--http-port <port>`: Enable HTTP transport on the specified port. If not provided, the server uses stdio transport (default).
- `--sse-port <port>`: Enable SSE (Server-Sent Events) transport on the specified port. (Conflicts with `--http-port`)

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

Stateful mode persists the V8 heap state between executions using either S3 or local filesystem storage.

**Benefits:**
- **State persistence**: Variables and objects persist between runs
- **Faster subsequent runs**: Preloaded context and data
- **Perfect for**: Interactive sessions, building up complex state over time

**Example use case:** Building a data structure incrementally, maintaining session state, or reusing expensive computations.

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
- Use heap snapshots to persist state between runs.

## Resource Functions

The V8 runtime includes built-in resource functions for managed file system access. These functions are available globally in your JavaScript code and operate within a sandboxed directory.

### Available Functions

#### `resource.read(uri)`
Read the contents of a file.

```javascript
// Read a file
const content = resource.read("data/example.txt");
// or with file:// URI scheme
const content = resource.read("file:///data/example.txt");
```

#### `resource.list(uri)`
List files and directories in a directory.

```javascript
// List files in a directory
const files = resource.list("data");
// Returns: ["file1.txt", "file2.txt", "subdir"]
```

#### `resource.write(uri, content)`
Write content to a file (creates parent directories if needed).

```javascript
// Write to a file
resource.write("data/output.txt", "Hello, world!");
```

#### `resource.delete(uri)`
Delete a file or directory.

```javascript
// Delete a file
resource.delete("data/temp.txt");
```

### Security

- All file operations are sandboxed to the directory specified by `--resource-directory` (default: `/tmp/mcp-v8-resources`)
- Path traversal attempts (e.g., `../../../etc/passwd`) are blocked
- URIs can use `file://` scheme or relative paths

### Example

```javascript
// Write some data
resource.write("users/alice.json", JSON.stringify({ name: "Alice", age: 30 }));

// Read it back
const data = JSON.parse(resource.read("users/alice.json"));

// List files
const files = resource.list("users");

// Clean up
resource.delete("users/alice.json");
```

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
- **Sandboxed file system access**: File operations via `resource.*` functions are restricted to the configured resource directory. See [Resource Functions](#resource-functions) for details.
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
