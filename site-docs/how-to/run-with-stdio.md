# Run with stdio

Start the server without `--http-port` or `--sse-port`:

```bash
mcp-v8 --directory-path /tmp/mcp-v8-heaps
```

Stdio is the default transport. Use this mode when an MCP client launches the
server as a subprocess and communicates over stdin/stdout.
