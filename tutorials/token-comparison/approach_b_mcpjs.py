#!/usr/bin/env python3
"""
Approach B: PydanticAI agent connected to mcp-js via Streamable HTTP.

Only the tools exposed by mcp-js (run_js) are sent to the model.
Claude writes JavaScript that calls mcp.callTool() inside the V8 sandbox.

Requirements:
    pip install "pydantic-ai[bedrock,mcp]"
    export AWS_REGION=us-east-1

    # Start mcp-js with the GitHub MCP server connected:
    mcp-v8 --stateless --http-port 3000 \
        --mcp-server 'github=stdio:npx:-y:@modelcontextprotocol/server-github'
"""

import asyncio
import os

from pydantic_ai import Agent
from pydantic_ai.mcp import MCPServerStreamableHTTP

MCPJS_URL = os.environ.get("MCPJS_URL", "http://localhost:3000/mcp")

PROMPT = (
    "List the GitHub repos user r33drichards has committed to in the past 24 hours. "
    "Write JavaScript using the mcp.callTool() API to query the GitHub MCP "
    "server connected as 'github'. The available JS API is:\n"
    "  mcp.servers            // string[] of connected server names\n"
    "  mcp.listTools('github') // list available tools with schemas\n"
    "  await mcp.callTool('github', 'toolName', {arg: 'val'})\n\n"
    "Start by listing the available tools, then use the appropriate ones "
    "to find repos r33drichards has committed to recently. Return a concise list."
)
MODEL = "bedrock:us.anthropic.claude-sonnet-4-20250514-v1:0"


async def main():
    server = MCPServerStreamableHTTP(MCPJS_URL)
    agent = Agent(MODEL, toolsets=[server])

    async with agent:
        result = await agent.run(PROMPT)

    print("\n=== Claude's answer ===")
    print(result.output)

    usage = result.usage()
    print(f"\n{'='*55}")
    print("TOKEN USAGE  (Approach B: mcp-js proxy)")
    print(f"{'='*55}")
    print(f"  Input tokens:    {usage.input_tokens}")
    print(f"  Output tokens:   {usage.output_tokens}")
    print(f"  Total tokens:    {usage.total_tokens}")
    print(f"  Requests:        {usage.requests}")
    print(f"{'='*55}")

    return usage


if __name__ == "__main__":
    asyncio.run(main())
