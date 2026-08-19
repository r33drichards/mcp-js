#!/usr/bin/env bash
# Vendor a curated subset of Node.js core tests (test/parallel) into
# server/tests/node_compat/vendor/. Files are fetched from the nodejs/node
# repository at the pinned tag below (MIT license; LICENSE is vendored
# alongside). Chosen families: path, process, querystring, events, timers,
# console, crypto — the least host-coupled suites, matching the node: compat modules
# served by the engine (see src/engine/node_compat.rs). Files that reach
# into node-private surface (require('internal/...'), internalBinding,
# process.on('exit') assertions, child_process, fixtures) don't fit the
# harness and are not vendored.
set -euo pipefail

TAG=v22.14.0
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEST="$REPO_ROOT/server/tests/node_compat/vendor"
BASE="https://raw.githubusercontent.com/nodejs/node/$TAG"

FILES=(
  test/parallel/test-path-basename.js
  test/parallel/test-path-dirname.js
  test/parallel/test-path-extname.js
  test/parallel/test-path-isabsolute.js
  test/parallel/test-path-join.js
  test/parallel/test-path-normalize.js
  test/parallel/test-path-parse-format.js
  test/parallel/test-path-relative.js
  test/parallel/test-path-resolve.js
  test/parallel/test-path-zero-length-strings.js
  test/parallel/test-path.js
  test/parallel/test-process-getactiveresources-track-interval-lifetime.js
  test/parallel/test-process-getactiveresources.js
  test/parallel/test-querystring-escape.js
  test/parallel/test-querystring-multichar-separator.js
  test/parallel/test-querystring.js
  test/parallel/test-event-emitter-add-listeners.js
  test/parallel/test-event-emitter-emit-context.js
  test/parallel/test-event-emitter-errors.js
  test/parallel/test-event-emitter-get-max-listeners.js
  test/parallel/test-event-emitter-listener-count.js
  test/parallel/test-event-emitter-listeners.js
  test/parallel/test-event-emitter-max-listeners.js
  test/parallel/test-event-emitter-method-names.js
  test/parallel/test-event-emitter-num-args.js
  test/parallel/test-event-emitter-once.js
  test/parallel/test-event-emitter-prepend.js
  test/parallel/test-event-emitter-remove-all-listeners.js
  test/parallel/test-event-emitter-subclass.js
  test/parallel/test-events-once.js
  test/parallel/test-timers-args.js
  test/parallel/test-timers-api-refs.js
  test/parallel/test-timers-clear-null-does-not-throw-error.js
  test/parallel/test-timers-clearImmediate.js
  test/parallel/test-timers-immediate.js
  test/parallel/test-timers-non-integer-delay.js
  test/parallel/test-timers-zero-timeout.js
  test/parallel/test-console-instance.js
  test/parallel/test-crypto-randomuuid.js
  LICENSE
)

CA_ARGS=()
if [ -f /root/.ccr/ca-bundle.crt ]; then
  CA_ARGS=(--cacert /root/.ccr/ca-bundle.crt)
fi

mkdir -p "$DEST"
for f in "${FILES[@]}"; do
  mkdir -p "$DEST/$(dirname "$f")"
  curl -sSf "${CA_ARGS[@]}" "$BASE/$f" -o "$DEST/$f"
done

cat > "$DEST/../versions.json" <<EOF
{
  "node": {
    "tag": "$TAG",
    "repository": "https://github.com/nodejs/node",
    "vendored_by": "tools/compat/vendor-node-tests.sh"
  }
}
EOF

echo "Vendored ${#FILES[@]} files from node@$TAG into $DEST"
