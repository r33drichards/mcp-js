# Quick Start: Codex

Use the same local stdio command in your Codex MCP server configuration:

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

If you want a fresh isolate for every run:

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

Once connected, ask Codex to run a short JavaScript snippet and confirm the
output.

For the server-side model, see [Connect as an MCP Server](../how-to/connect-as-an-mcp-server.md).
