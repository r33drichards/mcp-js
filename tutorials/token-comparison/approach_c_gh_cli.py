#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "pydantic-ai",
# ]
# ///
"""
Approach C: PydanticAI agent with a single gh CLI tool.

One tool that runs `gh` commands. No MCP overhead at all.

Usage:
    export AWS_REGION=us-east-1
    gh auth login  # must be authenticated
    uv run tutorials/token-comparison/approach_c_gh_cli.py
"""

import asyncio
import subprocess

from pydantic_ai import Agent

PROMPT = (
    "List the GitHub repos user r33drichards has committed to in the past 24 hours. "
    "Use the gh CLI tool to find this information. "
    "Return a concise list of repo full names."
)
MODEL = "bedrock:us.anthropic.claude-sonnet-4-20250514-v1:0"

agent = Agent(MODEL)


@agent.tool_plain
def run_gh(command: str) -> str:
    """Run a gh CLI command and return stdout. The `gh` prefix is added automatically.
    Examples: 'api /user', 'search commits --author=@me --committed-date=>2024-03-01',
    'api /users/{username}/events --paginate --jq .[].repo.name'"""
    try:
        result = subprocess.run(
            ["gh"] + command.split(),
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode != 0:
            return f"ERROR (exit {result.returncode}): {result.stderr[:500]}"
        return result.stdout[:4000]  # cap output to avoid token bloat
    except subprocess.TimeoutExpired:
        return "ERROR: command timed out after 30s"
    except Exception as e:
        return f"ERROR: {e}"


async def main():
    result = await agent.run(PROMPT)

    print("\n=== Claude's answer ===")
    print(result.output)

    usage = result.usage()
    print(f"\n{'='*55}")
    print("TOKEN USAGE  (Approach C: gh CLI tool)")
    print(f"{'='*55}")
    print(f"  Input tokens:    {usage.input_tokens}")
    print(f"  Output tokens:   {usage.output_tokens}")
    print(f"  Total tokens:    {usage.total_tokens}")
    print(f"  Requests:        {usage.requests}")
    print(f"{'='*55}")

    return usage


if __name__ == "__main__":
    asyncio.run(main())
