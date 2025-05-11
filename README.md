# mcp-v8: V8 JavaScript MCP Server

A Rust-based Model Context Protocol (MCP) server that exposes a V8 JavaScript runtime as a tool for AI agents like Claude and Cursor. Supports persistent heap snapshots via S3 or local filesystem, and is ready for integration with modern AI development environments.

## Features

- **V8 JavaScript Execution**: Run arbitrary JavaScript code in a secure, isolated V8 engine.
- **Heap Snapshots**: Persist and restore V8 heap state between runs, supporting both S3 and local file storage.
- **MCP Protocol**: Implements the Model Context Protocol for seamless tool integration with Claude, Cursor, and other MCP clients.
- **Configurable Storage**: Choose between S3 or local directory for heap storage at runtime.
- **Extensible**: Built for easy extension with new tools or storage backends.

## Installation

The easiest way to install `mcp-v8` is with the provided install script:

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This will automatically download and install the latest release for your platform to `/usr/local/bin/mcp-v8` (you may be prompted for your password).

Alternatively, you can build from source as described below.

## Quick Start

### Prerequisites
- Rust (nightly toolchain recommended) *(only required if building from source)*
- (Optional) AWS credentials for S3 storage

### Build the Server (Alternative to Install Script)

If you prefer to build from source instead of using the install script:

```bash
cd server
cargo build --release
```

### Run the Server

You can now run the server (whether installed via the script or built from source):

By default, the server uses S3 for heap storage. You can override this with CLI flags:

```bash
# Use S3 (default or specify bucket)
cargo run --release -- --s3-bucket my-bucket-name

# Use local filesystem
directory for heap storage
cargo run --release -- --directory-path /tmp/mcp-v8-heaps
```

## Integration

### Claude for Desktop

1. Build the server as above.
2. Open Claude Desktop → Settings → Developer → Edit Config.
3. Add your server to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "js": {
      "command": "/absolute/path/to/your/mcp-v8/server/target/release/server"
    }
  }
}
```

4. Restart Claude Desktop. The new tools will appear under the hammer icon.

### Cursor

1. Build the server as above.
2. Create or edit `.cursor/mcp.json` in your project root:

```json
{
  "mcpServers": {
    "js": {
      "command": "/absolute/path/to/your/mcp-v8/server/target/release/server"
    }
  }
}
```

3. Restart Cursor. The MCP tools will be available in the UI.

## Example Usage

- Ask Claude or Cursor: "Run this JavaScript: `1 + 2`"
- Use heap snapshots to persist state between runs.

## Heap Storage Options

- **S3**: Set `--s3-bucket <bucket>` to use AWS S3 for heap snapshots (requires AWS credentials).
- **Filesystem**: Set `--directory-path <path>` to use a local directory for heap snapshots.

## Limitations

While `mcp-v8` provides a powerful and persistent JavaScript execution environment, there are limitations to its runtime. 

- **No `async`/`await` or Promises**: Asynchronous JavaScript is not supported. All code must be synchronous.
- **No `fetch` or network access**: There is no built-in way to make HTTP requests or access the network.
- **No `console.log` or standard output**: Output from `console.log` or similar functions will not appear. To return results, ensure the value you want is the last line of your code.
- **No file system access**: The runtime does not provide access to the local file system or environment variables.
- **No `npm install` or external packages**: You cannot install or import npm packages. Only standard JavaScript (ECMAScript) built-ins are available.
- **No timers**: Functions like `setTimeout` and `setInterval` are not available.
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.
