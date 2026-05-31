# Connect as an MCP Server

The primary way to use `mcp-v8` is to run it as an MCP server and connect an
MCP client to it.

## Local stdio integration

Use stdio when the client launches `mcp-v8` as a subprocess:

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

## Remote HTTP integration

Use Streamable HTTP when the MCP client can connect to a remote endpoint:

```text
http://your-host:3000/mcp
```

For clients that accept URL-based MCP configuration directly, the config can
look like this:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "url": "http://localhost:3000/mcp"
    }
  }
}
```

For Codex, MCP servers are configured in `config.toml`:

```toml
[mcp_servers.mcp-v8]
url = "http://localhost:3000/mcp"
```

This mode is better for load-balanced, containerized, or remotely hosted
deployments.

See [Run with stdio](run-with-stdio.md) and [Run with HTTP](run-with-http.md)
for the server-side startup commands, and [Transports](../concepts/transports.md)
for the connection model behind them.
