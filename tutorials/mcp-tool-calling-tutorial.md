# MCP Programmatic Tool Calling Tutorial

This tutorial compares two ways to build an AI agent that uses GitHub MCP tools, and shows why **programmatic tool calling via mcp-js** is dramatically more token-efficient.

**Task:** Build an agent loop that answers *"What repos have I committed to in the past 24 hours?"* using the GitHub MCP server's tools.

## The Two Approaches

| | **Approach A: MCP Server Agent Loop** | **Approach B: mcp-js Programmatic Tool Calling** |
|---|---|---|
| **How it works** | Claude talks directly to all 30+ GitHub MCP tools | Claude writes JS that calls `mcp.callTool()` inside mcp-js |
| **Tools sent to Claude** | ~30 tool definitions (~4,000+ tokens each turn) | 1 tool definition (`run_js`, ~150 tokens each turn) |
| **Script** | [`mcp_server_agent_loop.py`](mcp_server_agent_loop.py) | [`mcpjs_agent_loop.py`](mcpjs_agent_loop.py) |

## Prerequisites

```bash
# Python dependencies
pip install anthropic mcp requests

# GitHub MCP server
npm install -g @modelcontextprotocol/server-github

# Environment variables
export ANTHROPIC_API_KEY="sk-ant-..."
export GITHUB_PERSONAL_ACCESS_TOKEN="ghp_..."
```

## Approach A: MCP Server Agent Loop

This is the standard pattern. Your Python script:

1. Spawns `@modelcontextprotocol/server-github` as a child process via stdio
2. Discovers all tools through the MCP protocol
3. Converts them to Anthropic tool format and sends **all of them** to Claude on every turn
4. When Claude calls a tool, routes it to the MCP server and sends results back

```python
# Connect to GitHub MCP server via stdio
server_params = StdioServerParameters(
    command="npx",
    args=["-y", "@modelcontextprotocol/server-github"],
    env={"GITHUB_PERSONAL_ACCESS_TOKEN": token, **os.environ},
)

async with stdio_client(server_params) as (read, write):
    async with ClientSession(read, write) as session:
        await session.initialize()

        # Discover tools — this returns 30+ tool definitions
        tools_result = await session.list_tools()
        anthropic_tools = [
            {"name": t.name, "description": t.description, "input_schema": t.inputSchema}
            for t in tools_result.tools
        ]

        # Agent loop: send ALL tools to Claude on every turn
        response = client.messages.create(
            model="claude-sonnet-4-20250514",
            tools=anthropic_tools,   # <-- ~4,000 tokens of tool defs
            messages=messages,
        )
```

### Run it

```bash
python tutorials/mcp_server_agent_loop.py
```

### Expected output

```
Discovered 34 tools from GitHub MCP server

--- Turn 1 ---
  tokens: input=7532  output=187
  calling tool: search_users({"q": "type:user", ...})
--- Turn 2 ---
  tokens: input=8104  output=156
  calling tool: list_commits({"owner": "...", ...})
--- Turn 3 ---
  tokens: input=12847  output=203
...

=======================================================
TOKEN USAGE SUMMARY  (MCP Server Agent Loop)
=======================================================
  Turn  1: input=  7532  output=  187
  Turn  2: input=  8104  output=  156
  Turn  3: input= 12847  output=  203
  Turn  4: input= 13421  output=  312
-------------------------------------------------------
  TOTAL   : input= 41904  output=  858
  GRAND TOTAL TOKENS: 42762
=======================================================
```

The key observation: **input tokens are 7,000+ per turn** because Claude receives all 34 tool definitions every time.

## Approach B: mcp-js Programmatic Tool Calling

This approach uses mcp-js as a **tool proxy**. Claude only sees one tool (`run_js`) and writes JavaScript to call GitHub tools programmatically:

```python
# Only ONE tool definition — ~150 tokens instead of ~4,000
MCPJS_TOOL = {
    "name": "run_js",
    "description": (
        "Execute JavaScript in the mcp-js V8 sandbox. The sandbox has access to "
        "external MCP servers via the global `mcp` object:\n"
        "  mcp.servers — string[] of connected server names\n"
        "  mcp.listTools(server?) — list tools with name, description, inputSchema\n"
        "  await mcp.callTool(server, tool, args) — call a tool\n"
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "code": {"type": "string", "description": "JavaScript code to execute."}
        },
        "required": ["code"],
    },
}

# Agent loop: send just 1 tool to Claude
response = client.messages.create(
    model="claude-sonnet-4-20250514",
    tools=[MCPJS_TOOL],           # <-- ~150 tokens of tool defs
    messages=messages,
)
```

Claude writes JS like:

