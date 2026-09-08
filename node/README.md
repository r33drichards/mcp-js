# @wholelottahoopla/mcp-js-node

Native embedded mcp-js engine for Node.js 22+, using the pinned `@ubjs/node`
Node-API runtime. This is an ESM package with JavaScript and TypeScript
declarations, not an HTTP client, Python bridge, or runtime CLI wrapper.

## Supported target and release status

This packaging path targets **Linux x64 with glibc only**. macOS, Windows,
ARM64 and musl/Alpine are not supported by this package. No registry publication
is performed by the build or CI. Version 0.1.0 is initial package metadata, not
an assertion that a registry release exists.

Colocated loading makes the package movable; it does **not** make the shared
library self-contained. The engine still needs compatible glibc, libstdc++,
and any other ELF dependencies on the consumer host. CI targets Ubuntu 24.04;
no older glibc baseline or broader Linux compatibility is claimed. A source
build using Nix can carry Nix-store RPATHs and dependencies. `prepack` rejects
all RPATH/RUNPATH entries, path-based DT_NEEDED entries, Nix-store resolutions,
and unresolved dependencies. Such a build is not release-ready. Rebuild for
the intended distribution baseline or deliberately stage compatible dependencies
with appropriate licensing, then rerun all gates. Do not bypass the checks with
`--ignore-scripts`; removing RPATH alone does not establish portability.

## Build and validate

From the repository root:

```sh
nix develop --command bash -c '
  unset RUSTY_V8_ARCHIVE
  export V8_FROM_SOURCE=1
  export GN_ARGS="v8_monolithic=true v8_monolithic_for_shared_library=true"
  export CARGO_TARGET_DIR="$PWD/target/python-uniffi"
  cargo build -p mcp-v8-uniffi-python --release \
    --config mcp-v8-uniffi-python/cargo-config.toml
'
nix develop --command bash node/scripts/build-bindgen.sh
npm ci --include=dev --prefix node
npm run test:unit --prefix node
nix develop --command bash node/scripts/generate.sh \
  target/python-uniffi/release/libmcp_v8_uniffi.so
npm run typecheck --prefix node
timeout 90s npm test --prefix node
npm run build --prefix node
# Run outside nix develop, with readelf/ldd available on the target host:
npm run test:package --prefix node
# Keep a tarball only after the checks above pass:
(cd node && npm pack)
```

Generation clears stale generated/dist outputs and copies the real shared
library next to generated sources. The build compiles ESM and declarations,
fixes generator-emitted extensionless imports for Node ESM, and copies the
library into `dist/`. Neither generated source nor build output is committed.
`prepack` refuses missing/non-x64/non-ELF binaries and additionally invokes the
real engine API, so a file with a plausible header is not enough. The package
allowlist includes only `dist/`, README, package metadata and the root AGPL
license (copied verbatim to `node/LICENSE`). Build and pack require the source
checkout; consumers do not run a native compilation or install hook.

The tarball test accepts both legacy array and npm 12 package-keyed `pack
--json` output, checks payload contents, installs in an unrelated temporary
directory without install scripts, typechecks a NodeNext consumer, hides the
checkout's generated/dist directories, and executes JavaScript via the installed
package without tsx. A hard deadline catches crashes/hangs; there is no mock
native success path. CI retains the original full native lifecycle E2E test
and adds these packaging checks. Unit tests alone are not native validation.

## API

```js
import { Engine } from '@wholelottahoopla/mcp-js-node';

const engine = Engine.createStateless(64n, 1n);
try {
  const result = JSON.parse(await engine.callToolAsync(
    "run_js", JSON.stringify({ code: "console.log(6 * 7)" }),
    undefined, undefined,
  ));
  console.log(result.output); // 42
} finally {
  engine.close();
  engine.uniffiDestroy();
}
```

`callToolAsync` returns a Promise and does not block the Node event loop. The
same positional arguments are accepted by synchronous `callTool`, which is
useful in scripts but blocks the host thread. The native engine awaits Promises
in the executed JavaScript. Execution errors and deadlines are returned in
JSON; native API failures reject the Promise (or throw from `callTool`).

## Generator compatibility and licensing

The generator is pinned at `0f7fc67e8dd98ad43af3787d45c3c570d469a145`
(upstream release `0.31.0-5`), with matching `@ubjs/core` and `@ubjs/node`.
Verified in that commit: `crates/ubrn_cli/src/napi/generate.rs` defines
`--lib-colocated`; `wrapper-ffi-player.ts` emits `resolveLibPath` with
`callerUrl: import.meta.url` and no absolute override; the runtime's
`runtimes/napi/typescript/src/resolve-lib.ts` resolves the platform library
beside that caller. No unsupported generator flags are assumed.

`patches/ubrn-uniffi-0.32.patch` adapts the generator and lockfile to the engine's
UniFFI 0.32 metadata APIs while preserving ABI contract/checksum validation.
Box/Set types remain explicitly unsupported. This patch modifies MPL-2.0
upstream source and retains its notices. The engine/package uses the root
AGPL-3.0 license; dependencies retain their own licenses.
