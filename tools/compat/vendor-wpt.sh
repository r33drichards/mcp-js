#!/usr/bin/env bash
# Vendor a curated subset of web-platform-tests into server/tests/wpt/vendor/.
#
# Usage:
#   tools/compat/vendor-wpt.sh /path/to/wpt-checkout
#
# The checkout can be a sparse clone:
#   git clone --depth 1 --filter=blob:none --sparse \
#       https://github.com/web-platform-tests/wpt /tmp/wpt
#   git -C /tmp/wpt sparse-checkout set resources html/webappapis console \
#       encoding fetch/api/headers wasm/jsapi
#
# Only serverless testharness tests are vendored (no wptserve, no reftests,
# no testdriver). Re-running refreshes the vendored files and records the
# source commit in versions.json. WPT is BSD-3-Clause; see
# server/tests/wpt/vendor/LICENSE.
set -euo pipefail

WPT_SRC="${1:?usage: vendor-wpt.sh /path/to/wpt-checkout}"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEST="$REPO_ROOT/server/tests/wpt/vendor"

# Harness (always required).
FILES=(
  resources/testharness.js
  common/sab.js
  LICENSE.md
)

# html/webappapis — atob/btoa, timers, microtask queuing.
FILES+=(
  html/webappapis/atob/base64.any.js
  html/webappapis/microtask-queuing/queue-microtask.any.js
  html/webappapis/timers/clearinterval-from-callback.any.js
  html/webappapis/timers/cleartimeout-clearinterval.any.js
  html/webappapis/timers/evil-spec-example.any.js
  html/webappapis/timers/missing-timeout-setinterval.any.js
  html/webappapis/timers/negative-setinterval.any.js
  html/webappapis/timers/negative-settimeout.any.js
  html/webappapis/timers/setinterval-settimeout-clamping.any.js
  html/webappapis/timers/type-long-setinterval.any.js
  html/webappapis/timers/type-long-settimeout.any.js
)

# console (idlharness.any.js excluded — needs WebIDL infrastructure).
FILES+=(
  console/console-is-a-namespace.any.js
  console/console-label-conversion.any.js
  console/console-log-large-array.any.js
  console/console-log-symbol.any.js
  console/console-namespace-object-class-string.any.js
  console/console-tests-historical.any.js
)

# encoding — a starter subset (full label-table suites left for later).
FILES+=(
  encoding/api-basics.any.js
  encoding/api-surrogates-utf8.any.js
  encoding/encodeInto.any.js
  encoding/textdecoder-arguments.any.js
  encoding/textdecoder-fatal.any.js
  encoding/textencoder-constructor-non-utf.any.js
  encoding/resources/encodings.js
)

# fetch/api/headers — pure-compute Headers tests (network ones excluded).
FILES+=(
  fetch/api/headers/header-setcookie.any.js
  fetch/api/headers/headers-basic.any.js
  fetch/api/headers/headers-casing.any.js
  fetch/api/headers/headers-combine.any.js
  fetch/api/headers/headers-errors.any.js
  fetch/api/headers/headers-normalize.any.js
  fetch/api/headers/headers-record.any.js
  fetch/api/headers/headers-structure.any.js
)

# wasm/jsapi — helpers + all suites that need neither SharedArrayBuffer
# helpers (/common/sab.js), JSPI, js-string builtins, nor idlharness.
FILES+=(
  wasm/jsapi/assertions.js
  wasm/jsapi/wasm-module-builder.js
  wasm/jsapi/instanceTestFactory.js
  wasm/jsapi/bad-imports.js
  wasm/jsapi/interface.any.js
  wasm/jsapi/prototypes.any.js
)
for dir in constructor exception function global instance memory module table tag; do
  while IFS= read -r f; do
    FILES+=("${f#"$WPT_SRC"/}")
  done < <(find "$WPT_SRC/wasm/jsapi/$dir" -name '*.any.js' | sort)
done
FILES+=(
  wasm/jsapi/memory/assertions.js
  wasm/jsapi/table/assertions.js
)

mkdir -p "$DEST"
for f in "${FILES[@]}"; do
  src="$WPT_SRC/$f"
  if [ ! -f "$src" ]; then
    echo "MISSING in checkout: $f" >&2
    exit 1
  fi
  mkdir -p "$DEST/$(dirname "$f")"
  cp "$src" "$DEST/$f"
done
# The vendored LICENSE keeps attribution obvious.
mv "$DEST/LICENSE.md" "$DEST/LICENSE" 2>/dev/null || true

COMMIT=$(git -C "$WPT_SRC" rev-parse HEAD)
cat > "$REPO_ROOT/server/tests/wpt/versions.json" <<EOF
{
  "wpt": {
    "commit": "$COMMIT",
    "repository": "https://github.com/web-platform-tests/wpt",
    "vendored_by": "tools/compat/vendor-wpt.sh"
  }
}
EOF

echo "Vendored ${#FILES[@]} files from wpt@$COMMIT into $DEST"
