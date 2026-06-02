# Quick Start: Claude Code CLI
Add the MCP server to Claude Code using the claude mcp add command:

Stdio transport (local):

# Stateful mode with local filesystem
```
claude mcp add mcp-v8 -- mcp-v8 --directory-path /tmp/mcp-v8-heaps
```
# Stateless mode
```
claude mcp add mcp-v8 -- mcp-v8 --stateless
```
SSE transport (remote):

```
claude mcp add mcp-v8 -t sse https://mcp-js-production.up.railway.app/sse
```

Then test by running claude and asking: Run this JavaScript: `console.log([1,2,3].map(x => x * 2))`