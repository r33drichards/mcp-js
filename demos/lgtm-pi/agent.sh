#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v node >/dev/null; then
 for p in ../../../tools/node-*/bin; do test ! -x "$p/node" || export PATH="$(cd "$p" && pwd):$PATH"; done
fi
command -v node >/dev/null || { echo 'Node required; install the approved Node runtime first.'; exit 1; }
export AWS_REGION="${AWS_REGION:-us-west-2}"
exec ./node_modules/.bin/pi --no-builtin-tools --no-extensions --extension builtin:mcp --extension ./agent.ts --no-skills --no-prompt-templates --no-context-files --tools mcp__demo__run_js,mcp__demo__get_artifact,mcp__demo__list_artifacts --append-system-prompt "$(cat agent-prompt.md)" "$@"
