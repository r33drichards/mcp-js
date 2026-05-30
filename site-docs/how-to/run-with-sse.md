# Run with SSE

Start the server with SSE transport enabled:

```bash
mcp-v8 --stateless --sse-port 3000
```

This exposes:

- SSE stream at `/sse`
- POST message endpoint at `/message`
