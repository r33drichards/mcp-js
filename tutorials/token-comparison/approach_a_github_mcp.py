#!/usr/bin/env python3
"""
Approach A: PydanticAI agent connected directly to GitHub MCP server.

All 26 GitHub MCP tools are exposed to the model on every turn.

Requirements:
    pip install "pydantic-ai[bedrock,mcp]"
    export GITHUB_PERSONAL_ACCESS_TOKEN=...
    export AWS_REGION=us-east-1
"""

import asyncio
import os
import sys

from pydantic_ai import Agent
from pydantic_ai.mcp import MCPServerStdio

PROMPT = (
    "List the GitHub repos user r33drichards has committed to in the past 24 hours. "
    "Use the available GitHub tools to search for recent activity. "
    "Return a concise list of repo full names."
)
MODEL = "bedrock:us.anthropic.claude-sonnet-4-20250514-v1:0"


async def main():
    token = os.environ.get("GITHUB_PERSONAL_ACCESS_TOKEN")
    if not token:
        print("ERROR: Set GITHUB_PERSONAL_ACCESS_TOKEN", file=sys.stderr)
        sys.exit(1)

    server = MCPServerStdio(
        "npx",
        args=["-y", "@modelcontextprotocol/server-github"],
        env={**os.environ, "GITHUB_PERSONAL_ACCESS_TOKEN": token},
    )

    agent = Agent(MODEL, toolsets=[server])

    async with agent:
        result = await agent.run(PROMPT)

    print("\n=== Claude's answer ===")
    print(result.output)

    usage = result.usage()
    print(f"\n{'='*55}")
    print("TOKEN USAGE  (Approach A: GitHub MCP direct)")
    print(f"{'='*55}")
    print(f"  Input tokens:    {usage.input_tokens}")
    print(f"  Output tokens:   {usage.output_tokens}")
    print(f"  Total tokens:    {usage.total_tokens}")
    print(f"  Requests:        {usage.requests}")
    print(f"{'='*55}")

    return usage


if __name__ == "__main__":
    asyncio.run(main())
