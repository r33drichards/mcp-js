#!/usr/bin/env bash
# Vendor a curated subset of Node.js core tests (test/parallel) into
# server/tests/node_compat/vendor/. Files are fetched from the nodejs/node
# repository at the pinned tag below (MIT license; LICENSE is vendored
# alongside). Chosen families: path, querystring, events — the least
# host-coupled suites, matching the node: compat modules served by the
# engine (see src/engine/node_compat.rs).
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