```javascript
// Turn 1: Discover tools
const tools = mcp.listTools("github");
console.log(JSON.stringify(tools.map(t => t.name)));

// Turn 2: Search for recent commits
const result = await mcp.callTool("github", "search_issues", {
  q: "author:r33drichards type:pr merged:>2024-03-01"
});
console.log(JSON.stringify(result));
```

### Setup

Start mcp-js with the GitHub MCP server connected:

```yaml
# docker-compose.yml
services:
  mcp-js:
    build: .
    command:
      - --stateless
      - --http-port=3000
      - --mcp-server=github=stdio:npx:-y:@modelcontextprotocol/server-github
    environment:
      - GITHUB_PERSONAL_ACCESS_TOKEN=${GITHUB_PERSONAL_ACCESS_TOKEN}
    ports:
      - "3000:3000"
```

```bash
docker compose up --build -d
```

Or run the binary directly:

```bash
mcp-v8 --stateless --http-port 3000 \
    --mcp-server 'github=stdio:npx:-y:@modelcontextprotocol/server-github'
```

### Run it

```bash
python tutorials/mcpjs_agent_loop.py
```

### Expected output

```
--- Turn 1 ---
  tokens: input=523  output=145
  executing JS (89 chars):
    const tools = mcp.listTools("github"); console.log(JSON.stringify(tools.map(t => t.name)))...
  output: ["search_repositories","search_issues","list_commits",...]...
--- Turn 2 ---
  tokens: input=1847  output=198
  executing JS (156 chars):
    const result = await mcp.callTool("github", "search_issues", ...
  output: {"content":[{"type":"text","text":"{...}"],"isError":false}...
--- Turn 3 ---
  tokens: input=3204  output=267
...

=======================================================
TOKEN USAGE SUMMARY  (mcp-js Agent Loop)
=======================================================
  Turn  1: input=   523  output=  145
  Turn  2: input=  1847  output=  198
  Turn  3: input=  3204  output=  267
  Turn  4: input=  4102  output=  356
-------------------------------------------------------
  TOTAL   : input=  9676  output=  966
  GRAND TOTAL TOKENS: 10642
=======================================================
```

## Token Comparison

```
┌─────────────────────────┬──────────────┬──────────────┐
│                         │  MCP Server  │   mcp-js     │
│                         │  Agent Loop  │  Agent Loop  │
├─────────────────────────┼──────────────┼──────────────┤
│ Tool definitions/turn   │  ~4,000 tok  │   ~150 tok   │
│ Tools sent to Claude    │     34       │      1       │
│ Total input tokens      │  ~42,000     │  ~10,000     │
│ Total output tokens     │    ~850      │    ~950      │
│ GRAND TOTAL             │  ~43,000     │  ~11,000     │
│ Token savings           │     —        │   ~75%       │
└─────────────────────────┴──────────────┴──────────────┘
```

The mcp-js approach uses **~75% fewer tokens** because:

1. **Fewer tool definitions per turn** — 1 tool (~150 tokens) vs 34 tools (~4,000 tokens). This compounds across every turn.
2. **Claude discovers tools on-demand** — On the first turn, Claude calls `mcp.listTools()` and gets only the tool *names*. It then calls only the tools it needs.
3. **Tool results stay server-side** — When Claude calls `mcp.callTool()` inside JS, the raw API response never enters the Claude context. Claude only sees what `console.log()` prints.

## How It Works Under the Hood

```
Approach A (MCP Server):
  Claude ←→ [34 tool schemas per turn] ←→ Your Python ←→ MCP Server

Approach B (mcp-js):
  Claude ←→ [1 tool: run_js] ←→ Your Python ←→ mcp-js ←→ MCP Server
                                                  ↑
                                          JS sandbox with
                                         mcp.callTool() API
```

In Approach B, mcp-js acts as a **tool-calling proxy**. The V8 sandbox connects to the GitHub MCP server at startup and exposes it through `globalThis.mcp`. Claude writes JavaScript that:

1. Calls `mcp.listTools("github")` to discover available tools
2. Calls `await mcp.callTool("github", "tool_name", {args})` to invoke them
3. Uses `console.log()` to return only the relevant data back to Claude

This keeps the tool schema overhead out of the Claude context window entirely.

## When to Use Each Approach

| Use Case | Recommendation |
|---|---|
| Quick prototype, few tools | Approach A — simpler setup |
| Many tools (10+) | Approach B — significant token savings |
| Cost-sensitive production | Approach B — ~75% fewer input tokens |
| Untrusted tool servers | Approach A — no code execution needed |
| Chaining multiple MCP servers | Approach B — one `run_js` tool covers all servers |
