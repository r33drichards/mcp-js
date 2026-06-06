# Calling upstream MCP servers — Reference

Complete reference for upstream MCP server configuration flags and the JavaScript `mcp` API.

## CLI flags

### `--mcp-server`

Type: string (repeatable)

Connect one upstream MCP server. May be repeated. All names must be unique across `--mcp-server` entries and `--mcp-config` entries combined.

**Stdio transport** — spawns the command as a child process via the MCP stdio transport:

```
--mcp-server name=stdio:command:arg1:arg2:...
```

Arguments after the command are delimited by `:`. The subprocess inherits the mcp-v8 process environment; additional environment variables can only be set via `--mcp-config`.

```bash
--mcp-server github=stdio:npx:-y:@modelcontextprotocol/server-github
--mcp-server fs=stdio:mcp-filesystem-server:/tmp
```

**SSE transport** — connects to an existing MCP server over Server-Sent Events:

```
--mcp-server name=sse:URL
```

```bash
--mcp-server analytics=sse:http://analytics-mcp.internal:9000/sse
```

---

### `--mcp-config`

Type: string (file path)  
Default: none

Path to a JSON file containing a JSON array of `McpServerConfig` objects (see schema below). Combined with any `--mcp-server` entries; all server names must be unique.

---

### `--mcp-stubs`

Type: bool  
Default: `true`

When `true`, each upstream tool is surfaced in mcp-v8's own `list_tools` response as a stub tool. Calling a stub tool directly returns a text response instructing the caller to use `run_js` with `mcp.callTool`; it does not execute the upstream tool.

When `false`, upstream tools are not included in `list_tools` at all. They remain callable from JavaScript via `mcp.callTool`.

---

### `--mcp-stub-prefix`

Type: string  
Default: `runjs__`

Prefix prepended to every stub tool name. The full stub name format is:

```
{prefix}{server}__{tool}
```

The separator between server name and tool name is always `__` (two underscores). Changing this prefix has no effect on the JavaScript `mcp` API; the API always uses the original server and tool names.

---

## `--mcp-config` JSON schema

The file must contain a JSON array. Each element is one of the following object shapes.

### Stdio server object

```json
{
  "name": "<unique-server-name>",
  "transport": "stdio",
  "command": "<executable>",
  "args": ["<arg1>", "<arg2>"],
  "env": {
    "<ENV_VAR>": "<value>"
  }
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | — | Unique logical name; used in the JS API and in stub tool names |
| `transport` | `"stdio"` | yes | — | Transport discriminator |
| `command` | string | yes | — | Executable to spawn |
| `args` | string\[\] | no | `[]` | Command-line arguments passed to the executable |
| `env` | object (string → string) | no | `{}` | Additional environment variables for the subprocess |

### SSE server object

```json
{
  "name": "<unique-server-name>",
  "transport": "sse",
  "url": "<SSE endpoint URL>"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique logical name; used in the JS API and in stub tool names |
| `transport` | `"sse"` | yes | Transport discriminator |
| `url` | string | yes | Full URL of the upstream server's SSE endpoint |

---

## JavaScript `mcp` global

The `mcp` global is injected into every V8 execution when at least one upstream server is configured. It is not available when no upstream servers are connected.

### `mcp.servers`

Type: getter property → `string[]`

Returns an array of the names of all connected upstream MCP servers.

```js
mcp.servers  // e.g. ["github", "analytics"]
```

---

### `mcp.listTools([serverName])`

Type: synchronous function

Parameters:

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `serverName` | string | no | If provided, return only tools for this server. If omitted, return tools for all servers. |

Returns: `ToolInfo[]`

```ts
interface ToolInfo {
  server: string;           // name of the connected server
  name: string;             // upstream tool name
  description: string | null;
  inputSchema: object;      // JSON Schema object for the tool's input
}
```

Examples:

```js
// All tools across all servers
const all = mcp.listTools();

// Tools on one server only
const githubTools = mcp.listTools("github");
```

Throws an `Error` if `serverName` refers to a server that is not connected.

---

### `mcp.callTool(serverName, toolName[, args])`

Type: async function → `Promise<CallToolResult>`

Parameters:

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `serverName` | string | yes | Name of the connected server to call |
| `toolName` | string | yes | Name of the tool to invoke |
| `args` | object | no | Arguments for the tool; must be JSON-serialisable. Omit or pass `undefined` for tools that take no arguments. |

Returns:

```ts
interface CallToolResult {
  content: unknown[];   // array of MCP content items from the upstream server
  isError: boolean;     // true when the upstream tool reported an error
}
```

Throws `McpToolError` when the upstream tool returns a result with `isError: true`.  
Throws a `TypeError` if `serverName` or `toolName` is not a string.  
Throws an `Error` if `serverName` refers to a server that is not connected, if the tool is not found, or if the policy denies the call.

```js
const result = await mcp.callTool("github", "list_issues", {
  owner: "acme",
  repo: "api",
});
// result.content[0] is a MCP content item, e.g. { type: "text", text: "..." }
```

---

### `McpToolError`

Thrown by `mcp.callTool` when the upstream tool returns `isError: true`.

| Property | Type | Description |
|----------|------|-------------|
| `name` | `"McpToolError"` | Error class name |
| `message` | string | `"mcp.callTool {server}.{tool} failed: {first content text}"` |
| `serverName` | string | Name of the server that was called |
| `toolName` | string | Name of the tool that was called |
| `result` | `CallToolResult` | The full result object with `isError: true` |

```js
try {
  await mcp.callTool("github", "create_issue", { owner: "acme", repo: "api", title: "Bug" });
} catch (e) {
  if (e instanceof McpToolError) {
    console.error(e.serverName, e.toolName, e.result);
  }
}
```

---

## Stub tool naming

| Component | Value |
|-----------|-------|
| Default prefix | `runjs__` |
| Server–tool separator | `__` (two underscores) |
| Full pattern | `{prefix}{server}__{tool}` |

Example: server `github`, tool `create_issue`, default prefix → `runjs__github__create_issue`.

Stub tools carry the same `inputSchema` as the upstream tool. Their description is replaced with a notice that the tool is a stub and code showing how to call it via `mcp.callTool`. Annotations from the upstream tool are not included in the stub.

---

## `mcp_tools` policy input

Rego entrypoint: `data.mcp.tools.allow`

Evaluated on every `mcp.callTool` call before the upstream server is contacted.

```json
{
  "operation": "mcp_call_tool",
  "server": "<server-name>",
  "tool": "<tool-name>",
  "arguments": { ... }
}
```

`arguments` is `null` when no arguments are passed to `mcp.callTool`.

If the policy returns `false`, `mcp.callTool` throws a JavaScript error; the upstream server is not contacted.

## See also

- [How-to guide](../how-to/mcp-client.md)
- [Concepts](../concepts/mcp-client.md)
- [Security policies](../concepts/policies.md)
- [MCP tools reference](../reference/mcp-tools.md)
