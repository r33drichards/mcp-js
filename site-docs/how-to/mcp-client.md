# Calling upstream MCP servers

Recipes for connecting upstream MCP servers to mcp-v8 and using them from JavaScript.

## Connect a stdio server

Use `--mcp-server name=stdio:command:arg1:arg2`. Arguments after the command are separated by `:`.

```bash
mcp-v8 --http-port 8080 \
  --mcp-server github=stdio:npx:-y:@modelcontextprotocol/server-github
```

The process is spawned as a child of mcp-v8 and kept alive for the lifetime of the server.

## Connect an SSE server

Use `--mcp-server name=sse:URL`. mcp-v8 opens an SSE connection to the given endpoint at startup.

```bash
mcp-v8 --http-port 8080 \
  --mcp-server analytics=sse:http://analytics-mcp.internal:9000/sse
```

## Connect multiple servers at once

Repeat `--mcp-server` for each server. Names must be unique.

```bash
mcp-v8 --http-port 8080 \
  --mcp-server github=stdio:npx:-y:@modelcontextprotocol/server-github \
  --mcp-server db=sse:http://db-mcp.internal:9000/sse
```

## Use --mcp-config for richer configuration

`--mcp-config` takes a path to a JSON file. Use this when you need to pass environment variables to a stdio subprocess or when managing many servers.

Create `mcp-servers.json`:

```json
[
  {
    "name": "github",
    "transport": "stdio",
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-github"],
    "env": {
      "GITHUB_TOKEN": "ghp_xxxxxxxxxxxxxxxxxxxx"
    }
  },
  {
    "name": "analytics",
    "transport": "sse",
    "url": "http://analytics-mcp.internal:9000/sse"
  }
]
```

Pass the file to mcp-v8:

```bash
mcp-v8 --http-port 8080 --mcp-config mcp-servers.json
```

`--mcp-server` entries and `--mcp-config` entries are merged at startup; all server names must be unique across both sources.

## Call an upstream tool from JavaScript

Inside a `run_js` execution the global `mcp` object is always available when servers are configured.

```js
// List all tools across all servers
const all = mcp.listTools();

// List tools from one server
const tools = mcp.listTools("github");

// Call a tool
const result = await mcp.callTool("github", "list_issues", {
  owner: "acme",
  repo: "api",
});
console.log(result.content[0].text);
```

## Handle upstream tool errors

`mcp.callTool` throws a `McpToolError` when the upstream tool returns `isError: true`.

```js
try {
  await mcp.callTool("github", "create_issue", {
    owner: "acme",
    repo: "api",
    title: "New bug",
  });
} catch (e) {
  if (e instanceof McpToolError) {
    console.error("upstream tool failed:", e.result);
  } else {
    throw e;
  }
}
```

## Disable stub tools

By default, every upstream tool is also listed in mcp-v8's own `list_tools` response as a stub. To turn this off:

```bash
mcp-v8 --mcp-server myserver=stdio:mcp-my-server --mcp-stubs false
```

With stubs disabled, upstream tools do not appear in `list_tools` at all; they remain callable from JavaScript via `mcp.callTool`.

## Change the stub prefix

The default stub prefix is `runjs__`, giving names like `runjs__github__create_issue`. To use a different prefix:

```bash
mcp-v8 \
  --mcp-server github=stdio:npx:-y:@modelcontextprotocol/server-github \
  --mcp-stub-prefix "up__"
```

With this setting the stub tool appears as `up__github__create_issue`. The JavaScript `mcp` API is unaffected — always use the original server and tool names there.

## Gate upstream calls with a policy

Add an `mcp_tools` entry to your `--policies-json` config. The Rego entrypoint is `data.mcp.tools.allow` and the policy input is:

```json
{
  "operation": "mcp_call_tool",
  "server": "<server-name>",
  "tool": "<tool-name>",
  "arguments": { ... }
}
```

Example policy — allow only read operations on the `github` server:

```rego
package mcp.tools

default allow := false

allow if {
    input.server == "github"
    input.tool in {"list_issues", "get_issue", "list_repos", "get_repo"}
}
```

Pass it via `--policies-json`:

```bash
mcp-v8 \
  --mcp-server github=stdio:npx:-y:@modelcontextprotocol/server-github \
  --policies-json '{"mcp_tools": {"policies": [{"inline": "package mcp.tools\ndefault allow := false\nallow if { input.tool in {\"list_issues\",\"get_issue\"} }"}]}}'
```

See the [policies concepts page](../concepts/policies.md) for the full policy configuration format.

## See also

- [Tutorial](../tutorials/mcp-client.md)
- [Concepts](../concepts/mcp-client.md)
- [Reference](../reference/mcp-client.md)
- [Running JavaScript & TypeScript](../tutorials/js-execution.md)
- [Security policies](../concepts/policies.md)
- [MCP tools reference](../reference/mcp-tools.md)
