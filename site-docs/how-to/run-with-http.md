# Run with HTTP

Start the server with Streamable HTTP enabled:

```bash
./target/release/server --stateless --http-port 3000
```

This exposes:

- MCP at `/mcp`
- REST API endpoints under `/api/...`
- OpenAPI JSON at `/api-doc/openapi.json`
