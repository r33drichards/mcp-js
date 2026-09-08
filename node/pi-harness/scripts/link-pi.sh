#!/usr/bin/env bash
# Link @earendil-works/pi-agent-core from a pi checkout into this package.
#
# A symlink, not an npm `file:` dependency: npm would otherwise resolve the
# linked package's own workspace dependencies (@earendil-works/pi-ai, chord,
# telemetry) from the registry instead of from the checkout. Node resolves a
# symlinked package from its real path, so pi-agent-core's imports resolve
# against the checkout's hoisted node_modules and built workspace packages.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
checkout="${PI_CHECKOUT:-$here/../../pi-checkout}"
package="$checkout/packages/agent"
[[ -f "$package/package.json" ]] || { echo "No pi checkout at $checkout (set PI_CHECKOUT)" >&2; exit 1; }
[[ -f "$package/dist/node.js" ]] || { echo "pi is not built: $package/dist/node.js is missing; run npm run build in the checkout" >&2; exit 1; }
mkdir -p "$here/node_modules/@earendil-works"
ln -sfn "$(cd "$package" && pwd)" "$here/node_modules/@earendil-works/pi-agent-core"
echo "linked @earendil-works/pi-agent-core -> $package"
