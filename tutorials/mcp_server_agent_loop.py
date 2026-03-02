#!/usr/bin/env python3
"""
MCP Server Agent Loop — GitHub MCP via stdio transport.

Spins up @modelcontextprotocol/server-github as a child process, discovers
its tools through the MCP protocol, then runs a Claude agent loop that calls
those tools to answer: "What repos have I committed to in the past 24 hours?"

Prints per-turn and total token usage at the end.

Requirements:
    pip install anthropic mcp
    npm install -g @modelcontextprotocol/server-github   # or use npx
    export ANTHROPIC_API_KEY=sk-...
    export GITHUB_PERSONAL_ACCESS_TOKEN=ghp_...
"""

import asyncio
import json
import os
import sys

import anthropic
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

PROMPT = (
    "List the GitHub repos I have committed to in the past 24 hours. "
    "Use the available GitHub tools to find this information. "
    "Return a concise list of repo full names."
)
MODEL = "claude-sonnet-4-20250514"
MAX_TOKENS = 4096
MAX_TURNS = 10


def _format_tool_result(result) -> str:
    """Extract text from an MCP CallToolResult."""
    parts = []
    for block in result.content:
        if hasattr(block, "text"):
            parts.append(block.text)
    return "\n".join(parts) if parts else str(result)


async def run_mcp_agent_loop():
    github_token = os.environ.get("GITHUB_PERSONAL_ACCESS_TOKEN")
    if not github_token:
        print("ERROR: Set GITHUB_PERSONAL_ACCESS_TOKEN env var", file=sys.stderr)
        sys.exit(1)

    # --- 1. Connect to the GitHub MCP server via stdio -------------------
    server_params = StdioServerParameters(
        command="npx",
        args=["-y", "@modelcontextprotocol/server-github"],
        env={
            **os.environ,
            "GITHUB_PERSONAL_ACCESS_TOKEN": github_token,
        },
    )

    total_input_tokens = 0
    total_output_tokens = 0
    turn_stats = []

    async with stdio_client(server_params) as (read_stream, write_stream):
        async with ClientSession(read_stream, write_stream) as session:
            await session.initialize()

            # --- 2. Discover tools --------------------------------------
            tools_result = await session.list_tools()
            # Convert MCP tool schemas to Anthropic tool format
            anthropic_tools = []
            for tool in tools_result.tools:
                anthropic_tools.append(
                    {
                        "name": tool.name,
                        "description": tool.description or "",
                        "input_schema": tool.inputSchema,
                    }
                )
            print(f"Discovered {len(anthropic_tools)} tools from GitHub MCP server\n")

            # --- 3. Agent loop ------------------------------------------
            client = anthropic.Anthropic()
            messages = [{"role": "user", "content": PROMPT}]

            for turn in range(MAX_TURNS):
                print(f"--- Turn {turn + 1} ---")

                response = client.messages.create(
                    model=MODEL,
                    max_tokens=MAX_TOKENS,
                    tools=anthropic_tools,
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

                # If Claude is done (no more tool calls), break
                if response.stop_reason == "end_turn":
                    # Print final text
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
                        print(f"  calling tool: {block.name}({json.dumps(block.input)[:120]}...)")
                        mcp_result = await session.call_tool(
                            block.name, arguments=block.input
                        )
                        tool_results.append(
                            {
                                "type": "tool_result",
                                "tool_use_id": block.id,
                                "content": _format_tool_result(mcp_result),
                            }
                        )

                messages.append({"role": "user", "content": tool_results})

            else:
                print(f"\nReached max turns ({MAX_TURNS}) without final answer.")

    # --- 4. Print token summary -----------------------------------------
    print("\n" + "=" * 55)
    print("TOKEN USAGE SUMMARY  (MCP Server Agent Loop)")
    print("=" * 55)
    for s in turn_stats:
        print(f"  Turn {s['turn']:>2}: input={s['input_tokens']:>6}  output={s['output_tokens']:>5}")
    print("-" * 55)
    print(f"  TOTAL   : input={total_input_tokens:>6}  output={total_output_tokens:>5}")
    print(f"  GRAND TOTAL TOKENS: {total_input_tokens + total_output_tokens}")
    print("=" * 55)

    return total_input_tokens, total_output_tokens


if __name__ == "__main__":
    asyncio.run(run_mcp_agent_loop())
