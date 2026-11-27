#!/usr/bin/env bash
set -euo pipefail

modal server deploy \
  --app-name mcp-js-code-interpreter \
  --image docker.io/wholelottahoopla/mcp-js:0.22.0 \
  --port 8080 \
  --min-containers 0 \
  --max-containers 1 \
  --scaledown-window 60 \
  --volume mcp-js-interpreter-state:/data \
  --env-var PORT=8080 \
  --env-var MCP_V8_HEAP_STORE=dir \
  --env-var MCP_V8_HEAP_DIR=/data/heaps \
  --env-var MCP_V8_SESSION_DB_PATH=/data/sessions
