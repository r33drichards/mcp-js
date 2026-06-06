# Transports: stdio, HTTP, SSE

We'll connect an MCP client to mcp-v8 over stdio, then switch to Streamable HTTP and verify the server is reachable over plain HTTP.

## Prerequisites

`mcp-v8` is installed and on your `PATH`. Run `mcp-v8 --version` to confirm. If not, see the [install overview](../install/overview.md).

## Part 1 — stdio (local MCP client)

stdio is the default transport. When neither `--http-port` nor `--sse-port` is given, the server reads MCP JSON-RPC from stdin and writes responses to stdout. The MCP client is responsible for spawning and managing the process.

**Step 1.** Confirm the binary starts without error:

```bash
mcp-v8
```

The process blocks waiting for MCP messages. Press Ctrl-C to stop it — you would not normally run it this way; an MCP client does it for you.

**Step 2.** Add mcp-v8 to your MCP client's configuration. The exact file location varies by client. For Claude Desktop on macOS the config lives at `~/Library/Application Support/Claude/claude_desktop_config.json`:

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

Restart the client. It spawns `mcp-v8`, performs the MCP `initialize` handshake, and lists tools including `run_js`.

**Step 3.** Ask the client to call `run_js` with a simple expression:

```json
{
  "code": "1 + 1"
}
```

The tool returns an `execution_id`. In stateful mode you poll `get_execution` with that ID to retrieve the result.

## Part 2 — Streamable HTTP

Streamable HTTP is the right choice when you want to connect from a remote machine, a container, or any HTTP-capable MCP client — without spawning a subprocess.

**Step 1.** Start the server on port 8080:

```bash
mcp-v8 --http-port=8080
```

The server logs:

```
Streamable HTTP server listening on 0.0.0.0:8080
```

**Step 2.** Send an MCP `initialize` request with curl to confirm the endpoint:

```bash
curl -s -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {"name": "test", "version": "0.1"}
    }
  }'
```

A successful response includes `serverInfo` with the server version and a `capabilities` object.

**Step 3.** Confirm the bundled OpenAPI spec is available:

```bash
curl -s http://localhost:8080/api-doc/openapi.json | head -c 120
```

You should see the beginning of the JSON spec for the REST sidecar API (`/api/*`).

**Step 4.** Configure an HTTP-capable MCP client to connect to `http://localhost:8080/mcp` using the Streamable HTTP transport.

## See also

- [How-to guide](../how-to/transports.md)
- [Concepts](../concepts/transports.md)
- [Reference](../reference/transports.md)
- [Async execution tutorial](../tutorials/async-execution.md)
- [Authentication tutorial](../tutorials/authentication.md)
- [Clustering tutorial](../tutorials/clustering.md)
- [CLI flags reference](../reference/cli-flags.md)
