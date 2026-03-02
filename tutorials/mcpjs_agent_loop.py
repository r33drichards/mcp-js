#!/usr/bin/env python3
"""
mcp-js Programmatic Tool Calling Agent Loop.

Instead of exposing every GitHub MCP tool to Claude directly, this approach
uses mcp-js as the *only* tool.  Claude writes JavaScript that calls
`mcp.callTool("github", toolName, args)` inside the mcp-js V8 sandbox.

This dramatically reduces the number of tool definitions sent to Claude
(1 tool instead of 30+), cutting input token costs on every turn.

Prerequisites:
    pip install anthropic requests
    export ANTHROPIC_API_KEY=sk-...

    # Start mcp-js with the GitHub MCP server connected:
    docker compose -f docker-compose.single-node.yml up --build -d
    # (see tutorial for full docker-compose config with --mcp-server flag)

    # Or run the binary directly:
    mcp-v8 --stateless --http-port 3000 \
        --mcp-server 'github=stdio:npx:-y:@modelcontextprotocol/server-github' \
        --fetch-header 'host=api.github.com,header=Authorization,value=Bearer <GITHUB_TOKEN>'
"""

import json
import os
import sys
import time

import anthropic
import requests

MCPJS_URL = os.environ.get("MCPJS_URL", "http://localhost:3000")
MODEL = "claude-sonnet-4-20250514"
MAX_TOKENS = 4096
MAX_TURNS = 10

PROMPT = (
    "List the GitHub repos I have committed to in the past 24 hours. "
    "Write JavaScript using the mcp.callTool() API to query the GitHub MCP "
    "server connected as 'github'. The available JS API is:\n"
    "  mcp.servers            // string[] of connected server names\n"
    "  mcp.listTools('github') // list available tools with schemas\n"
    "  await mcp.callTool('github', 'toolName', {arg: 'val'})\n\n"
    "Start by listing the available tools, then use the appropriate ones "
    "to find repos I have committed to recently. Return a concise list."
)

# The single tool definition for mcp-js
MCPJS_TOOL = {
    "name": "run_js",
    "description": (
        "Execute JavaScript in the mcp-js V8 sandbox. The sandbox has access to "
        "external MCP servers via the global `mcp` object:\n"
        "  mcp.servers — string[] of connected server names\n"
        "  mcp.listTools(server?) — list tools with name, description, inputSchema\n"
        "  await mcp.callTool(server, tool, args) — call a tool, returns {content, isError}\n\n"
        "Use console.log() to print output. The code runs in an async context."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "code": {
                "type": "string",
                "description": "JavaScript code to execute. Use console.log() for output.",
            }
        },
        "required": ["code"],
    },
}


def execute_js(code: str) -> str:
    """Submit JS code to the mcp-js /api/exec endpoint and return the output."""
    resp = requests.post(
        f"{MCPJS_URL}/api/exec",
        json={"code": code},
        timeout=60,
    )
    resp.raise_for_status()
    data = resp.json()
    if "error" in data and data["error"]:
        return f"ERROR: {data['error']}\nOutput: {data.get('output', '')}"
    return data.get("output", "")


def run_mcpjs_agent_loop():
    client = anthropic.Anthropic()
    messages = [{"role": "user", "content": PROMPT}]
    tools = [MCPJS_TOOL]

    total_input_tokens = 0
    total_output_tokens = 0
    turn_stats = []

    for turn in range(MAX_TURNS):
        print(f"--- Turn {turn + 1} ---")

        response = client.messages.create(
            model=MODEL,
            max_tokens=MAX_TOKENS,
            tools=tools,
            messages=messages,
        )

        # Track tokens
        inp_tok = response.usage.input_tokens
        out_tok = response.usage.output_tokens
        total_input_tokens += inp_tok
        total_output_tokens += out_tok
        turn_stats.append(
            {"turn": turn + 1, "input_tokens": inp_tok, "output_tokens": out_tok}
        )
        print(f"  tokens: input={inp_tok}  output={out_tok}")

        if response.stop_reason == "end_turn":
            for block in response.content:
                if hasattr(block, "text"):
                    print(f"\n=== Claude's answer ===\n{block.text}")
            break

        # Process tool calls
        assistant_content = response.content
        messages.append({"role": "assistant", "content": assistant_content})

        tool_results = []
        for block in assistant_content:
            if block.type == "tool_use":
                code = block.input.get("code", "")
                print(f"  executing JS ({len(code)} chars):\n    {code[:150]}...")
                output = execute_js(code)
                print(f"  output: {output[:200]}...")
                tool_results.append(
                    {
                        "type": "tool_result",
                        "tool_use_id": block.id,
                        "content": output,
                    }
                )

        messages.append({"role": "user", "content": tool_results})
    else:
        print(f"\nReached max turns ({MAX_TURNS}) without final answer.")

    # --- Print token summary ---
    print("\n" + "=" * 55)
    print("TOKEN USAGE SUMMARY  (mcp-js Agent Loop)")
    print("=" * 55)
    for s in turn_stats:
        print(f"  Turn {s['turn']:>2}: input={s['input_tokens']:>6}  output={s['output_tokens']:>5}")
    print("-" * 55)
    print(f"  TOTAL   : input={total_input_tokens:>6}  output={total_output_tokens:>5}")
    print(f"  GRAND TOTAL TOKENS: {total_input_tokens + total_output_tokens}")
    print("=" * 55)

    return total_input_tokens, total_output_tokens


if __name__ == "__main__":
    run_mcpjs_agent_loop()
