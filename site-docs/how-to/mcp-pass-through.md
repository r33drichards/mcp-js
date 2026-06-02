# How to Connect Upstream MCP Servers

Expose tools from external MCP servers inside the mcp-v8 JavaScript runtime.

## Add an upstream MCP server (stdio)

```bash
mcp-v8 --mcp-server myserver=stdio:my-mcp-binary:arg1:arg2
```

Format: `name=stdio:command:arg1:arg2:...`

The server is launched as a subprocess using stdio transport.

## Add an upstream MCP server (SSE)

```bash
mcp-v8 --mcp-server myserver=sse:http://other-server:3000/sse
```

Format: `name=sse:url`

## Add multiple servers

Repeat the flag:

```bash
mcp-v8 --mcp-server srv1=stdio:cmd1 --mcp-server srv2=sse:http://host:3000/sse
```

## Use a JSON config file

Create a config file:

```json
[
  {
    "name": "srv1",
    "transport": "stdio",
    "command": "my-mcp-binary",
    "args": ["--flag", "value"]
  },
  {
    "name": "srv2",
    "transport": "sse",
    "url": "http://other-server:3000/sse"
  }
]
```

Load it with:

```bash
mcp-v8 --mcp-config /etc/mcp-v8/mcp-servers.json
```

## Call upstream tools from JavaScript

Use the `mcp` global object:

```js
// List all connected servers
const servers = mcp.servers;
console.log(JSON.stringify(servers));

// List tools on a specific server
const tools = await mcp.listTools("srv1");
console.log(JSON.stringify(tools));

// Call a tool
const result = await mcp.callTool("srv1", "tool-name", { param: "value" });
console.log(JSON.stringify(result));
```

## Tool stubs for discovery

By default, when upstream servers are configured, mcp-v8 exposes stub tools on its own MCP interface. These appear in `tools/list` as `runjs__<server>__<tool>` and return instructional text telling calling agents to invoke the tool via `run_js` + `mcp.callTool(...)`.

Customize the prefix:

```bash
mcp-v8 --mcp-stub-prefix "js__"
```

Disable stubs entirely:

```bash
mcp-v8 --mcp-stubs false
```

## Gate upstream tool calls with OPA

Use the `mcp_tools` policy section to control which tools can be called:

```bash
mcp-v8 --mcp-server srv1=stdio:cmd1 \
       --policies-json '{"mcp_tools": {"policies": [{"url": "file:///etc/mcp-v8/mcp_tools.rego"}]}}'
```

See [Policy System](policy-system.md) for policy details.
