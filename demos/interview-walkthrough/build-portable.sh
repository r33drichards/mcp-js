#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
export MCP_JS_DEMO_STATE="${MCP_JS_DEMO_STATE:-$PWD/.state}"
# Preserve Docker Desktop plugin/auth configuration supplied by the local operator.
export DOCKER_CONFIG="${DOCKER_CONFIG:-$MCP_JS_DEMO_STATE/docker-config}"
DEMO_IMAGE_ID=$(python3 -c 'import demo;print(demo.config()["id"])')
DEMO_BUILD_CONTEXT=$(mktemp -d "${TMPDIR:-/tmp}/mcp-interview-build.XXXXXX")
trap 'rm -rf "$DEMO_BUILD_CONTEXT"' EXIT
python3 build-context.py --repo ../.. --revision "${MCP_JS_BUILD_COMMIT:-HEAD}" --output "$DEMO_BUILD_CONTEXT/source"
# One Cargo job limits parallel compilation, not linker peak memory.
docker build --platform linux/amd64 --build-arg CARGO_BUILD_JOBS=1 -f "$DEMO_BUILD_CONTEXT/source/Dockerfile.source" -t "$DEMO_IMAGE_ID-engine:source" "$DEMO_BUILD_CONTEXT/source"
mkdir "$DEMO_BUILD_CONTEXT/runtime"
for file in Dockerfile.runtime object-store.py filesystem.rego; do
 cp "$DEMO_BUILD_CONTEXT/source/demos/interview-walkthrough/$file" "$DEMO_BUILD_CONTEXT/runtime/$file"
done
docker build --platform linux/amd64 -f "$DEMO_BUILD_CONTEXT/runtime/Dockerfile.runtime" --build-arg "ENGINE_IMAGE=$DEMO_IMAGE_ID-engine:source" -t "$DEMO_IMAGE_ID:local" "$DEMO_BUILD_CONTEXT/runtime"
