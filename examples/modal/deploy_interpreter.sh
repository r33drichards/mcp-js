#!/usr/bin/env bash
set -euo pipefail

policy_dir=$(mktemp -d)
trap 'rm -rf "$policy_dir"' EXIT

cat >"$policy_dir/filesystem.rego" <<'REGO'
package mcp.filesystem

default allow := false

allow if {
    input.operation in {"readFile", "writeFile", "mkdir", "readdir"}
    startswith(input.path, "/workspace/")
    not ".." in split(input.path, "/")
}

allow if {
    input.operation == "readdir"
    input.path == "/workspace"
}
REGO

cat >"$policy_dir/fetch.rego" <<'REGO'
package mcp.fetch

default allow := false

allow if {
    input.method == "GET"
    input.url_parsed.scheme == "https"
    input.url_parsed.host == "example.com"
    input.url_parsed.port == null
}
REGO

cat >"$policy_dir/modules.rego" <<'REGO'
package mcp.modules

default allow := false

allow if {
    input.resolved_url == "https://cdn.jsdelivr.net/npm/escape-html@1.0.3/+esm"
}
REGO

policies='{
  "filesystem": {"policies": [{"url": "file:///policies/filesystem.rego"}]},
  "fetch": {"policies": [{"url": "file:///policies/fetch.rego"}]},
  "modules": {"policies": [{"url": "file:///policies/modules.rego"}]}
}'

python - <<'PYTHON'
import modal

for name in ("state", "workspace", "policies"):
    modal.Volume.objects.create(f"mcp-js-interpreter-{name}", allow_existing=True)
PYTHON

for name in filesystem fetch modules; do
  modal volume put --force mcp-js-interpreter-policies "$policy_dir/$name.rego" "/$name.rego"
done

modal server deploy \
  --app-name mcp-js-code-interpreter \
  --image docker.io/wholelottahoopla/mcp-js:0.22.0 \
  --port 8080 \
  --min-containers 0 \
  --max-containers 1 \
  --scaledown-window 60 \
  --volume mcp-js-interpreter-state:/data \
  --volume mcp-js-interpreter-workspace:/workspace \
  --volume mcp-js-interpreter-policies:/policies \
  --env-var "MCP_V8_POLICIES_JSON=$policies" \
  --env-var MCP_V8_ALLOW_EXTERNAL_MODULES=true \
  --env-var PORT=8080 \
  --env-var MCP_V8_HEAP_STORE=dir \
  --env-var MCP_V8_HEAP_DIR=/data/heaps \
  --env-var MCP_V8_SESSION_DB_PATH=/data/sessions
