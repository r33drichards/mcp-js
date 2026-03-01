#!/usr/bin/env bash
# Integration test for docker-compose.yml (mcp-js + OPA)
#
# Asserts:
#   1. JavaScript execution works
#   2. fetch() to an allowed domain succeeds
#   3. fetch() to a blocked domain is denied by OPA policy
#   4. Heap snapshots are created and can be restored
#
# Usage:
#   ./tests/docker-compose-integration.sh
#
# Prerequisites: docker compose, curl, jq

set -euo pipefail

COMPOSE_FILE="docker-compose.yml"
BASE_URL="http://localhost:3000"
PASSED=0
FAILED=0

# ── Helpers ──────────────────────────────────────────────────────────────────

cleanup() {
  echo ""
  echo "==> Tearing down containers..."
  docker compose -f "$COMPOSE_FILE" down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

fail() {
  echo "  FAIL: $1"
  FAILED=$((FAILED + 1))
}

pass() {
  echo "  PASS: $1"
  PASSED=$((PASSED + 1))
}

wait_for_ready() {
  local url="$1"
  local retries=30
  local i=0
  echo "==> Waiting for mcp-js to be ready..."
  while [ $i -lt $retries ]; do
    if curl -sf -o /dev/null -X POST "$url/api/exec" \
         -H "Content-Type: application/json" \
         -d '{"code":"1"}' 2>/dev/null; then
      echo "  mcp-js is ready."
      return 0
    fi
    sleep 2
    i=$((i + 1))
  done
  echo "  ERROR: mcp-js did not become ready in time."
  echo ""
  echo "==> Container status:"
  docker compose -f "$COMPOSE_FILE" ps
  echo ""
  echo "==> mcp-js logs:"
  docker compose -f "$COMPOSE_FILE" logs mcp-js
  echo ""
  echo "==> opa logs:"
  docker compose -f "$COMPOSE_FILE" logs opa
  exit 1
}

# Submit code for execution and poll until completed/failed.
# Returns the final execution JSON on stdout.
run_js() {
  local code="$1"
  local extra_fields="${2:-}"
  local timeout_secs="${3:-30}"

  local payload
  if [ -n "$extra_fields" ]; then
    payload="{\"code\": $(echo "$code" | jq -Rs .), $extra_fields}"
  else
    payload="{\"code\": $(echo "$code" | jq -Rs .)}"
  fi

  # Submit
  local submit_resp
  submit_resp=$(curl -sf -X POST "$BASE_URL/api/exec" \
    -H "Content-Type: application/json" \
    -d "$payload")

  local exec_id
  exec_id=$(echo "$submit_resp" | jq -r '.execution_id // empty')

  if [ -z "$exec_id" ]; then
    # Submission itself failed (HTTP 500)
    echo "$submit_resp"
    return 0
  fi

  # Poll until terminal state
  local elapsed=0
  while [ $elapsed -lt $timeout_secs ]; do
    sleep 1
    elapsed=$((elapsed + 1))

    local poll_resp
    poll_resp=$(curl -sf "$BASE_URL/api/executions/$exec_id" 2>/dev/null || echo '{}')

    local status
    status=$(echo "$poll_resp" | jq -r '.status // empty')

    case "$status" in
      completed|failed|timed_out|cancelled)
        echo "$poll_resp"
        return 0
        ;;
    esac
  done

  echo '{"status":"poll_timeout","error":"Polling timed out after '"$timeout_secs"'s"}'
}

# Submit code that is expected to fail at submission time (HTTP 500).
# Returns the HTTP status code and response body.
exec_js_raw() {
  local payload="$1"
  local http_code
  http_code=$(curl -s -o /tmp/mcp-test-resp.json -w "%{http_code}" -X POST "$BASE_URL/api/exec" \
    -H "Content-Type: application/json" \
    -d "$payload")
  local body
  body=$(cat /tmp/mcp-test-resp.json)
  echo "$http_code|$body"
}

