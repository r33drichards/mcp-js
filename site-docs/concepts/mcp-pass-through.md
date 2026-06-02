# MCP Pass-Through

mcp-v8 can connect to upstream MCP servers and expose their tools to JavaScript code running inside the sandbox. This feature turns mcp-v8 into an MCP client and server simultaneously: it consumes tools from upstream servers and makes them available both to JavaScript code (via the `mcp` global) and optionally to its own MCP clients (via auto-generated tool stubs).

## Why Pass-Through Exists

AI agents often need to call multiple tools across different services. Without pass-through, the agent would need direct connections to each MCP server. Pass-through consolidates these connections: the agent connects only to mcp-v8, and mcp-v8 handles the fan-out to upstream servers.

This is particularly valuable when the agent needs to combine tool results with computation. For example, an agent might call a database MCP tool to fetch data, then process the data with JavaScript, then call an email MCP tool to send a summary. With pass-through, all of this happens through a single mcp-v8 connection.

## JavaScript API

When upstream MCP servers are configured, a `globalThis.mcp` object is injected into the JavaScript runtime:

### mcp.servers

A string array of connected server names:

```js
console.log(mcp.servers);  // ["database", "email", "slack"]
```

### mcp.listTools(server?)

Lists available tools, optionally filtered by server:

```js
// All tools from all servers
const allTools = mcp.listTools();

// Tools from a specific server
const dbTools = mcp.listTools("database");
// [{server: "database", name: "query", description: "...", inputSchema: {...}}, ...]
```

Each tool entry includes:
- `server` -- The server name
- `name` -- The tool name
- `description` -- Human-readable description
- `inputSchema` -- JSON Schema for the tool's input parameters

### mcp.callTool(server, tool, arguments)

Calls a tool on a specific server:

```js
const result = await mcp.callTool("database", "query", {
  sql: "SELECT * FROM users LIMIT 10"
});
// { content: [...], isError: false }
```

The result follows the MCP `CallToolResult` format:
- `content` -- Array of content items (text, images, etc.)
- `isError` -- Boolean indicating whether the tool reported an error

If the tool call fails, `mcp.callTool` throws an error rather than returning an error result. This makes error handling natural with try/catch.

## Supported Transports

mcp-v8 connects to upstream servers using two transport types:

### stdio

```
--mcp-server database=stdio:sqlite-mcp-server:--db:mydb.sqlite
```

The format is `name=stdio:command:arg1:arg2:...`. mcp-v8 spawns the upstream server as a child process and communicates over its stdin/stdout. Environment variables can be passed in the JSON config format.

### SSE

```
--mcp-server remote=sse:http://mcp-server:8080/sse
```

The format is `name=sse:url`. mcp-v8 connects to the upstream server's SSE endpoint.

For complex configurations with environment variables or multiple servers, use `--mcp-config` with a JSON file:

```json
[
  {
    "name": "database",
    "transport": "stdio",
    "command": "sqlite-mcp-server",
    "args": ["--db", "mydb.sqlite"],
    "env": { "LOG_LEVEL": "warn" }
  },
  {
    "name": "remote",
    "transport": "sse",
    "url": "http://mcp-server:8080/sse"
  }
]
```

## Tool Discovery and Caching

When mcp-v8 starts, it connects to all configured upstream servers and calls `tools/list` on each to discover available tools. The tool lists are cached in the `McpClientManager` and used for:

1. **JavaScript `mcp.listTools()`** -- Returns the cached tool metadata. No network call is needed at query time.
2. **MCP tool stubs** -- The cached tools are used to generate stub tool definitions on mcp-v8's own MCP interface.

Tool lists are fetched once at startup and not refreshed during the server's lifetime. If an upstream server adds or removes tools, mcp-v8 must be restarted to pick up the changes.

## Tool Stubs with Configurable Prefix

When `--mcp-stubs` is enabled (the default), mcp-v8 re-exports upstream tools on its own MCP interface. Each upstream tool is wrapped as a stub with a configurable prefix (default: `runjs__`):

An upstream tool `database.query` becomes `runjs__database__query` on mcp-v8's MCP interface.

When a client calls a stub tool, mcp-v8 generates JavaScript code that invokes `mcp.callTool()` with the appropriate parameters and executes it through the normal `run_js` path. This makes upstream tools accessible to MCP clients that connect only to mcp-v8.

The prefix can be customized via `--mcp-stub-prefix`, and stubbing can be disabled entirely with `--no-mcp-stubs` (or `--mcp-stubs=false`).

## Policy Gating

When an `mcp_tools` policy chain is configured (via `--policies-json`), every `mcp.callTool()` invocation is evaluated against the policy before the call reaches the upstream server. The policy input includes:

```json
{
  "operation": "mcp_call_tool",
  "server": "database",
  "tool": "query",
  "arguments": { "sql": "SELECT * FROM users" }
}
```

This allows operators to restrict which upstream tools can be called, or to limit calls based on the arguments (e.g., allowing only SELECT queries on the database tool).
