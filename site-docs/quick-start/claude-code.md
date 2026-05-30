# Quick Start: Claude Code

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
