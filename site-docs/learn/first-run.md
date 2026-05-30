# Quick Start

Pick the entry point you want:

- [Quick Start: Claude Code](#quick-start-claude-code)
- [Quick Start: Codex](#quick-start-codex)
- [Quick Start: Cursor](#quick-start-cursor)
- [Quick Start: Generic MCP](#quick-start-generic-mcp)
- [Quick Start: curl](#quick-start-curl)
- [Quick Start: Bundled CLI](#quick-start-bundled-cli)

If you do not already have `mcp-v8` installed, start with the install guide:

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

That is the same GitHub install script used in [Install the Server](../how-to/install-server.md).

## Quick Start: Claude Code

Add the server to Claude Code:

```bash
claude mcp add mcp-v8 -- mcp-v8 --directory-path /tmp/mcp-v8-heaps
```

For a stateless setup instead:

```bash
claude mcp add mcp-v8 -- mcp-v8 --stateless
```

Then ask Claude Code:

```text
Run this JavaScript: console.log(1 + 2)
```

For more detail, see [Connect as an MCP Server](../how-to/connect-as-an-mcp-server.md)
and [Run with stdio](../how-to/run-with-stdio.md).

## Quick Start: Codex

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

## Quick Start: Cursor

Create or edit `.cursor/mcp.json`:

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

For stateless mode:

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

Restart Cursor, then ask it to run:

```text
console.log(1 + 2)
```

For more detail, see [Connect as an MCP Server](../how-to/connect-as-an-mcp-server.md).

## Quick Start: Generic MCP

If your MCP client launches local subprocess servers over stdio, use this
shape:

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

If your client connects to remote Streamable HTTP MCP servers, start `mcp-v8`
like this:

```bash
mcp-v8 --directory-path /tmp/mcp-v8-heaps --http-port 3000
```

Then point the client at:

```text
http://localhost:3000/mcp
```

See [Run with stdio](../how-to/run-with-stdio.md),
[Run with HTTP](../how-to/run-with-http.md), and
[Connect as an MCP Server](../how-to/connect-as-an-mcp-server.md).

## Quick Start: curl

Start the server in stateless HTTP mode:

```bash
mcp-v8 --stateless --http-port 3000
```

Submit a small execution:

```bash
EXECUTION_ID="$(
  curl -s http://localhost:3000/api/exec \
    -H 'content-type: application/json' \
    -d '{"code":"console.log(1 + 1)"}' \
    | jq -r '.execution_id'
)"
```

Read the output:

```bash
curl -s "http://localhost:3000/api/executions/${EXECUTION_ID}/output" | jq -r '.data'
```

Success looks like:

- the server starts
- `/api/exec` returns an execution ID
- the output endpoint returns `2`

See [Run with HTTP](../how-to/run-with-http.md) and
[HTTP API](../reference/http-api.md).

## Quick Start: Bundled CLI

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
