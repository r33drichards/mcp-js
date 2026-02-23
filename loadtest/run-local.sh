#!/usr/bin/env bash
# run-local.sh — Run a load test benchmark locally.
#
# Usage:
#   ./loadtest/run-local.sh                           # cluster-stateful, 1000 req/s, 60s
#   ./loadtest/run-local.sh single stateless           # single-stateless
#   ./loadtest/run-local.sh cluster stateful 5000 30s  # cluster-stateful, 5000 req/s, 30s
set -euo pipefail

TOPOLOGY="${1:-cluster}"
MODE="${2:-stateful}"
RATE="${3:-1000}"
DURATION="${4:-60s}"
LABEL="${TOPOLOGY}-${MODE}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="${PROJECT_DIR}/server/target/release/server"
DATA_DIR="/tmp/mcp-loadtest-$$"

cleanup() {
  echo ""
  echo "Stopping servers..."
  for pidfile in "${DATA_DIR}"/*.pid; do
    [ -f "$pidfile" ] || continue
    kill "$(cat "$pidfile")" 2>/dev/null || true
  done
  sleep 1
  for pidfile in "${DATA_DIR}"/*.pid; do
    [ -f "$pidfile" ] || continue
    kill -9 "$(cat "$pidfile")" 2>/dev/null || true
  done
  rm -rf "$DATA_DIR"
}
trap cleanup EXIT

# ── Preflight ──────────────────────────────────────────────────────────
if ! command -v k6 &>/dev/null; then
  echo "Error: k6 is not installed. Install with: brew install k6" >&2
  exit 1
fi

if [ ! -x "$BINARY" ]; then
  echo "Binary not found at ${BINARY}, building..."
  (cd "$PROJECT_DIR" && nix develop --command bash -c "cd server && cargo build --release")
fi

# ── Data dirs ──────────────────────────────────────────────────────────
mkdir -p "${DATA_DIR}/node"{1,2,3}/{heaps,sessions}
mkdir -p "${PROJECT_DIR}/results"

echo "============================================================"
echo "  Benchmark: ${LABEL} @ ${RATE} req/s for ${DURATION}"
echo "============================================================"
echo ""

# ── Start servers ──────────────────────────────────────────────────────
if [ "$TOPOLOGY" = "single" ]; then
  if [ "$MODE" = "stateful" ]; then
    ARGS="--http-port=3001 --directory-path=${DATA_DIR}/node1/heaps --session-db-path=${DATA_DIR}/node1/sessions"
  else
    ARGS="--http-port=3001 --stateless"
  fi

  echo "Starting single node on port 3001..."
  "$BINARY" $ARGS > "${DATA_DIR}/node1.log" 2>&1 &
  echo $! > "${DATA_DIR}/node1.pid"

  TARGET_URLS="http://127.0.0.1:3001"
else
  for i in 1 2 3; do
    HTTP_PORT=$((3000 + i))
    CLUSTER_PORT=$((4000 + i))
    NODE_ID="node${i}"

    PEERS=""
    for j in 1 2 3; do
      if [ "$j" -ne "$i" ]; then
        [ -n "$PEERS" ] && PEERS="${PEERS},"
        PEERS="${PEERS}node${j}@127.0.0.1:$((4000 + j))"
      fi
    done

    if [ "$MODE" = "stateful" ]; then
      MODE_ARGS="--directory-path=${DATA_DIR}/${NODE_ID}/heaps --session-db-path=${DATA_DIR}/${NODE_ID}/sessions"
    else
      MODE_ARGS="--stateless"
    fi

    echo "Starting ${NODE_ID} on HTTP ${HTTP_PORT}, cluster ${CLUSTER_PORT}..."
    "$BINARY" --http-port=${HTTP_PORT} ${MODE_ARGS} \
      --cluster-port=${CLUSTER_PORT} \
      --node-id=${NODE_ID} \
      --peers=${PEERS} \
      --advertise-addr=127.0.0.1:${CLUSTER_PORT} \
      --heartbeat-interval=200 \
      --election-timeout-min=1000 \
      --election-timeout-max=2000 \
      > "${DATA_DIR}/${NODE_ID}.log" 2>&1 &
    echo $! > "${DATA_DIR}/${NODE_ID}.pid"
  done

  TARGET_URLS="http://127.0.0.1:3001,http://127.0.0.1:3002,http://127.0.0.1:3003"
fi

# ── Health check ───────────────────────────────────────────────────────
echo ""
echo "Waiting for servers to be ready..."

IFS=',' read -ra URLS <<< "$TARGET_URLS"
for URL in "${URLS[@]}"; do
  for i in $(seq 1 30); do
    if curl -sf --max-time 10 -o /dev/null -X POST "${URL}/api/exec" \
      -H 'Content-Type: application/json' \
      -d '{"code":"1"}' 2>/dev/null; then
      echo "  ${URL} ready"
      break
    fi
    if [ "$i" -eq 30 ]; then
      echo "ERROR: ${URL} did not become ready"
      cat "${DATA_DIR}"/*.log 2>/dev/null || true
      exit 1
    fi
    sleep 1
  done
done

if [ "$TOPOLOGY" = "cluster" ]; then
  echo "Waiting for Raft leader election..."
  for i in $(seq 1 30); do
    ROLE=$(curl -sf --max-time 5 http://127.0.0.1:4001/raft/status 2>/dev/null \
      | python3 -c "import sys,json; print(json.load(sys.stdin).get('role',''))" 2>/dev/null || true)
    if [ "$ROLE" = "Leader" ] || [ "$ROLE" = "Follower" ]; then
      echo "  Raft operational (node1: ${ROLE})"
      break
    fi
    sleep 1
  done
fi

# ── Run k6 ─────────────────────────────────────────────────────────────
echo ""
k6 run \
  --out json="${PROJECT_DIR}/results/raw-${LABEL}-${RATE}.json" \
  -e TARGET_URLS="${TARGET_URLS}" \
  -e TARGET_RATE="${RATE}" \
  -e DURATION="${DURATION}" \
  -e TOPOLOGY="${LABEL}" \
  "${SCRIPT_DIR}/k6-load-test.js" \
  || true

# Move k6 handleSummary output
if [ -f "results-${LABEL}-${RATE}rps.json" ]; then
  mv "results-${LABEL}-${RATE}rps.json" "${PROJECT_DIR}/results/"
fi

# ── Report ─────────────────────────────────────────────────────────────
echo ""
chmod +x "${SCRIPT_DIR}/generate-report.sh"
"${SCRIPT_DIR}/generate-report.sh" "${PROJECT_DIR}/results" || true
