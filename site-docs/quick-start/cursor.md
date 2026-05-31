# Quick Start: Cursor

Create or edit `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": ["--directory-path", "/tmp/mcp-v8-heaps"]
    }
  }
}
```

For stateless mode:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": ["--stateless"]
    }
  }
}
```

Restart Cursor, then ask it to run:

```text
console.log(1 + 2)
```

If your Cursor setup uses a remote HTTP MCP endpoint instead of a local stdio
process, the config can look like this:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "url": "http://localhost:3000/mcp"
    }
  }
}
```

For more detail, see [Connect as an MCP Server](../how-to/connect-as-an-mcp-server.md).
