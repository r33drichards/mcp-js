#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo test --locked --release -p server --test fs_snapshot_store --test fs_mount_overlay --test fs_snapshot_e2e
