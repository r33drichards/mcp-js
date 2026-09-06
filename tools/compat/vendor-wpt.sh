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


# Expanded coverage: streams, compression, url, urlpattern, FileAPI,
# formdata, WebCryptoAPI, dom/abort, dom/events, hr-time, structured-clone
# (selected mechanically with transitive META script deps).
FILES+=(
  FileAPI/blob/Blob-array-buffer.any.js
  FileAPI/blob/Blob-bytes.any.js
  FileAPI/blob/Blob-constructor-detached-buffer.any.js
  FileAPI/blob/Blob-constructor-endings.any.js
  FileAPI/blob/Blob-constructor.any.js
  FileAPI/blob/Blob-newobject.any.js
  FileAPI/blob/Blob-slice-overflow.any.js
  FileAPI/blob/Blob-slice.any.js
  FileAPI/blob/Blob-stream.any.js
  FileAPI/blob/Blob-text.any.js
  FileAPI/blob/Blob-textStream.any.js
  FileAPI/file/File-constructor-endings.any.js
  FileAPI/file/File-constructor.any.js
  FileAPI/support/Blob.js
  FileAPI/unicode.any.js
  WebCryptoAPI/digest/digest.https.any.js
  WebCryptoAPI/digest/digest.js
  WebCryptoAPI/digest/digest_test_data.js
  WebCryptoAPI/getRandomValues.any.js
  WebCryptoAPI/randomUUID.https.any.js
  WebCryptoAPI/util/helpers.js
  common/gc.js
  common/subset-tests-by-key.js
  compression/compression-bad-chunks.any.js
  compression/compression-constructor-error.any.js
  compression/compression-including-empty-chunk.any.js
  compression/compression-large-flush-output.any.js
  compression/compression-multiple-chunks.any.js
  compression/compression-output-length.any.js
  compression/compression-stream.any.js
  compression/compression-with-detach.any.js
  compression/decompression-bad-chunks.any.js
  compression/decompression-buffersource.any.js
  compression/decompression-constructor-error.any.js
  compression/decompression-correct-input.any.js
  compression/decompression-corrupt-input.any.js
  compression/decompression-empty-input.any.js
  compression/decompression-extra-input.any.js
  compression/decompression-split-chunk.any.js
  compression/decompression-uint8array-output.any.js
  compression/decompression-with-detach.any.js
  compression/resources/concatenate-stream.js
  compression/resources/decompress.js
  compression/resources/decompression-input.js
  compression/resources/formats.js
  compression/third_party/pako/pako_inflate.min.js
  dom/abort/AbortSignal.any.js
  dom/abort/abort-signal-any.any.js
  dom/abort/event.any.js
  dom/abort/resources/abort-signal-any-tests.js
  dom/abort/timeout.any.js
  dom/events/AddEventListenerOptions-once.any.js
  dom/events/AddEventListenerOptions-passive.any.js
  dom/events/AddEventListenerOptions-signal.any.js
  dom/events/Event-constructors.any.js
  dom/events/Event-isTrusted.any.js
  dom/events/EventTarget-add-remove-listener.any.js
  dom/events/EventTarget-addEventListener.any.js
  dom/events/EventTarget-constructible.any.js
  dom/events/EventTarget-removeEventListener.any.js
  hr-time/basic.any.js
  hr-time/monotonic-clock.any.js
  html/webappapis/structured-clone/structured-clone-battery-of-tests-harness.js
  html/webappapis/structured-clone/structured-clone-battery-of-tests-with-transferables.js
  html/webappapis/structured-clone/structured-clone-battery-of-tests.js
  html/webappapis/structured-clone/structured-clone.any.js
  streams/piping/abort.any.js
  streams/piping/close-propagation-backward.any.js
  streams/piping/close-propagation-forward.any.js
  streams/piping/error-propagation-backward.any.js
  streams/piping/error-propagation-forward.any.js
  streams/piping/flow-control.any.js
  streams/piping/general-addition.any.js
  streams/piping/general.any.js
  streams/piping/multiple-propagation.any.js
  streams/piping/pipe-through.any.js
  streams/piping/then-interception.any.js
  streams/piping/throwing-options.any.js
  streams/piping/transform-streams.any.js
  streams/queuing-strategies.any.js
  streams/readable-byte-streams/bad-buffers-and-views.any.js
  streams/readable-byte-streams/construct-byob-request.any.js
  streams/readable-byte-streams/enqueue-with-detached-buffer.any.js
  streams/readable-byte-streams/general.any.js
  streams/readable-byte-streams/non-transferable-buffers.any.js
  streams/readable-byte-streams/patched-global.any.js
  streams/readable-byte-streams/read-min.any.js
  streams/readable-byte-streams/respond-after-enqueue.any.js
  streams/readable-byte-streams/tee.any.js
  streams/readable-byte-streams/templated.any.js
  streams/readable-streams/async-iterator.any.js
  streams/readable-streams/bad-strategies.any.js
  streams/readable-streams/bad-underlying-sources.any.js
  streams/readable-streams/cancel.any.js
  streams/readable-streams/constructor.any.js
  streams/readable-streams/count-queuing-strategy-integration.any.js
  streams/readable-streams/default-reader.any.js
  streams/readable-streams/floating-point-total-queue-size.any.js
  streams/readable-streams/from.any.js
  streams/readable-streams/garbage-collection.any.js
  streams/readable-streams/general.any.js
  streams/readable-streams/owning-type-message-port.tentative.any.js
  streams/readable-streams/owning-type-video-frame.tentative.any.js
  streams/readable-streams/owning-type.tentative.any.js
  streams/readable-streams/patched-global.any.js
  streams/readable-streams/reentrant-strategies.any.js
  streams/readable-streams/tee.any.js
  streams/readable-streams/templated.any.js
  streams/resources/recording-streams.js
  streams/resources/rs-test-templates.js
  streams/resources/rs-utils.js
  streams/resources/test-utils.js
  streams/transferable/transform-stream-members.any.js
  streams/transform-streams/backpressure.any.js
  streams/transform-streams/cancel.any.js
  streams/transform-streams/errors.any.js
  streams/transform-streams/flush.any.js
  streams/transform-streams/general.any.js
  streams/transform-streams/lipfuzz.any.js
  streams/transform-streams/patched-global.any.js
  streams/transform-streams/properties.any.js
  streams/transform-streams/reentrant-strategies.any.js
  streams/transform-streams/strategies.any.js
  streams/transform-streams/terminate.any.js
  streams/writable-streams/aborting.any.js
  streams/writable-streams/bad-strategies.any.js
  streams/writable-streams/bad-underlying-sinks.any.js
  streams/writable-streams/byte-length-queuing-strategy.any.js
  streams/writable-streams/close.any.js
  streams/writable-streams/constructor.any.js
  streams/writable-streams/count-queuing-strategy.any.js
  streams/writable-streams/error.any.js
  streams/writable-streams/floating-point-total-queue-size.any.js
  streams/writable-streams/garbage-collection.any.js
  streams/writable-streams/general.any.js
  streams/writable-streams/properties.any.js
  streams/writable-streams/reentrant-strategy.any.js
  streams/writable-streams/start.any.js
  streams/writable-streams/write.any.js
  url/historical.any.js
  url/resources/setters_tests.json
  url/resources/urltestdata-javascript-only.json
  url/resources/urltestdata.json
  url/url-constructor.any.js
  url/url-origin.any.js
  url/url-searchparams.any.js
  url/url-setters-stripping.any.js
  url/url-setters.any.js
  url/url-statics-canparse.any.js
  url/url-statics-parse.any.js
  url/url-tojson.any.js
  url/urlencoded-parser.any.js
  url/urlsearchparams-append.any.js
  url/urlsearchparams-constructor.any.js
  url/urlsearchparams-delete.any.js
  url/urlsearchparams-foreach.any.js
  url/urlsearchparams-get.any.js
  url/urlsearchparams-getall.any.js
  url/urlsearchparams-has.any.js
  url/urlsearchparams-set.any.js
  url/urlsearchparams-size.any.js
  url/urlsearchparams-sort.any.js
  url/urlsearchparams-stringifier.any.js
  urlpattern/resources/urlpattern-hasregexpgroups-tests.js
  urlpattern/resources/urlpatterntestdata.json
  urlpattern/resources/urlpatterntests.js
  urlpattern/urlpattern-constructor.any.js
  urlpattern/urlpattern-hasregexpgroups.any.js
  urlpattern/urlpattern.any.js
  xhr/formdata/append.any.js
  xhr/formdata/constructor.any.js
  xhr/formdata/delete.any.js
  xhr/formdata/foreach.any.js
  xhr/formdata/get.any.js
  xhr/formdata/has.any.js
  xhr/formdata/iteration.any.js
  xhr/formdata/set-blob.any.js
  xhr/formdata/set.any.js
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
