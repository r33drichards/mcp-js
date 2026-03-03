#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "pydantic-ai[mcp]",
# ]
# ///
"""
Constraint-solving agent using mcp-v8 + MiniZinc MCP server.

The agent connects to mcp-v8 via Streamable HTTP. mcp-v8 proxies the MiniZinc
MCP server, exposing it through a V8 sandbox. Claude writes JavaScript that
calls mcp.callTool("minizinc", "solve_constraint", ...) to solve each problem.

Usage:
    # Start the MiniZinc MCP server:
    git clone https://github.com/r33drichards/minizinc-mcp
    cd minizinc-mcp && pip install -r requirements.txt && python main.py

    # Start mcp-v8 with MiniZinc connected:
    mcp-v8 --stateless --http-port 3000 \
        --mcp-server 'minizinc=sse:http://localhost:8000/sse'

    # Run the agent:
    uv run tutorials/solve_with_agent.py
"""

import asyncio
import os

from pydantic_ai import Agent
from pydantic_ai.mcp import MCPServerStreamableHTTP

MCPJS_URL = os.environ.get("MCPJS_URL", "http://localhost:3000/mcp")
MODEL = os.environ.get("MODEL", "anthropic:claude-sonnet-4-6")

SYSTEM_PROMPT = (
    "You are a constraint-solving assistant with access to run_js, "
    "which executes JavaScript in a V8 sandbox connected to the MiniZinc MCP server:\n\n"
    "  mcp.servers                     // string[] of connected server names\n"
    "  mcp.listTools('minizinc')       // list available tools with schemas\n"
    "  await mcp.callTool('minizinc', 'solve_constraint', { problem: { model: '...' } })\n\n"
    "Write JavaScript to call the MiniZinc solver, then report the solution clearly."
)

PROBLEMS = [
    (
        "N-Queens (n=4)",
        (
            "Solve the 4-Queens problem: place 4 queens on a 4×4 chessboard "
            "so no two queens threaten each other. Use the MiniZinc solver and "
            "report which row each queen is placed in."
        ),
    ),
    (
        "Knapsack Optimization",
        (
            "Solve a 0/1 knapsack problem: 5 items with weights [2,3,4,5,9] and "
            "values [3,4,8,8,10], capacity=20. Maximize total value. Use MiniZinc "
            "and report which items to select and the total value achieved."
        ),
    ),
    (
        "Graph Coloring",
        (
            "Color a 5-node graph with at most 3 colors so no adjacent nodes share "
            "a color. Edges: (1,2),(1,3),(2,3),(2,4),(3,4),(3,5),(4,5). Use MiniZinc "
            "and report the color assigned to each node."
        ),
    ),
]


async def main():
    server = MCPServerStreamableHTTP(MCPJS_URL)
    agent = Agent(MODEL, system_prompt=SYSTEM_PROMPT, toolsets=[server])

    async with agent:
        for label, prompt in PROBLEMS:
            print(f"\n{'='*60}")
            print(f"Problem: {label}")
            print(f"{'='*60}")
            result = await agent.run(prompt)
            print(result.output)


if __name__ == "__main__":
    asyncio.run(main())
