#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
export MCP_JS_DEMO_STATE="${MCP_JS_DEMO_STATE:-$PWD/.state}"
export DOCKER_CONFIG="${DOCKER_CONFIG:-$MCP_JS_DEMO_STATE/docker-config}"
case "${1:-}" in
 prepare) shift; exec python3 prepare.py "$@" ;;
 up)
  test -f "$MCP_JS_DEMO_STATE/compose.json" || { echo 'Run launch.sh prepare --binary /absolute/current/server first'; exit 1; }
  python3 -c 'import shutil,sys; free=shutil.disk_usage(".").free; sys.exit("At least 14 GiB free disk required for full Astronomy/browser stack; nothing started") if free<14*1024**3 else None'
  python3 lock-images.py
  docker compose -f "$MCP_JS_DEMO_STATE/compose.json" --profile browser up -d --build
  echo 'Wait for browser MCP readiness before starting queries. Run python3 demo.py seed, then launch.sh serve.' ;;
 serve) exec python3 demo.py serve ;;
 down|reset) exec python3 cleanup.py "${1}" ;;
 *) echo 'Usage: launch.sh prepare --binary PATH | up | serve | down | reset'; exit 2 ;;
esac
