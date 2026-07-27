#!/usr/bin/env bash
set -euo pipefail

v8_version="${1:-145.0.0}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$repo_root"
cargo fetch

v8_source="$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -mindepth 2 -maxdepth 2 -type d -name "v8-$v8_version" -print -quit)"
if [[ -z "$v8_source" ]]; then
  echo "Unable to locate the v8-$v8_version Cargo source tree" >&2
  exit 1
fi

build_revision="$(sed -n "s#.*chromium/src/build.git.*'@' + '\([0-9a-f]\{40\}\)'.*#\1#p" "$v8_source/v8/DEPS" | head -1)"
if [[ -z "$build_revision" ]]; then
  echo "Unable to determine Chromium build revision from $v8_source/v8/DEPS" >&2
  exit 1
fi

destination="$v8_source/build/rust/known-target-triples.txt"
mkdir -p "$(dirname "$destination")"
curl -fsSL \
  "https://chromium.googlesource.com/chromium/src/build/+/$build_revision/rust/known-target-triples.txt?format=TEXT" \
  | base64 --decode > "$destination"

test -s "$destination"
echo "Restored $destination from Chromium build revision $build_revision"
