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
mkdir -p "$root/node/generated"
cd "$root"
"$generator" generate napi bindings --library "$library" \
  --ts-dir "$root/node/generated" --lib-absolute --no-format
