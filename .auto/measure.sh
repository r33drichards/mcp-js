#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
test "$(git hash-object loadtest/fs-grep.mjs)" = df2d9fa3dff6f393a818299290bbd2e7ac2521c2 || {
  echo "Benchmark workload changed" >&2
  exit 1
}
command -v "${MOTO_SERVER:-moto_server}" >/dev/null
corpus=$(python3 .auto/cache-corpora.py "${FS_GREP_DATA_DIR:-.fs-grep-data}")
revision=4c6c554cca365a075834805cc0d15d11cbdc7aaa
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo build --locked --release -p server --bin server
binary="$CARGO_TARGET_DIR/release/server"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# Use the same 24 slices, two passes, and latency profile as the PR workflow.
# Bound concurrent slices on this host so Moto and the local blob stores fit.
run_slice() (
  slice=$1
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  "${MOTO_SERVER:-moto_server}" -H 127.0.0.1 -p "$port" > "$scratch/moto-$slice.log" 2>&1 &
  moto_pid=$!
  trap 'kill "$moto_pid" 2>/dev/null || true; wait "$moto_pid" 2>/dev/null || true' EXIT
  export AWS_ACCESS_KEY_ID=testing AWS_SECRET_ACCESS_KEY=testing AWS_REGION=us-east-1
  export AWS_ENDPOINT_URL="http://127.0.0.1:$port" AWS_S3_FORCE_PATH_STYLE=1
  if ! curl --noproxy '*' -fsS --retry 5 --retry-connrefused -X PUT "$AWS_ENDPOINT_URL/fs-grep"; then
    cat "$scratch/moto-$slice.log" >&2
    exit 1
  fi
  options=()
  if (( slice != 0 )); then options+=(--skip-mdn); fi
  node loadtest/fs-grep.mjs --binary "$binary" --data-dir "$corpus" --s3-bucket fs-grep \
    --wiki-revision "$revision" --wiki-slice "$slice/24" --s3-latency 20/60 "${options[@]}" \
    | tee "$scratch/fs-grep-$slice.jsonl"
)
for ((slice=0; slice<24; slice+=2)); do
  run_slice "$slice" &
  first=$!
  run_slice "$((slice+1))" &
  second=$!
  status=0
  wait "$first" || status=1
  wait "$second" || status=1
  if (( status != 0 )); then exit "$status"; fi
done
node .auto/verify.mjs "$scratch"
