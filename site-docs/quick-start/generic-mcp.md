# Quick Start: Generic MCP

If your MCP client launches local subprocess servers over stdio, use this
shape:

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

If your client connects to remote Streamable HTTP MCP servers, start `mcp-v8`
like this:

```bash
mcp-v8 --directory-path /tmp/mcp-v8-heaps --http-port 3000
```

Then point the client at:

```text
http://localhost:3000/mcp
```

For clients with JSON-based MCP configuration, that usually means a config like:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "url": "http://localhost:3000/mcp"
    }
  }
}
```

See [Run with stdio](../how-to/run-with-stdio.md),
[Run with HTTP](../how-to/run-with-http.md), and
[Connect as an MCP Server](../how-to/connect-as-an-mcp-server.md).
