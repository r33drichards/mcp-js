#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
jar="${TLA2TOOLS_JAR:-$PWD/tla2tools.jar}"
out="${TLC_RESULTS_DIR:-$PWD/cron-results}"
mkdir -p "$out"
if [[ ! -f "$jar" ]]; then
  echo 'Download tla2tools.jar as described in TaskScheduleCron.md.' >&2
  exit 1
fi
printf '%s  %s\n' \
  0f56d6a813368c48008dfbecb23a73ab43a4a5e1bf09f026782a674aa9d4b3c9 \
  "$jar" | sha256sum --check --status
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
printf 'config\texit\tresult\n' > "$out/summary.tsv"
run() {
  local config=$1 expected_exit=$2 expected_text=$3 code=0
  TZ=UTC java -Duser.timezone=UTC -XX:+UseParallelGC -Xmx2g -cp "$jar" tlc2.TLC \
    -workers 1 -seed 1 -fp 0 -deadlock -noGenerateSpecTE \
    -metadir "$scratch/$config" -config "$config.cfg" TaskScheduleCron.tla \
    > "$out/$config.log" 2>&1 || code=$?
  if [[ "$code" != "$expected_exit" ]] || ! grep -Fq "$expected_text" "$out/$config.log"; then
    cat "$out/$config.log" >&2
    echo "Unexpected TLC result: $config (exit $code)" >&2
    exit 1
  fi
  printf '%s\t%s\t%s\n' "$config" "$code" "$expected_text" >> "$out/summary.tsv"
  echo "$config: $expected_text"
}
run TaskScheduleCron_safe 0 'Model checking completed. No error has been found.'
run TaskScheduleCron_overlap 12 'Invariant NoOverlapDiagnostic is violated.'
run TaskScheduleCron_catchup 12 'Invariant NoCatchUp is violated.'
run TaskScheduleCron_retry 12 'Invariant NoAutomaticRetry is violated.'
echo "All four expected outcomes verified. Logs: $out"
