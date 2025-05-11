# mcp-v8: V8 JavaScript MCP Server

A Rust-based Model Context Protocol (MCP) server that exposes a V8 JavaScript runtime as a tool for AI agents like Claude and Cursor. Supports persistent heap snapshots via S3 or local filesystem, and is ready for integration with modern AI development environments.

## Features

- **V8 JavaScript Execution**: Run arbitrary JavaScript code in a secure, isolated V8 engine.
- **Heap Snapshots**: Persist and restore V8 heap state between runs, supporting both S3 and local file storage.
- **MCP Protocol**: Implements the Model Context Protocol for seamless tool integration with Claude, Cursor, and other MCP clients.
- **Configurable Storage**: Choose between S3 or local directory for heap storage at runtime.
- **Extensible**: Built for easy extension with new tools or storage backends.

## Quick Start

### Prerequisites
- Rust (nightly toolchain recommended)
- (Optional) AWS credentials for S3 storage

### Build the Server

```bash
cd server
cargo build --release
```

### Run the Server

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
