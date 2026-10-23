#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
export MCP_JS_DEMO_STATE="${MCP_JS_DEMO_STATE:-$PWD/.state}"
export DOCKER_CONFIG="$MCP_JS_DEMO_STATE/docker-config"
DEMO_IMAGE_ID=$(python3 -c 'import demo;print(demo.config()["id"])')
# Compile this checkout, never silently substitute the default latest release image.
docker build --platform linux/amd64 -f ../../Dockerfile.source -t "$DEMO_IMAGE_ID-engine:source" ../..
docker build --platform linux/amd64 -f Dockerfile.runtime --build-arg "ENGINE_IMAGE=$DEMO_IMAGE_ID-engine:source" -t "$DEMO_IMAGE_ID:local" .
