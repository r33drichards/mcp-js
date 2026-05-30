# Quick Start: Bundled CLI

Start the server in HTTP mode:

```bash
mcp-v8 --stateless --http-port 3000
```

Inspect the server's embedded CLI download index:

```bash
curl -s http://localhost:3000/api/cli | jq
```

Download the platform binary directly from the server:

```bash
curl -Lo mcp-v8-cli http://localhost:3000/api/cli/linux-x86_64
chmod +x ./mcp-v8-cli
```

Then use it:

```bash
export MCP_V8_URL=http://localhost:3000
./mcp-v8-cli exec 'console.log(1 + 2)'
```

See [Use CLI](../how-to/use-cli.md) and [HTTP API](../reference/http-api.md).
