# Tutorial: Connecting Upstream MCP Servers

In this tutorial you will configure mcp-v8 to connect to external MCP servers and call their tools from JavaScript. This lets you use mcp-v8 as a bridge that combines JavaScript execution with any MCP-compatible service.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- An external MCP server to connect to (examples included)

## Step 1: Understand MCP pass-through

MCP pass-through lets you connect upstream MCP servers to mcp-v8. The tools from those servers become available inside JavaScript code via the `globalThis.mcp` API. This means an AI agent can:

1. Call `run_js` on mcp-v8
2. Inside that JavaScript, call tools from external MCP servers
3. Combine the results with JavaScript logic
4. Return the processed output

Two connection types are supported:
- **stdio**: The upstream server is launched as a subprocess
- **SSE**: The upstream server is already running and accessible via SSE URL

## Step 2: Connect an upstream server via stdio

Use the `--mcp-server` flag to connect a server that runs as a subprocess:

```bash
mcp-v8 --http-port 3000 \
  --mcp-server "myserver=stdio:npx:-y:@modelcontextprotocol/server-everything"
```

The format is `name=stdio:command:arg1:arg2:...`. In this example:
- `myserver` is the name you will use in JavaScript
- `stdio` is the transport type
- `npx:-y:@modelcontextprotocol/server-everything` is the command with arguments (colons separate args)

## Step 3: List available tools from the upstream server

Once connected, the upstream server's tools are merged with mcp-v8's own tools. An AI client will see all of them. You can also discover them from JavaScript:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// List all connected MCP servers and their tools
const servers = await globalThis.mcp.listServers();
servers;
'
```

## Step 4: Call an upstream tool from JavaScript

Use `globalThis.mcp` to call tools on the upstream server:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// Call a tool on the upstream server
const result = await globalThis.mcp.callTool("myserver", "echo", {
  message: "Hello from mcp-v8!"
});
result;
'
```

The `callTool` function takes:
1. The server name (as specified in `--mcp-server`)
2. The tool name
3. The tool arguments as an object

## Step 5: Connect an upstream server via SSE

If the upstream server is already running with SSE transport, connect to it by URL:

```bash
mcp-v8 --http-port 3000 \
  --mcp-server "remote=sse:http://upstream-server:8080/sse"
```

The format is `name=sse:url`.

## Step 6: Connect multiple upstream servers

You can connect multiple servers at once:

```bash
mcp-v8 --http-port 3000 \
  --mcp-server "filesystem=stdio:npx:-y:@modelcontextprotocol/server-filesystem:/tmp" \
  --mcp-server "github=stdio:npx:-y:@modelcontextprotocol/server-github"
```

Each server gets its own name and is independently accessible from JavaScript.

## Step 7: Combine tools from multiple servers

The real power of pass-through is combining tools in JavaScript:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// Read a file using the filesystem MCP server
const fileContent = await globalThis.mcp.callTool("filesystem", "read_file", {
  path: "/tmp/data.json"
});

// Parse and process the data with JavaScript
const data = JSON.parse(fileContent.content);
const processed = data.map(item => ({
  ...item,
  processed: true,
  timestamp: new Date().toISOString(),
}));

// Return the combined result
JSON.stringify(processed, null, 2);
'
```

## Step 8: Handle tool errors

Upstream tool calls can fail. Handle errors with try/catch:

```bash
mcp-v8-cli --http-port 3000 exec --code '
try {
  const result = await globalThis.mcp.callTool("myserver", "nonexistent_tool", {});
  result;
} catch (error) {
  `Tool call failed: ${error.message}`;
}
'
```

## Step 9: Use pass-through with policies

If you have MCP tools policies configured, they gate which upstream tools can be called:

```json
{
  "mcp_tools": {
    "rego_source": "package mcp_v8.mcp_tools\n\ndefault allow = false\n\nallow {\n  input.server == \"filesystem\"\n  input.tool == \"read_file\"\n}\n\nallow {\n  input.server == \"filesystem\"\n  input.tool == \"list_directory\"\n}"
  }
}
```

This policy allows only `read_file` and `list_directory` from the filesystem server, denying all other tool calls.

## Step 10: Build a practical workflow

Here is a more complete example that demonstrates the power of combining JavaScript with upstream MCP tools:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// Step 1: List files in a directory
const listing = await globalThis.mcp.callTool("filesystem", "list_directory", {
  path: "/tmp/reports"
});

// Step 2: Read each JSON file
const reports = [];
for (const file of listing.entries) {
  if (file.name.endsWith(".json")) {
    const content = await globalThis.mcp.callTool("filesystem", "read_file", {
      path: `/tmp/reports/${file.name}`
    });
    reports.push(JSON.parse(content.content));
  }
}

// Step 3: Aggregate the data with JavaScript
const summary = {
  totalReports: reports.length,
  totalRevenue: reports.reduce((sum, r) => sum + (r.revenue || 0), 0),
  averageScore: reports.reduce((sum, r) => sum + (r.score || 0), 0) / reports.length,
};

JSON.stringify(summary, null, 2);
'
```

This workflow uses the filesystem MCP server to read files and JavaScript to process the data -- something neither tool could do alone.

## What you learned

- How to connect upstream MCP servers via stdio (`--mcp-server name=stdio:cmd:args`) and SSE (`--mcp-server name=sse:url`)
- How to call upstream tools from JavaScript using `globalThis.mcp.callTool()`
- How to connect and use multiple upstream servers simultaneously
- How to handle errors from upstream tool calls
- How to gate tool access with OPA policies
- How pass-through enables powerful workflows that combine JavaScript with external tools

Next, learn about Docker deployment in [Deploying with Docker](docker-deployment.md).
