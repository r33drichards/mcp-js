#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
FETCH=(python3 "$ROOT/tools/compat/fetch-node-compat-corpora.py")
if [[ ${NODE_COMPAT_OFFLINE:-0} == 1 ]]; then
  FETCH+=(--offline)
fi

usage() {
  cat <<'USAGE'
Usage: tools/compat/node-compat.sh COMMAND [ARG]

Commands:
  fetch                 Fetch the pinned Deno node_test and CITGM corpora
  inventory             Regenerate the full Deno-vendored test inventory
  report                Regenerate JSON and Markdown compatibility reports
  fast                  Run the curated Node core compatibility suite
  family <name>         Run curated tests for one module family
  profile <name>        Run curated tests for one capability profile
  check                 Run tooling tests, fast tests, and generated-file checks
USAGE
}

run_fast() {
  (cd "$ROOT/server" && cargo test --test node_compat node_core_subset_matches_expectations -- --nocapture)
}

case "${1:---help}" in
  -h|--help|help)
    usage
    ;;
  fetch)
    "${FETCH[@]}" --source all
    ;;
  inventory)
    corpus=$("${FETCH[@]}" --source deno_node_test | tail -1)
    python3 "$ROOT/tools/compat/gen-node-compat-inventory.py" --corpus "$corpus"
    ;;
  report)
    python3 "$ROOT/tools/compat/gen-node-compat-report.py"
    python3 "$ROOT/tools/compat/gen-compat-docs.py"
    ;;
  fast)
    run_fast
    ;;
  family)
    [[ $# -eq 2 && -n $2 ]] || { echo "family requires a name" >&2; exit 2; }
    (cd "$ROOT/server" && NODE_COMPAT_FAMILY="$2" cargo test --test node_compat node_core_subset_matches_expectations -- --nocapture)
    ;;
  profile)
    [[ $# -eq 2 && -n $2 ]] || { echo "profile requires a name" >&2; exit 2; }
    (cd "$ROOT/server" && NODE_COMPAT_PROFILE="$2" cargo test --test node_compat node_core_subset_matches_expectations -- --nocapture)
    ;;
  check)
    python3 -m unittest "$ROOT/tools/compat/tests/test_node_compat_tools.py" -v
    run_fast
    corpus=$("${FETCH[@]}" --source deno_node_test | tail -1)
    python3 "$ROOT/tools/compat/gen-node-compat-inventory.py" --corpus "$corpus" --check
    python3 "$ROOT/tools/compat/gen-node-compat-report.py" --check
    python3 "$ROOT/tools/compat/gen-compat-docs.py"
    git -C "$ROOT" diff --exit-code -- site-docs/reference/compatibility.md
    ;;
  *)
    echo "unknown command: $1" >&2
    usage >&2
    exit 2
    ;;
esac
