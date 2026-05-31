# Claude for Desktop

Open Claude Desktop → Settings → Developer → Edit Config.
Add your server to claude_desktop_config.json:
Stateful mode with S3:

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
Stateful mode with local filesystem:
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