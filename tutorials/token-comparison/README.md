# Token Usage Case Study: MCP Tool Calling Approaches

Compares three approaches for building an AI agent that answers *"What repos has r33drichards committed to in the past 24 hours?"* using GitHub data.

All approaches use **PydanticAI** with **Claude Sonnet 4 on AWS Bedrock**.

## Results

Average token usage over 10 runs (March 2, 2026):

```
                          Approach A        Approach B        Approach C
                         GitHub MCP      mcp-js proxy         gh CLI
                         (26 tools)        (1 tool)          (1 tool)
 ──────────────────────────────────────────────────────────────────────
  Avg input tokens         121,450         114,763            22,981
  Avg output tokens            606           3,063             1,038
  Avg total tokens         122,056         117,826            24,019
  Avg API requests             3.9             5.9               9.2
 ──────────────────────────────────────────────────────────────────────
  vs. Approach A                —              -3%              -80%
```

**Key finding:** Approach C (gh CLI) is the clear winner at **80% fewer tokens** than
direct MCP. Approach B (mcp-js) shows high variance (38K–197K per run) depending on
how much data Claude's JS code pulls back via `console.log()` — the server-side
filtering advantage is real but inconsistent.

![Average token usage by approach](graph_avg_tokens.png)

![Token usage distribution across runs](graph_distribution.png)

![Token usage per run](graph_per_run.png)

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

**Theoretical token advantages:**
1. **Fewer tool definitions per turn** — 1 tool vs 26
2. **Tool results stay server-side** — When Claude calls `mcp.callTool()` inside JS, the raw GitHub API response never enters the Claude context. Claude only sees what `console.log()` outputs.
3. **Claude controls data extraction** — The JS code can filter/summarize results before returning them

**In practice:** High variance (38K–197K per run). The savings depend on how well Claude's generated JS filters the data. Sometimes it `console.log()`s large objects, negating the server-side advantage.

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

**Why the fewest tokens:** The `gh` CLI returns raw JSON/text, but the `--jq` flag and API pagination give Claude fine-grained control over what data enters the context. Combined with only 1 tool definition per turn, this approach consistently uses 80% fewer tokens than Approach A.

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

## Running the Benchmark

To reproduce these results (runs all three approaches N times):

```bash
# Ensure mcp-js is running for Approach B (see above)
uv run tutorials/token-comparison/run_benchmark.py --runs 10
```

## When to Use Each Approach

| Use Case | Recommendation |
|---|---|
| Quick prototype, few tools | Approach A — simplest MCP setup |
| Cost-sensitive production | Approach C — 80% fewer tokens, no infra needed |
| Chaining multiple MCP servers | Approach B — one mcp-js covers all servers |
| Custom data filtering logic | Approach B — JS sandbox controls what enters context |
| No infrastructure to run | Approach C — just needs `gh` CLI |
