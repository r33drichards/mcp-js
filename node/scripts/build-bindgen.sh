#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
revision=0f7fc67e8dd98ad43af3787d45c3c570d469a145
source_dir="$(mktemp -d)"
trap 'rm -rf "$source_dir"' EXIT
git -C "$source_dir" init --quiet
git -C "$source_dir" fetch --quiet --depth 1 \
  https://github.com/jhugman/uniffi-bindgen-react-native.git "$revision"
git -C "$source_dir" checkout --quiet --detach FETCH_HEAD
git -C "$source_dir" apply "$root/node/patches/ubrn-uniffi-0.32.patch"
export CARGO_TARGET_DIR="$root/target/node-bindgen"
cargo build --locked --manifest-path "$source_dir/crates/ubrn_cli/Cargo.toml" \
  --no-default-features
