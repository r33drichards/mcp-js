#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Benchmark runner: executes all three approaches N times via subprocess and reports averages.

Usage:
    export AWS_REGION=us-east-1
    export GITHUB_PERSONAL_ACCESS_TOKEN=$(gh auth token)
    # Ensure mcp-js is running on localhost:3000 for Approach B
    uv run tutorials/token-comparison/run_benchmark.py [--runs N]
"""

import json
import os
import re
import subprocess
import sys

RUNS = int(sys.argv[sys.argv.index("--runs") + 1]) if "--runs" in sys.argv else 10

SCRIPTS = {
    "A": "tutorials/token-comparison/approach_a_github_mcp.py",
    "B": "tutorials/token-comparison/approach_b_mcpjs.py",
    "C": "tutorials/token-comparison/approach_c_gh_cli.py",
}

LABELS = {
    "A": "GitHub MCP (26 tools)",
    "B": "mcp-js proxy (1 tool)",
    "C": "gh CLI (1 tool)",
}


def parse_output(output: str) -> dict | None:
    """Extract token usage from script output."""
    m_in = re.search(r"Input tokens:\s+(\d+)", output)
    m_out = re.search(r"Output tokens:\s+(\d+)", output)
    m_total = re.search(r"Total tokens:\s+(\d+)", output)
    m_req = re.search(r"Requests:\s+(\d+)", output)
    if m_in and m_out and m_total and m_req:
        return {
            "input": int(m_in.group(1)),
            "output": int(m_out.group(1)),
            "total": int(m_total.group(1)),
            "requests": int(m_req.group(1)),
        }
    return None


def run_script(script: str, timeout: int = 180) -> dict | None:
    """Run a script via uv and parse its token output."""
    try:
        result = subprocess.run(
            ["uv", "run", script],
            capture_output=True, text=True, timeout=timeout,
            env=os.environ,
        )
        if result.returncode != 0:
            print(f"    FAILED (exit {result.returncode}): {result.stderr[:200]}")
            return None
        return parse_output(result.stdout)
    except subprocess.TimeoutExpired:
        print(f"    TIMEOUT after {timeout}s")
        return None
    except Exception as e:
        print(f"    ERROR: {e}")
        return None


def main():
    results: dict[str, list[dict]] = {k: [] for k in SCRIPTS}

    for i in range(1, RUNS + 1):
        for key, script in SCRIPTS.items():
            print(f"Run {i:>2}/{RUNS}  Approach {key} ({LABELS[key]})...", end="", flush=True)
            r = run_script(script)
            if r:
                results[key].append(r)
                print(f"  in={r['input']:,}  out={r['output']:,}  total={r['total']:,}  reqs={r['requests']}")
            else:
                print()

    # Compute averages
    def avg(runs: list[dict], field: str) -> float:
        return sum(r[field] for r in runs) / len(runs) if runs else 0

    print(f"\n{'='*78}")
    print(f"AVERAGED RESULTS OVER {RUNS} RUNS")
    print(f"{'='*78}")
    print(f"{'':>25s}  {'Approach A':>14s}  {'Approach B':>14s}  {'Approach C':>14s}")
    print(f"{'':>25s}  {'GitHub MCP':>14s}  {'mcp-js':>14s}  {'gh CLI':>14s}")
    print(f"{'':>25s}  {'(26 tools)':>14s}  {'(1 tool)':>14s}  {'(1 tool)':>14s}")
    print(f"{'-'*78}")

    avgs = {}
    for key in ["A", "B", "C"]:
        avgs[key] = {
            "input": avg(results[key], "input"),
            "output": avg(results[key], "output"),
            "total": avg(results[key], "total"),
            "requests": avg(results[key], "requests"),
            "n": len(results[key]),
        }

    for label, field, fmt in [
        ("Avg input tokens", "input", ",.0f"),
        ("Avg output tokens", "output", ",.0f"),
        ("Avg total tokens", "total", ",.0f"),
        ("Avg API requests", "requests", ",.1f"),
        ("Successful runs", "n", ",.0f"),
    ]:
        va = avgs["A"][field]
        vb = avgs["B"][field]
        vc = avgs["C"][field]
        print(f"  {label:>23s}  {va:>14{fmt}}  {vb:>14{fmt}}  {vc:>14{fmt}}")

    print(f"{'-'*78}")

    if avgs["A"]["total"] > 0:
        if avgs["B"]["total"] > 0:
            pct_b = (1 - avgs["B"]["total"] / avgs["A"]["total"]) * 100
            print(f"  {'B vs A savings':>23s}  {'':>14s}  {pct_b:>13.0f}%  {'':>14s}")
        if avgs["C"]["total"] > 0:
            pct_c = (1 - avgs["C"]["total"] / avgs["A"]["total"]) * 100
            print(f"  {'C vs A savings':>23s}  {'':>14s}  {'':>14s}  {pct_c:>13.0f}%")

    print(f"{'='*78}")

    # Dump raw data
    raw = {}
    for key in ["A", "B", "C"]:
        raw[key] = {"label": LABELS[key], "runs": results[key], "avg": avgs[key]}
    print(f"\nRaw JSON:\n{json.dumps(raw, indent=2)}")


if __name__ == "__main__":
    main()
