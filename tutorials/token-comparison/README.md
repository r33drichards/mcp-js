# Token Usage Case Study: MCP Tool Calling Approaches

Compares three approaches for building an AI agent that answers *"What repos has r33drichards committed to in the past 24 hours?"* using GitHub data.

All approaches use **PydanticAI** with **Claude Sonnet 4 on AWS Bedrock**.

## Results

Real token usage from a single run (March 2, 2026):

```
                          Approach A        Approach B        Approach C
                         GitHub MCP      mcp-js proxy         gh CLI
                         (26 tools)        (1 tool)          (1 tool)
 ──────────────────────────────────────────────────────────────────────
  Input tokens             105,708          42,244            64,488
  Output tokens                546           3,103             1,908
  Total tokens             106,254          45,347            66,396
  API requests                   4               8                18
 ──────────────────────────────────────────────────────────────────────
  vs. Approach A                —             -57%              -37%
```

## The Three Approaches

### Approach A: GitHub MCP Server Direct

PydanticAI connects to the GitHub MCP server via stdio. All 26 tool definitions are sent to the model on every turn.

```python
server = MCPServerStdio("npx", args=["-y", "@modelcontextprotocol/server-github"], ...)
agent = Agent("bedrock:us.anthropic.claude-sonnet-4-20250514-v1:0", toolsets=[server])
result = await agent.run(prompt)
```

**Why the tokens are high:** Every API request includes all 26 tool schemas (~4,000 tokens of definitions per turn). Tool results from GitHub (issue/PR search results) are also large — individual turns hit 50K-100K+ input tokens.

### Approach B: mcp-js Programmatic Tool Calling

PydanticAI connects to mcp-js via Streamable HTTP MCP. The model sees only the tools mcp-js exposes (primarily `run_js`). Claude writes JavaScript that calls `mcp.callTool()` inside the V8 sandbox.

```python
server = MCPServerStreamableHTTP("http://localhost:3000/mcp")
agent = Agent("bedrock:us.anthropic.claude-sonnet-4-20250514-v1:0", toolsets=[server])
result = await agent.run(prompt)
```

**Why this saves tokens:**
1. **Fewer tool definitions per turn** — 1 tool vs 26
2. **Tool results stay server-side** — When Claude calls `mcp.callTool()` inside JS, the raw GitHub API response never enters the Claude context. Claude only sees what `console.log()` outputs.
3. **Claude controls data extraction** — The JS code can filter/summarize results before returning them

### Approach C: gh CLI Tool

PydanticAI with a single custom tool that runs `gh` CLI commands. No MCP at all.

```python
agent = Agent("bedrock:us.anthropic.claude-sonnet-4-20250514-v1:0")

@agent.tool_plain
def run_gh(command: str) -> str:
    result = subprocess.run(["gh"] + command.split(), capture_output=True, text=True)
    return result.stdout[:4000]

result = await agent.run(prompt)
```

**Why more tokens than B:** The `gh` CLI returns raw JSON/text that all goes into the context. With mcp-js, Claude writes JS that extracts only the relevant fields.

## How It Works

```
Approach A:
  Claude ←→ [26 tool schemas] ←→ PydanticAI ←→ GitHub MCP Server

Approach B:
  Claude ←→ [1 tool: run_js] ←→ PydanticAI ←→ mcp-js ←→ GitHub MCP Server
                                                  ↑
                                          V8 sandbox with
                                         mcp.callTool() API

Approach C:
  Claude ←→ [1 tool: run_gh] ←→ PydanticAI ←→ subprocess ←→ gh CLI
```

## Running the Scripts

### Prerequisites

```bash
# uv handles Python dependencies automatically via inline script metadata
# https://docs.astral.sh/uv/guides/scripts/
export AWS_REGION=us-east-1
export GITHUB_PERSONAL_ACCESS_TOKEN=$(gh auth token)
```

### Approach A

```bash
uv run tutorials/token-comparison/approach_a_github_mcp.py
```

### Approach B

```bash
# Start mcp-js with GitHub MCP server connected
mcp-v8 --stateless --http-port 3000 \
    --mcp-server 'github=stdio:npx:-y:@modelcontextprotocol/server-github'

# In another terminal
uv run tutorials/token-comparison/approach_b_mcpjs.py
```

### Approach C

```bash
gh auth login  # must be authenticated
uv run tutorials/token-comparison/approach_c_gh_cli.py
```

## When to Use Each Approach

| Use Case | Recommendation |
|---|---|
| Quick prototype, few tools | Approach A or C — simpler setup |
| Many tools (10+) | Approach B — significant token savings |
| Cost-sensitive production | Approach B — 57% fewer tokens |
| No infrastructure to run | Approach C — just needs `gh` CLI |
| Chaining multiple MCP servers | Approach B — one mcp-js covers all servers |
