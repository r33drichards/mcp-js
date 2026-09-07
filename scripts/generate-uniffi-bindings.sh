#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/generate-uniffi-bindings.sh <swift|kotlin|python|ruby> [output-dir]

Environment:
  PROFILE           Cargo profile to build (default: debug)
  TARGET            Optional Rust target triple
  CARGO_TARGET_DIR  Optional Cargo target directory
USAGE
}

language="${1:-}"
if [[ -z "$language" ]]; then
  usage >&2
  exit 2
fi

case "$language" in
  swift|kotlin|python|ruby) ;;
  *)
    echo "Unsupported UniFFI language: $language" >&2
    usage >&2
    exit 2
    ;;
esac

if ! command -v uniffi-bindgen >/dev/null 2>&1; then
  echo "uniffi-bindgen is required; install UniFFI 0.32.0 with the cli feature" >&2
  exit 1
fi

uniffi_version="$(uniffi-bindgen --version)"
if [[ "$uniffi_version" != "uniffi-bindgen 0.32.0" ]]; then
  echo "Expected uniffi-bindgen 0.32.0, found: $uniffi_version" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile="${PROFILE:-debug}"
out_dir="${2:-$repo_root/generated/uniffi/$language}"
target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"

cargo_args=(build -p mcp-v8-uniffi)
profile_dir="$profile"
if [[ "$profile" == "release" ]]; then
  cargo_args+=(--release)
elif [[ "$profile" != "debug" ]]; then
  cargo_args+=(--profile "$profile")
fi
if [[ -n "${TARGET:-}" ]]; then
  cargo_args+=(--target "$TARGET")
  artifact_dir="$target_dir/$TARGET/$profile_dir"
else
  artifact_dir="$target_dir/$profile_dir"
fi

case "${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}" in
  *-windows-*) library="$artifact_dir/mcp_v8_uniffi.lib" ;;
  *) library="$artifact_dir/libmcp_v8_uniffi.a" ;;
esac

cd "$repo_root"
cargo "${cargo_args[@]}"

if [[ ! -f "$library" ]]; then
  echo "Expected static library was not produced: $library" >&2
  exit 1
fi

rm -rf "$out_dir"
mkdir -p "$out_dir"
uniffi-bindgen generate \
  --language "$language" \
  --out-dir "$out_dir" \
  --no-format \
  "$library"

echo "Generated $language bindings in $out_dir"
