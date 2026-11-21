#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: node/scripts/package-musl.sh <output-directory>" >&2
  exit 2
fi
mkdir -p "$1"
output="$(realpath "$1")"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/source" "$stage/artifacts"
cd "$root"

export CARGO_TARGET_DIR="$root/target/node-musl"
target=x86_64-unknown-linux-musl
nix develop .#musl-sdk --command cargo build --locked --release \
  --target "$target" -p mcp-v8-uniffi-python
nix develop --command bash node/scripts/build-bindgen.sh
nix develop --command bash node/scripts/generate.sh \
  "$CARGO_TARGET_DIR/$target/release/libmcp_v8_uniffi.so"
nix develop --command npm run build --prefix node

tar --exclude=node/node_modules --exclude=node/generated -cf - LICENSE node |
  tar -C "$stage/source" -xf -
node --input-type=module - "$stage/source/node" <<'JS'
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const directory = process.argv[2];
const manifestPath = join(directory, 'package.json');
const lockPath = join(directory, 'package-lock.json');
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
const lock = JSON.parse(readFileSync(lockPath, 'utf8'));
manifest.description = 'Native embedded mcp-js engine for Node.js (Linux x64 musl)';
manifest.libc = ['musl'];
lock.packages[''].libc = manifest.libc;
writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + '\n');
writeFileSync(lockPath, JSON.stringify(lock, null, 2) + '\n');
JS

# Pack and test with native musl Node and the same Bun image used by Noisy.
# npm's host-libc check and the native addon selection run inside Alpine.
docker run --rm --platform linux/amd64 --user 0:0 --entrypoint sh \
  -v "$stage/source:/source:ro" -v "$stage/artifacts:/artifacts" \
  oven/bun:1.4.4-alpine -eu -c '
    apk add --no-cache nodejs npm binutils libstdc++
    cp -R /source /tmp/sdk
    cd /tmp/sdk/node
    npm ci --include=dev
    npm pack --pack-destination /artifacts
    TEST_WITH_BUN=1 node scripts/test-package.mjs /artifacts/*.tgz
  '
tarballs=("$stage/artifacts/"*.tgz)
[[ ${#tarballs[@]} -eq 1 && -f "${tarballs[0]}" ]]
filename="$(basename "${tarballs[0]}" .tgz)"
cp "${tarballs[0]}" "$output/$filename-linux-x64-musl.tgz"
