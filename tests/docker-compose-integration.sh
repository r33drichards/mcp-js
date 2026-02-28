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
  exit 1
}

exec_js() {
  local payload="$1"
  curl -sf -X POST "$BASE_URL/api/exec" \
    -H "Content-Type: application/json" \
    -d "$payload"
}

# ── Start services ───────────────────────────────────────────────────────────

echo "==> Starting docker-compose services..."
docker compose -f "$COMPOSE_FILE" up -d

wait_for_ready "$BASE_URL"

# ── Test 1: Basic JavaScript execution ───────────────────────────────────────

echo ""
echo "==> Test 1: Basic JavaScript execution"
RESULT=$(exec_js '{"code":"const x = 2 + 3; x;"}')
OUTPUT=$(echo "$RESULT" | jq -r '.output')
if [ "$OUTPUT" = "5" ]; then
  pass "JS execution returned 5"
else
  fail "Expected output '5', got '$OUTPUT'"
fi

# ── Test 2: fetch() to allowed domain succeeds ──────────────────────────────

echo ""
echo "==> Test 2: fetch() to allowed domain (registry.npmjs.org)"
RESULT=$(exec_js '{"code":"const r = fetch(\"https://registry.npmjs.org/\"); r.ok;"}')
OUTPUT=$(echo "$RESULT" | jq -r '.output')
if [ "$OUTPUT" = "true" ]; then
  pass "fetch() to allowed domain returned ok=true"
else
  fail "Expected output 'true', got '$OUTPUT' (full response: $RESULT)"
fi

# ── Test 3: fetch() to blocked domain is denied by OPA ──────────────────────

echo ""
echo "==> Test 3: fetch() to blocked domain (evil.example.com)"
HTTP_CODE=$(curl -s -o /tmp/mcp-test-deny.json -w "%{http_code}" -X POST "$BASE_URL/api/exec" \
  -H "Content-Type: application/json" \
  -d '{"code":"fetch(\"https://evil.example.com/\");"}')
BODY=$(cat /tmp/mcp-test-deny.json)
if [ "$HTTP_CODE" = "500" ] && echo "$BODY" | grep -qi "denied\|policy"; then
  pass "fetch() to blocked domain was denied by policy (HTTP $HTTP_CODE)"
else
  fail "Expected HTTP 500 with policy denial, got HTTP $HTTP_CODE: $BODY"
fi

# ── Test 4: Heap snapshot created and restorable ─────────────────────────────

echo ""
echo "==> Test 4: Heap snapshot persistence"

# 4a: Execute code that sets state, capture the heap hash
RESULT=$(exec_js '{"code":"var counter = 42;"}')
HEAP=$(echo "$RESULT" | jq -r '.heap')
if [ -z "$HEAP" ] || [ "$HEAP" = "null" ]; then
  fail "No heap hash returned from initial execution"
else
  pass "Heap hash returned: ${HEAP:0:16}..."

  # 4b: Restore the heap and read the persisted state
  RESULT2=$(exec_js "{\"code\":\"counter;\",\"heap\":\"$HEAP\"}")
  OUTPUT2=$(echo "$RESULT2" | jq -r '.output')
  if [ "$OUTPUT2" = "42" ]; then
    pass "Heap restored successfully, counter = 42"
  else
    fail "Expected counter=42 after heap restore, got '$OUTPUT2'"
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
