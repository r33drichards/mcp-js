#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: scripts/test-python-uniffi-library.sh <shared-library>" >&2
  exit 2
fi

if ! command -v uniffi-bindgen >/dev/null 2>&1; then
  echo "uniffi-bindgen is required" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
library="$(realpath "$1")"
out_dir="$(mktemp -d)"
trap 'rm -rf "$out_dir"' EXIT

uniffi-bindgen generate \
  --language python \
  --out-dir "$out_dir" \
  --no-format \
  "$library"

case "$(uname -s)" in
  Darwin) native_name="libmcp_v8_uniffi.dylib" ;;
  Linux) native_name="libmcp_v8_uniffi.so" ;;
  *) echo "Unsupported Python UniFFI test platform: $(uname -s)" >&2; exit 1 ;;
esac

cp "$library" "$out_dir/$native_name"
PYTHONPATH="$out_dir" python3 "$repo_root/scripts/python-uniffi-library-smoke.py"