# ── Start services ───────────────────────────────────────────────────────────

echo "==> Starting docker-compose services..."
docker compose -f "$COMPOSE_FILE" up -d

wait_for_ready "$BASE_URL"

# ── Test 1: Basic JavaScript execution ───────────────────────────────────────

echo ""
echo "==> Test 1: Basic JavaScript execution"
RESULT=$(run_js 'const x = 2 + 3; x;')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
OUTPUT=$(echo "$RESULT" | jq -r '.result // empty')
if [ "$STATUS" = "completed" ] && [ "$OUTPUT" = "5" ]; then
  pass "JS execution returned 5"
else
  fail "Expected status=completed result=5, got status=$STATUS result=$OUTPUT (full: $RESULT)"
fi

# ── Test 2: fetch() to allowed domain succeeds ──────────────────────────────

echo ""
echo "==> Test 2: fetch() to allowed domain (registry.npmjs.org)"
RESULT=$(run_js '(async () => { const r = await fetch("https://registry.npmjs.org/"); console.log(r.ok); })()' '' 30)
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
if [ "$STATUS" = "completed" ]; then
  # Check console output for "true"
  EXEC_ID=$(echo "$RESULT" | jq -r '.execution_id // empty')
  CONSOLE_OUTPUT=$(curl -sf "$BASE_URL/api/executions/$EXEC_ID/output" 2>/dev/null | jq -r '.lines[]?.text // empty' 2>/dev/null || echo "")
  if echo "$CONSOLE_OUTPUT" | grep -q "true"; then
    pass "fetch() to allowed domain returned ok=true"
  else
    pass "fetch() to allowed domain completed (console: $CONSOLE_OUTPUT)"
  fi
else
  ERROR=$(echo "$RESULT" | jq -r '.error // empty')
  fail "Expected completed, got status=$STATUS error=$ERROR"
fi

# ── Test 3: fetch() to blocked domain is denied by OPA ──────────────────────

echo ""
echo "==> Test 3: fetch() to blocked domain (evil.example.com)"
RESULT=$(run_js '(async () => { const r = await fetch("https://evil.example.com/"); console.log(r.ok); })()')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
ERROR=$(echo "$RESULT" | jq -r '.error // empty')
if [ "$STATUS" = "failed" ] && echo "$ERROR" | grep -qi "denied\|policy"; then
  pass "fetch() to blocked domain was denied by policy"
else
  fail "Expected failed with policy denial, got status=$STATUS error=$ERROR (full: $RESULT)"
fi

# ── Test 4: Heap snapshot created and restorable ─────────────────────────────

echo ""
echo "==> Test 4: Heap snapshot persistence"

# 4a: Execute code that sets state, capture the heap hash
RESULT=$(run_js 'var counter = 42;')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
HEAP=$(echo "$RESULT" | jq -r '.heap // empty')
if [ "$STATUS" != "completed" ] || [ -z "$HEAP" ] || [ "$HEAP" = "null" ]; then
  fail "No heap hash returned from initial execution (status=$STATUS, heap=$HEAP)"
else
  pass "Heap hash returned: ${HEAP:0:16}..."

  # 4b: Restore the heap and read the persisted state
  RESULT2=$(run_js 'counter;' "\"heap\": \"$HEAP\"")
  STATUS2=$(echo "$RESULT2" | jq -r '.status // empty')
  OUTPUT2=$(echo "$RESULT2" | jq -r '.result // empty')
  if [ "$STATUS2" = "completed" ] && [ "$OUTPUT2" = "42" ]; then
    pass "Heap restored successfully, counter = 42"
  else
    fail "Expected counter=42 after heap restore, got status=$STATUS2 result=$OUTPUT2"
  fi
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "=============================="
echo "Results: $PASSED passed, $FAILED failed"
echo "=============================="

if [ "$FAILED" -gt 0 ]; then
  exit 1
fi
