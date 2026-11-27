#!/usr/bin/env bash
# Integration test for programmatic tool calling with minizinc-mcp.
#
# Prerequisites:
#   - mcp-v8 built (server/target/release/server)
#   - python3 with minizinc Python package installed
#   - minizinc system package installed
#
# Usage:
#   ./tutorials/test_programmatic_tool_calling.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER="$ROOT/server/target/release/server"
MINIZINC_MCP_PORT=8000
MCPV8_PORT=3000

cleanup() {
    echo "Cleaning up..."
    [ -n "${MCPV8_PID:-}" ] && kill "$MCPV8_PID" 2>/dev/null || true
    [ -n "${MINIZINC_PID:-}" ] && kill "$MINIZINC_PID" 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT

# ── Clone and start MiniZinc MCP server ────────────────────────────────
echo "Starting MiniZinc MCP server on port $MINIZINC_MCP_PORT..."
TMPDIR=$(mktemp -d)
git clone --quiet https://github.com/r33drichards/minizinc-mcp "$TMPDIR/minizinc-mcp"
cd "$TMPDIR/minizinc-mcp"
pip install -q -r requirements.txt
python3 main.py &>/tmp/minizinc-mcp-test.log &
MINIZINC_PID=$!
sleep 3

# ── Start mcp-v8 ──────────────────────────────────────────────────────
echo "Starting mcp-v8 on port $MCPV8_PORT..."
"$SERVER" --stateless --http-port "$MCPV8_PORT" \
    --mcp-server "minizinc=sse:http://localhost:$MINIZINC_MCP_PORT/sse" \
    &>/tmp/mcp-v8-test.log &
MCPV8_PID=$!
sleep 5

# Verify mcp-v8 connected
if ! grep -q "All MCP servers connected" /tmp/mcp-v8-test.log; then
    echo "FAIL: mcp-v8 did not connect to minizinc MCP server"
    cat /tmp/mcp-v8-test.log
    exit 1
fi
echo "Servers ready."

# ── Run all tests from a single Python script ─────────────────────────
echo ""
echo "=== Running programmatic tool calling tests ==="
echo ""

python3 << 'PYEOF'
import json, urllib.request, time, sys

PORT = 3000
FAILURES = 0

def exec_js(code, wait=3):
    """Submit JS code and return console output."""
    req = urllib.request.Request(
        f"http://localhost:{PORT}/api/exec",
        data=json.dumps({"code": code}).encode(),
        headers={"Content-Type": "application/json"},
    )
    resp = json.loads(urllib.request.urlopen(req).read())
    eid = resp["execution_id"]
    time.sleep(wait)
    out = json.loads(
        urllib.request.urlopen(f"http://localhost:{PORT}/api/executions/{eid}/output").read()
    )
    return out["data"].strip()

def test(name, fn):
    global FAILURES
    try:
        fn()
        print(f"PASS [{name}]")
    except Exception as e:
        print(f"FAIL [{name}]: {e}")
        FAILURES += 1

# ── Test 1: mcp.servers ──────────────────────────────────────────────
def test_servers():
    data = exec_js('console.log(JSON.stringify(mcp.servers))')
    assert data == '["minizinc"]', f"got {data}"
    print(f"  Output: {data}")

test("mcp.servers returns connected servers", test_servers)

# ── Test 2: mcp.listTools ────────────────────────────────────────────
def test_list_tools():
    data = exec_js('console.log(JSON.stringify(mcp.listTools("minizinc").map(t => t.name)))')
    assert "solve_constraint" in data, f"got {data}"
    print(f"  Output: {data}")

test("mcp.listTools returns minizinc tools", test_list_tools)

# ── Test 3: 4-Queens ─────────────────────────────────────────────────
def test_queens():
    code = '''(async () => {
      const result = await mcp.callTool("minizinc", "solve_constraint", {
        problem: {
          model: [
            "int: n = 4;",
            'array[1..n] of var 1..n: queens;',
            'include "alldifferent.mzn";',
            "constraint alldifferent(queens);",
            "constraint alldifferent(i in 1..n)(queens[i] + i);",
            "constraint alldifferent(i in 1..n)(queens[i] - i);",
            "solve satisfy;"
          ].join("\\n")
        }
      });
      const parsed = JSON.parse(result.content[0].text);
      console.log(JSON.stringify({
        status: parsed.status,
        queens: parsed.solutions[0].variables.queens
      }));
    })()'''
    data = exec_js(code, wait=5)
    parsed = json.loads(data)
    assert parsed["status"] == "SATISFIED", f"status={parsed['status']}"
    assert len(parsed["queens"]) == 4, f"queens={parsed['queens']}"
    print(f"  Output: {data}")

test("4-Queens solved via mcp.callTool", test_queens)

# ── Test 4: Knapsack optimization ────────────────────────────────────
def test_knapsack():
    code = '''(async () => {
      const result = await mcp.callTool("minizinc", "solve_constraint", {
        problem: {
          model: [
            "int: n = 5;",
            "array[1..n] of int: weight = [2, 3, 4, 5, 9];",
            "array[1..n] of int: value = [3, 4, 8, 8, 10];",
            "int: capacity = 20;",
            "array[1..n] of var 0..1: x;",
            "constraint sum(i in 1..n)(weight[i] * x[i]) <= capacity;",
            "solve maximize sum(i in 1..n)(value[i] * x[i]);"
          ].join("\\n")
        }
      });
      const parsed = JSON.parse(result.content[0].text);
      console.log(JSON.stringify({
        status: parsed.status,
        objective: parsed.solutions[0].objective,
        items: parsed.solutions[0].variables.x
      }));
    })()'''
    data = exec_js(code, wait=5)
    parsed = json.loads(data)
    assert parsed["status"] == "OPTIMAL_SOLUTION", f"status={parsed['status']}"
    assert parsed["objective"] == 29.0, f"objective={parsed['objective']}"
    print(f"  Output: {data}")

test("Knapsack optimization solved via mcp.callTool", test_knapsack)

# ── Test 5: Graph coloring ───────────────────────────────────────────
def test_graph_coloring():
    code = '''(async () => {
      const result = await mcp.callTool("minizinc", "solve_constraint", {
        problem: {
          model: [
            "int: n = 5;",
            "int: c = 3;",
            "array[1..n] of var 1..c: color;",
            "constraint color[1] != color[2];",
            "constraint color[1] != color[3];",
            "constraint color[2] != color[3];",
            "constraint color[2] != color[4];",
            "constraint color[3] != color[4];",
            "constraint color[3] != color[5];",
            "constraint color[4] != color[5];",
            "solve satisfy;"
          ].join("\\n")
        }
      });
      const parsed = JSON.parse(result.content[0].text);
      console.log(JSON.stringify({
        status: parsed.status,
        colors: parsed.solutions[0].variables.color
      }));
    })()'''
    data = exec_js(code, wait=5)
    parsed = json.loads(data)
    assert parsed["status"] == "SATISFIED", f"status={parsed['status']}"
    assert len(parsed["colors"]) == 5, f"colors={parsed['colors']}"
    print(f"  Output: {data}")

test("Graph coloring solved via mcp.callTool", test_graph_coloring)

# ── Summary ──────────────────────────────────────────────────────────
print()
if FAILURES:
    print(f"=== {FAILURES} test(s) FAILED ===")
    sys.exit(1)
else:
    print("=== All tests passed ===")
PYEOF
