# Calling upstream MCP servers

We'll connect mcp-v8 to a running MCP server, list its tools from JavaScript, and call one — all in a few steps.

## Prerequisites

- mcp-v8 installed ([install guide](../install/overview.md))
- Node.js available to run the example upstream server:

  ```bash
  npm install -g @modelcontextprotocol/server-filesystem
  ```

## Step 1 — Start mcp-v8 with the upstream server

Pass the upstream server with `--mcp-server`. The stdio format is `name=stdio:command:arg1:arg2`.

```bash
mcp-v8 --http-port 8080 \
  --mcp-server fs=stdio:npx:-y:@modelcontextprotocol/server-filesystem:/tmp
```

mcp-v8 spawns the filesystem server as a child process, completes the MCP handshake, and logs each discovered tool:

```
INFO MCP server 'fs': 5 tool(s) available
INFO   - fs.read_file
INFO   - fs.write_file
INFO   - fs.list_directory
INFO   ...
```

## Step 2 — List the upstream tools from JavaScript

Connect any MCP client to `http://localhost:8080/mcp` and call `run_js`:

```js
const tools = mcp.listTools();
console.log(JSON.stringify(tools, null, 2));
```

Each entry in the returned array describes one upstream tool:

```json
[
  {
    "server": "fs",
    "name": "read_file",
    "description": "Read the complete contents of a file from the filesystem.",
    "inputSchema": {
      "type": "object",
      "properties": {
        "path": { "type": "string" }
      },
      "required": ["path"]
    }
  }
]
```

## Step 3 — Call an upstream tool

```js
// Write a file, then read it back
await mcp.callTool("fs", "write_file", {
  path: "/tmp/hello.txt",
  content: "Hello from mcp-v8!"
});

const result = await mcp.callTool("fs", "read_file", { path: "/tmp/hello.txt" });
console.log(result.content[0].text);
// → Hello from mcp-v8!
```

`mcp.callTool` returns `{ content: [...], isError: false }`. If the upstream tool reports an error it throws a `McpToolError`.

## Step 4 — Combine multiple tool calls

Because `mcp.callTool` is a plain `async` function, standard JavaScript patterns apply:

```js
// Fan out — run two tool calls in parallel
const [listing, stat] = await Promise.all([
  mcp.callTool("fs", "list_directory", { path: "/tmp" }),
  mcp.callTool("fs", "read_file", { path: "/tmp/hello.txt" }),
]);
console.log(listing.content[0].text);
console.log(stat.content[0].text);
```

## Step 5 — Observe stub tools in the tool list

With stubs enabled (the default), the upstream tools also appear in mcp-v8's own tool list using the prefix `runjs__`:

```
runjs__fs__read_file
runjs__fs__write_file
runjs__fs__list_directory
...
```

Calling one of these stub tools directly from an MCP client returns instructions on how to invoke it through `run_js`; it does not execute the upstream tool. See the [how-to guide](../how-to/mcp-client.md) to control this behaviour.

## See also

- [How-to guide](../how-to/mcp-client.md)
- [Concepts](../concepts/mcp-client.md)
- [Reference](../reference/mcp-client.md)
- [Running JavaScript & TypeScript](../tutorials/js-execution.md)
- [Security policies](../concepts/policies.md)
- [MCP tools reference](../reference/mcp-tools.md)
