#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: node/scripts/generate.sh <native-shared-library>" >&2
  exit 2
fi
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
library="$(realpath "$1")"
[[ -f "$library" ]] || { echo "Missing library: $library" >&2; exit 1; }
generator="${UBRN_BIN:-$root/target/node-bindgen/debug/uniffi-bindgen-react-native}"
rm -rf "$root/node/generated" "$root/node/dist"
mkdir -p "$root/node/generated"
cd "$root"
"$generator" generate napi bindings --library "$library" \
  --ts-dir "$root/node/generated" --lib-colocated --no-format

cp "$library" "$root/node/generated/libserver.so"

cp "$root/node/src/tool-runtime.ts" "$root/node/generated/tool-runtime.ts"
cat >> "$root/node/generated/index.ts" <<'TS'

export { createToolRuntime } from "./tool-runtime.js";
export type { HostTool, ToolRuntimeConfig, ToolRuntimeResult } from "./tool-runtime.js";
TS
