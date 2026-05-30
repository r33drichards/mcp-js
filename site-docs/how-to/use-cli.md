# Use CLI

`mcp-v8-cli` is a convenience client for the HTTP API. It is useful for shell
automation, quick testing, and environments where you do not need a full MCP
client.

Start the server in HTTP mode:

```bash
mcp-v8 --stateless --http-port 3000
```

Set the base URL once:

```bash
export MCP_V8_URL=http://localhost:3000
```

Submit code:

```bash
mcp-v8-cli exec 'console.log("hello"); 1 + 1'
```

Poll status:

```bash
mcp-v8-cli executions get <execution-id>
```

Read output:

```bash
mcp-v8-cli executions output <execution-id>
```
