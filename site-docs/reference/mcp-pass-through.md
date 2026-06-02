# MCP Pass-Through Reference

Connect to external MCP servers and expose their tools to JavaScript code running in the V8 runtime.

## --mcp-server Flag

```
--mcp-server <NAME>=<TRANSPORT>:<DETAILS>
```

Can be specified multiple times. Duplicate names are rejected.

### stdio Transport

```
--mcp-server <NAME>=stdio:<COMMAND>:<ARG1>:<ARG2>...
```

Arguments are separated by colons.

**Example:**

```
--mcp-server math=stdio:python3:math_server.py
```

### SSE Transport

```
--mcp-server <NAME>=sse:<URL>
```

**Example:**

```
--mcp-server api=sse:http://localhost:9000/sse
```

## --mcp-config JSON Format

Path to a JSON file with an array of server configurations.

```json
[
  {
    "name": "math",
    "transport": "stdio",
    "command": "python3",
    "args": ["math_server.py"],
    "env": {}
  },
  {
    "name": "api",
    "transport": "sse",
    "url": "http://localhost:9000/sse"
  }
]
```

### stdio Config Schema

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | `string` | Yes | -- | Server name |
| `transport` | `string` | Yes | -- | Must be `"stdio"` |
| `command` | `string` | Yes | -- | Command to execute |
| `args` | `string[]` | No | `[]` | Command arguments |
| `env` | `object` | No | `{}` | Environment variables |

### SSE Config Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `string` | Yes | Server name |
| `transport` | `string` | Yes | Must be `"sse"` |
| `url` | `string` | Yes | SSE endpoint URL |

## JavaScript API

The `mcp` global object is available when at least one MCP server is configured.

### mcp.servers

```js
mcp.servers  // string[]
```

Array of connected server names.

### mcp.listTools(server?)

```js
mcp.listTools("math")   // tools from specific server
mcp.listTools()          // all tools from all servers
```

**Returns:** Array of tool info objects:

```json
[
  {
    "server": "math",
    "name": "add",
    "description": "Add two numbers",
    "inputSchema": {
      "type": "object",
      "properties": {
        "a": {"type": "number"},
        "b": {"type": "number"}
      }
    }
  }
]
```

### mcp.callTool(server, tool, args)

```js
const result = await mcp.callTool("math", "add", { a: 1, b: 2 })
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `server` | `string` | Yes | Server name |
| `tool` | `string` | Yes | Tool name |
| `args` | `object` | Yes | Tool arguments |

**Returns:**

```json
{
  "content": [{"type": "text", "text": "3"}],
  "isError": false
}
```

Throws `McpToolError` on error.

## Stub Tools

When `--mcp-stubs` is `true` (the default when MCP servers are configured), upstream MCP server tools are exposed as stub tools on the mcp-v8 server itself.

### Stub Tool Naming

```
<prefix><server>__<tool>
```

Default prefix: `runjs__`

Example: upstream server `math` with tool `add` becomes `runjs__math__add`.

### Stub Tool Behavior

Calling a stub tool does **not** execute the upstream tool directly. Instead, it returns instructional text telling the calling agent to invoke the tool from JavaScript via `run_js` + `mcp.callTool(...)`.

### Stub Configuration

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--mcp-stubs` | `bool` | `true` | Enable/disable stub tool exposure |
| `--mcp-stub-prefix` | `string` | `"runjs__"` | Prefix for stub tool names |

## MCP Tools Policy

When the `mcp_tools` section is configured in `--policies-json`, each `mcp.callTool()` invocation is evaluated against the policy chain before dispatching to the upstream server.

Policy input schema:

```json
{
  "operation": "mcp_call_tool",
  "server": "math",
  "tool": "add",
  "arguments": {"a": 1, "b": 2}
}
```

Default OPA REST path: `mcp/tools`
Default local Rego rule: `data.mcp.tools.allow`
