# Programmatic Tool Calling

mcp-v8 can connect to external MCP servers at startup and expose their tools to JavaScript code running inside the V8 sandbox. This lets an AI agent call `run_js` once and have the JavaScript orchestrate multiple tool calls — instead of the model making one tool call per round trip.

## How It Works

```
AI Agent ←→ [1 tool: run_js] ←→ mcp-v8 ←→ External MCP Servers
                                    ↑
                            V8 sandbox with
                           mcp.callTool() API
```

The `--mcp-server` flag tells mcp-v8 to connect to an external MCP server and make its tools available inside the JavaScript runtime via a `globalThis.mcp` object:

```js
mcp.servers                                    // string[] — connected server names
mcp.listTools("server")                        // list tools with schemas
await mcp.callTool("server", "tool", { ... })  // call a tool and get results
```

The AI model sees only `run_js`. It writes JavaScript that discovers and calls external tools programmatically — no extra round trips to the model needed.

## Case Study: Constraint Solving with MiniZinc

This walkthrough connects mcp-v8 to the [MiniZinc MCP server](https://github.com/r33drichards/minizinc-mcp), a constraint solver that exposes a single `solve_constraint` tool. We solve three problems of increasing complexity entirely from JavaScript inside the sandbox.

### Prerequisites

- mcp-v8 installed ([install instructions](../README.md#installation))
- MiniZinc MCP server running locally or hosted

### Step 1: Start the MiniZinc MCP Server

If running locally:

```bash
git clone https://github.com/r33drichards/minizinc-mcp
cd minizinc-mcp
pip install -r requirements.txt
python main.py
```

This starts the MiniZinc MCP server on `http://localhost:8000` with SSE transport.

### Step 2: Start mcp-v8 with the MiniZinc Server Connected

```bash
mcp-v8 --stateless --http-port 3000 \
    --mcp-server 'minizinc=sse:http://localhost:8000/sse'
```

You should see:

```
MCP server 'minizinc': 1 tool(s) available
  - minizinc.solve_constraint
All MCP servers connected. JS code can use mcp.callTool(), mcp.listTools(), mcp.servers
Streamable HTTP server listening on 0.0.0.0:3000
```

### Step 3: Discover Available Tools

List connected servers and their tools from JavaScript:

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{"code": "console.log(JSON.stringify(mcp.servers))"}'
```

Output:

```json
["minizinc"]
```

List tools with full schemas:

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{"code": "console.log(JSON.stringify(mcp.listTools(\"minizinc\").map(t => ({ name: t.name, description: t.description.trim().split(String.fromCharCode(10))[0] }))))"}'
```

Output:

```json
[{"name": "solve_constraint", "description": "Solve a constraint satisfaction or optimization problem."}]
```

### Step 4: Solve the N-Queens Problem

The classic N-Queens problem: place N queens on an N×N chessboard so no two queens threaten each other.

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{
    "code": "(async () => { const result = await mcp.callTool(\"minizinc\", \"solve_constraint\", { problem: { model: \"int: n = 4;\\narray[1..n] of var 1..n: queens;\\ninclude \\\"alldifferent.mzn\\\";\\nconstraint alldifferent(queens);\\nconstraint alldifferent(i in 1..n)(queens[i] + i);\\nconstraint alldifferent(i in 1..n)(queens[i] - i);\\nsolve satisfy;\" } }); console.log(JSON.stringify(result, null, 2)); })()"
  }'
```

Result:

```json
{
  "content": [{
    "type": "text",
    "text": "{\"solutions\":[{\"variables\":{\"queens\":[3,1,4,2]},\"objective\":null,\"is_optimal\":false}],\"status\":\"SATISFIED\",\"solve_time\":0.000204,\"num_solutions\":1,\"error\":null}"
  }],
  "isError": false
}
```

Queens placed at rows `[3, 1, 4, 2]` — a valid solution where no two queens share a row, column, or diagonal.

### Step 5: Solve a Knapsack Optimization

A 0/1 knapsack problem: given items with weights and values, maximize total value without exceeding capacity.

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{
    "code": "(async () => { const model = \"int: n = 5;\\narray[1..n] of int: weight = [2, 3, 4, 5, 9];\\narray[1..n] of int: value = [3, 4, 8, 8, 10];\\nint: capacity = 20;\\narray[1..n] of var 0..1: x;\\nconstraint sum(i in 1..n)(weight[i] * x[i]) <= capacity;\\nsolve maximize sum(i in 1..n)(value[i] * x[i]);\"; const result = await mcp.callTool(\"minizinc\", \"solve_constraint\", { problem: { model } }); console.log(JSON.stringify(result, null, 2)); })()"
  }'
```

Result:

```json
{
  "solutions": [{
    "variables": {
      "x": [1, 0, 1, 1, 1],
      "objective": 29
    },
    "objective": 29.0,
    "is_optimal": true
  }],
  "status": "OPTIMAL_SOLUTION",
  "solve_time": 0.000387
}
```

The solver selects items 1, 3, 4, and 5 (total weight = 2+4+5+9 = 20, total value = 3+8+8+10 = 29) — provably optimal.

### Step 6: Solve a Graph Coloring Problem

Color 5 nodes of a graph with at most 3 colors such that no two adjacent nodes share a color.

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{
    "code": "(async () => { const model = \"int: n = 5;\\nint: c = 3;\\narray[1..n] of var 1..c: color;\\nconstraint color[1] != color[2];\\nconstraint color[1] != color[3];\\nconstraint color[2] != color[3];\\nconstraint color[2] != color[4];\\nconstraint color[3] != color[4];\\nconstraint color[3] != color[5];\\nconstraint color[4] != color[5];\\nsolve satisfy;\"; const result = await mcp.callTool(\"minizinc\", \"solve_constraint\", { problem: { model } }); console.log(JSON.stringify(result, null, 2)); })()"
  }'
```

Result:

```json
{
  "solutions": [{
    "variables": {
      "color": [2, 3, 1, 2, 3]
    }
  }],
  "status": "SATISFIED"
}
```

## Why Programmatic Tool Calling Matters

### Token Efficiency

When an AI agent connects directly to an MCP server with many tools, every tool schema is sent to the model on every turn. The [token comparison case study](token-comparison/README.md) measured this effect using the GitHub MCP server (26 tools):

```
                      Direct MCP       mcp-v8 proxy
                      (26 tools)        (1 tool)
 ─────────────────────────────────────────────────
  Avg input tokens     121,450         114,763
  Avg total tokens     122,056         117,826
  vs. Direct              —              -3%
```

The savings increase with the number of tools exposed. With a single `run_js` tool, the model context stays small regardless of how many external tools are connected behind mcp-v8.

### Batching Multiple Tool Calls

Without programmatic tool calling, each tool call requires a full round trip: model generates a tool call → client executes it → result sent back to model → model generates next call. With mcp-v8, JavaScript can chain multiple tool calls in a single `run_js` invocation:

```js
// One run_js call, multiple tool calls — no extra model round trips
const tools = mcp.listTools("github");
const repos = await mcp.callTool("github", "search_repositories", { query: "user:r33drichards" });
const issues = await mcp.callTool("github", "list_issues", { owner: "r33drichards", repo: "mcp-js" });
console.log(JSON.stringify({ repos, issues }));
```

### Connecting Multiple Servers

mcp-v8 can connect to multiple MCP servers simultaneously. Specify `--mcp-server` multiple times:

```bash
mcp-v8 --stateless --http-port 3000 \
    --mcp-server 'minizinc=sse:http://localhost:8000/sse' \
    --mcp-server 'github=stdio:npx:-y:@modelcontextprotocol/server-github'
```

JavaScript code can then call tools on any connected server:

```js
mcp.servers;  // ["minizinc", "github"]
await mcp.callTool("minizinc", "solve_constraint", { problem: { model: "..." } });
await mcp.callTool("github", "search_repositories", { query: "..." });
```

### OPA Policy Gating

When mcp-v8 is started with policy configuration, every `mcp.callTool()` invocation is evaluated against a Rego policy before the call is forwarded. This lets you restrict which tools can be called and with what arguments:

```rego
package mcp.tools

default allow = false

# Only allow calling solve_constraint on the minizinc server
allow if {
    input.operation == "mcp_call_tool"
    input.server == "minizinc"
    input.tool == "solve_constraint"
}
```

## Configuration Reference

### CLI Flags

| Flag | Description |
|------|-------------|
| `--mcp-server NAME=TRANSPORT:...` | Connect to an MCP server. Stdio: `name=stdio:command:arg1:arg2`. SSE: `name=sse:url`. Can be repeated. |
| `--mcp-config PATH` | JSON config file for MCP server connections. |

### JSON Config Format (`--mcp-config`)

```json
[
  {
    "name": "minizinc",
    "transport": "sse",
    "url": "http://localhost:8000/sse"
  },
  {
    "name": "github",
    "transport": "stdio",
    "command": "npx",
    "args": ["-y", "@modelcontextprotocol/server-github"],
    "env": {
      "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_..."
    }
  }
]
```

### JavaScript API

| API | Description |
|-----|-------------|
| `mcp.servers` | `string[]` — names of connected MCP servers |
| `mcp.listTools(server?)` | List tools with `name`, `description`, and `inputSchema`. Pass a server name to filter, or omit for all. |
| `await mcp.callTool(server, tool, args?)` | Call a tool. Returns `{ content: [...], isError: boolean }`. |
