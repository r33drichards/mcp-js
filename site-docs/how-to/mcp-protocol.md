# How to Connect mcp-v8 to MCP Clients

Configure Claude Desktop, Claude Code, Cursor, and other MCP clients to use mcp-v8.

## Claude Desktop

Edit `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": []
    }
  }
}
```

With additional flags:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": ["--directory-path", "/tmp/heaps", "--allow-external-modules"]
    }
  }
}
```

## Claude Code

```bash
claude mcp add mcp-v8 -- mcp-v8
```

With flags:

```bash
claude mcp add mcp-v8 -- mcp-v8 --directory-path /tmp/heaps --allow-external-modules
```

## Cursor

Create `.cursor/mcp.json` in your project root:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "command": "mcp-v8",
      "args": []
    }
  }
}
```

## Connect to a remote server

If mcp-v8 is running with `--http-port`:

```json
{
  "mcpServers": {
    "mcp-v8": {
      "url": "http://your-server:3000/mcp"
    }
  }
}
```

For SSE transport (`--sse-port`):

```json
{
  "mcpServers": {
    "mcp-v8": {
      "url": "http://your-server:3000/sse"
    }
  }
}
```

## Available MCP resources

Once connected, clients can read these resources via `resources/list`:

| URI | Description |
|-----|-------------|
| `docs://readme` | Full README |
| `docs://llms-txt` | llms.txt agent guide |
| `docs://openapi` | OpenAPI 3.0 spec (JSON) |
| `docs://tools` | MCP tool list (JSON) |

## JWT authentication

If the server is configured with `--jwks-url`, clients must send a valid JWT bearer token in the `Authorization` header during MCP initialization:

```bash
mcp-v8 --http-port 3000 --jwks-url https://auth.example.com/.well-known/jwks.json
```
