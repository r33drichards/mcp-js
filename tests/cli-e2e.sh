
set -euo pipefail

CLI_BIN="${CLI_BIN:-./target/release/mcp-v8-cli}"
SERVER_URL="${SERVER_URL:-http://localhost:3000}"

PASS=0
FAIL=0
FAILED_TESTS=()


pass() { echo "✅ PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "❌ FAIL: $1"; FAIL=$((FAIL + 1)); FAILED_TESTS+=("$1"); }

wait_for_completion() {
    local id="$1"
    local max="${2:-30}"
    local i=0
    local resp status
    while [ $i -lt "$max" ]; do
        resp=$("$CLI_BIN" --url "$SERVER_URL" --json executions get "$id" 2>/dev/null || true)
        status=$(echo "$resp" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('status',''))" 2>/dev/null || true)
        case "$status" in
            Completed|completed) echo "$resp"; return 0 ;;
            Failed|failed|TimedOut|timed_out)
                echo "$resp" >&2
                return 1
                ;;
        esac
        sleep 1
        i=$((i + 1))
    done
    echo "Timed out waiting for $id" >&2
    return 1
}


echo "=== CLI E2E Tests ==="
echo "CLI_BIN   : $CLI_BIN"
echo "SERVER_URL: $SERVER_URL"
echo ""

if [ ! -x "$CLI_BIN" ]; then
    echo "ERROR: CLI binary not found or not executable: $CLI_BIN"
    exit 1
fi

echo "--- Test 1: exec (console.log(42)) ---"

T1_OUT=$("$CLI_BIN" --url "$SERVER_URL" exec 'console.log(42)' 2>&1)
echo "$T1_OUT"

EXEC_ID=$(echo "$T1_OUT" | grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' | head -1 || true)

if [ -z "$EXEC_ID" ]; then
    fail "Test 1: exec — could not capture execution_id from output"
else
    pass "Test 1: exec — got execution_id: $EXEC_ID"
fi

echo ""
echo "--- Test 2: executions list ---"

LIST_OUT=$("$CLI_BIN" --url "$SERVER_URL" executions list 2>&1)
LIST_STATUS=$?
echo "$LIST_OUT"

if [ $LIST_STATUS -ne 0 ]; then
    fail "Test 2: executions list — exited with status $LIST_STATUS"
elif [ -z "$LIST_OUT" ]; then
    fail "Test 2: executions list — produced no output"
else
    pass "Test 2: executions list — exited 0 and printed output"
fi

echo ""
echo "--- Test 3: executions get <id> ---"

if [ -z "$EXEC_ID" ]; then
    fail "Test 3: executions get — skipped (no execution_id from Test 1)"
else
    GET_OUT=$("$CLI_BIN" --url "$SERVER_URL" executions get "$EXEC_ID" 2>&1)
    echo "$GET_OUT"

    if echo "$GET_OUT" | grep -qi "status"; then
        pass "Test 3: executions get — status field present"
    else
        fail "Test 3: executions get — status field not found in output"
    fi
fi

echo ""
echo "--- Test 4: executions output <id> (should contain '42') ---"

if [ -z "$EXEC_ID" ]; then
    fail "Test 4: executions output — skipped (no execution_id from Test 1)"
else
        wait_for_completion "$EXEC_ID" 30 >/dev/null 2>&1 || true

    OUTPUT_OUT=$("$CLI_BIN" --url "$SERVER_URL" executions output "$EXEC_ID" 2>&1)
    echo "$OUTPUT_OUT"

    if echo "$OUTPUT_OUT" | grep -q "42"; then
        pass "Test 4: executions output — output contains '42'"
    else
        fail "Test 4: executions output — '42' not found in output"
    fi
fi

echo ""
echo "--- Test 5: exec + poll until Completed, assert output contains '4' ---"

T5_OUT=$("$CLI_BIN" --url "$SERVER_URL" exec 'console.log(2+2)' 2>&1)
echo "$T5_OUT"
T5_ID=$(echo "$T5_OUT" | grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' | head -1 || true)

if [ -z "$T5_ID" ]; then
    fail "Test 5: exec+poll — could not capture execution_id"
else
    FINAL_RESP=$(wait_for_completion "$T5_ID" 30 2>/dev/null || true)
    FINAL_STATUS=$(echo "$FINAL_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('status',''))" 2>/dev/null || true)

    if [ "$FINAL_STATUS" != "Completed" ] && [ "$FINAL_STATUS" != "completed" ]; then
        fail "Test 5: exec+poll — execution did not reach Completed (got: '$FINAL_STATUS')"
    else
                T5_OUTPUT=$("$CLI_BIN" --url "$SERVER_URL" executions output "$T5_ID" 2>&1)
        echo "Output: $T5_OUTPUT"
        if echo "$T5_OUTPUT" | grep -q "4"; then
            pass "Test 5: exec+poll — status=Completed and output contains '4'"
        else
            fail "Test 5: exec+poll — '4' not found in output (got: '$T5_OUTPUT')"
        fi
    fi
fi

echo ""
echo "--- Test 6: exec --json flag produces valid JSON ---"

T6_OUT=$("$CLI_BIN" --url "$SERVER_URL" --json exec 'console.log("json-test")' 2>&1)
echo "$T6_OUT"

if echo "$T6_OUT" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    pass "Test 6: exec --json — output is valid JSON"
else
    fail "Test 6: exec --json — output is NOT valid JSON"
fi

echo ""
echo "--- Test 7: exec (infinite loop) + cancel ---"

T7_OUT=$("$CLI_BIN" --url "$SERVER_URL" exec 'while(true){}' 2>&1)
echo "$T7_OUT"
T7_ID=$(echo "$T7_OUT" | grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' | head -1 || true)

if [ -z "$T7_ID" ]; then
    fail "Test 7: cancel — could not capture execution_id"
else
    CANCEL_OUT=$("$CLI_BIN" --url "$SERVER_URL" executions cancel "$T7_ID" 2>&1)
    CANCEL_STATUS=$?
    echo "$CANCEL_OUT"

    if [ $CANCEL_STATUS -eq 0 ]; then
        pass "Test 7: executions cancel — exited 0"
    else
        fail "Test 7: executions cancel — exited with status $CANCEL_STATUS"
    fi
fi

echo ""
echo "==============================="
echo "Results: $PASS passed, $FAIL failed"
if [ ${#FAILED_TESTS[@]} -gt 0 ]; then
    echo "Failed tests:"
    for t in "${FAILED_TESTS[@]}"; do
        echo "  - $t"
    done
fi
echo "==============================="

[ $FAIL -eq 0 ]
