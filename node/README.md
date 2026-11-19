# @wholelottahoopla/mcp-js-node

Native embedded mcp-js engine for Node.js 22+, using the pinned `@ubjs/node`
Node-API runtime. This is an ESM package with JavaScript and TypeScript
declarations, not an HTTP client, Python bridge, or runtime CLI wrapper.

## Supported target and release status

This packaging path targets **Linux x64 with glibc 2.39 or newer only**. macOS, Windows,
ARM64 and musl/Alpine are not supported by this package. Tagged release builds
replace the development version in the manifest with the tag version.

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

## Install from GitHub Releases

On a release that includes SDK assets, replace `VERSION` with the version
listed on the release, including any prerelease suffix:

```sh
npm install '@wholelottahoopla/mcp-js-node@https://github.com/r33drichards/mcp-js/releases/download/vVERSION/wholelottahoopla-mcp-js-node-VERSION.tar.gz'
```

The package embeds the engine in-process through UniFFI. Use a compatible
Linux x64 glibc host. For remote execution, use the
[HTTP client](../clients/typescript/README.md) together with a server.

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

## Bind host tools to guest JavaScript

`createToolRuntime(engine, config)` binds one tool catalog and dispatcher.
Reuse the wrapper for calls that share the same permissions. For an app with
request-specific tools, create one wrapper per request. Each run has its own
request channel and can make up to 32 host tool calls. Internally,
`runJs` uses `ToolCallRequestBuilder` to attach that channel to the regular
`run_js` request and submits it through `engine.invokeTool(request)`.
The dispatcher receives
an AbortSignal that is aborted when the run ends. Pass an AbortSignal as the
second argument to `runJs` to cancel the execution. Validate arguments and
enforce permissions in the dispatcher.

```js
import { Engine, createToolRuntime } from '@wholelottahoopla/mcp-js-node';

const engine = Engine.createStateless(64n, 5n);
try {
  const runtime = createToolRuntime(engine, {
    server: "host",
    tools: [{
      name: "echo",
      description: "Echo a message",
      inputSchema: {
        type: "object",
        properties: { message: { type: "string" } },
        required: ["message"],
      },
    }],
    call(name, args) {
      if (name !== "echo" || typeof args.message !== "string") {
        throw new Error("Invalid echo request");
      }
      return { message: args.message };
    },
  });

  // Advertise these discovery stubs alongside your model's run_js tool.
  console.log(runtime.listTools());

  const result = await runtime.runJs(`
    const reply = await mcp.callTool("host", "echo", { message: "hello" });
    console.log(JSON.parse(reply.content[0].text).message);
  `);
  console.log(result.output);
} finally {
  engine.close();
  engine.uniffiDestroy();
}
```

Guest code can also discover tools with `mcp.listTools("host")`. The stub
definitions use the server's stub format and default `runjs__` prefix. Set
`stubs: false` to hide them, or set `stubPrefix` to change the prefix.
`runtime.stubInstructions(name, args)` returns the server-style redirect for a
direct call to a known stub. The wrapper runs on engines without heap snapshots or upstream MCP.
Host requests and JSON responses are limited to 256 KiB each.

To use the same MCP authorization and hooks as the standalone server, set
`EngineConfigBuilder.mcpToolsPoliciesJson(JSON.stringify(config))`. Pass the
contents of the server's `mcp_tools` config entry. The shared chain runs the
pre-hooks and policy before dispatch, and post-hooks can redact or change the
MCP result. The same setting also accepts the server's layered `stack` form.

```js
const config = new EngineConfigBuilder()
  .limits(new ExecutionLimitsBuilder().heapMemoryMaxMb(64n).executionTimeoutSecs(5n).build())
  .mcpToolsPoliciesJson(JSON.stringify({
    policies: [{ url: "file:///etc/mcp/mcp_tools.rego" }],
  }))
  .build();
const engine = Engine.create(config);
```

## Configuring an engine

`Engine.create(config)` takes an `EngineConfig` assembled with builders. Every
configuration record has a `<Record>Builder` with chainable setters and a
`build()` that throws `RuntimeError.MissingRequiredField` naming the first
unset required field, the same shape in every generated language:

```ts
const config = new EngineConfigBuilder()
  .limits(new ExecutionLimitsBuilder().heapMemoryMaxMb(64n).executionTimeoutSecs(5n).build())
  .dataDir("/var/lib/pi/mcp-js")                         // omit for a temporary directory
  .filesystem(new FilesystemAccessBuilder().policiesJson(policies).build())
  .heapStore(new BlobStoreBuilder().backend(StoreBackend.Directory).build())
  .fsSnapshotStore(new BlobStoreBuilder().backend(StoreBackend.Directory).build())
  .build();
const engine = Engine.create(config);
```

The axes are independent, as they are for the server: limits are required;
`filesystem` enables hook-gated `fs.*` and the native `fs*` methods; `heapStore`
enables V8 heap persistence (`run_js` accepts and reports content-addressed
`heap` hashes); `fsSnapshotStore` enables content-addressed filesystem
snapshots with labels; `wasmModules` pre-loads WebAssembly modules as globals. Directory stores default to paths under `dataDir`; S3
stores take a `bucket` and an optional cache `path`, and both axes must share
them. WASM modules cannot be combined with heap persistence: heap snapshots
bake compiled modules in, so `create` rejects `wasmModules` together with
`heapStore`.
`createStateless(...)` and `createWithFilesystem(...)` remain as conveniences
over `create`.

## Host filesystem access

`Engine.createWithFilesystem(heapMb, timeoutSecs, filesystemJson)` enables
hook/policy-gated host filesystem access and nothing else (no subprocess,
network, or module imports). `filesystemJson` is the `filesystem` entry of
`--policies-json`, so `policies`, `pre` hooks, and `stack` layers behave
exactly as they do for the server; a configuration without any of them is
rejected. Guest code then has `fs.*`, and the same engine exposes typed
native methods that run through the same hook chain, with no JavaScript
evaluation, no JSON or base64 encoding of file bytes, and structured errors:

```ts
const engine = Engine.createWithFilesystem(64n, 5n, JSON.stringify({
  policies: [{ url: "file:///etc/policies/filesystem.rego" }],
}));
await engine.fsWriteFile("/work/data.bin", new Uint8Array([1, 2, 3]));
const bytes = new Uint8Array(await engine.fsReadFile("/work/data.bin"));
const text = await engine.fsReadTextFile("/work/notes.txt");
const stat = await engine.fsStat("/work/notes.txt"); // { kind, size, readonly, mode, modifiedMs }
const names = await engine.fsReadDir("/work");
await engine.fsAppendFile("/work/notes.txt", new TextEncoder().encode("\n"));
await engine.fsMakeDir("/work/out", true);
await engine.fsRename("/work/notes.txt", "/work/out/notes.txt");
await engine.fsRemove("/work/out", true);
await engine.fsExists("/work/out"); // false
```

`fsLstat`, `fsReadLink`, `fsReadFileRange(path, offset, maxBytes)` for paging
through large files, and `fsCanonicalPath` (gated as a `stat`) complete the set. A pre hook that rewrites `path`
or `destination` applies to native calls exactly as to guest `fs.*` calls.
Failures reject with `RuntimeError.FileSystem`, carrying a `kind`
(`NotFound`, `PermissionDenied`, `AlreadyExists`, `NotDirectory`,
`IsDirectory`, `NotEmpty`, `InvalidData`, `NotSupported`, `Other`) and the
same message the guest wrapper would report, including its Node-style code
token. Engines created with `createStateless` reject native filesystem calls;
`hostFilesystemEnabled()` reports which kind you have. Overlay-backed
(session snapshot) engines are not supported by the native methods.

`tests/filesystem.test.ts` covers policy denial, hook-only configuration with a
path rewrite, guest/native parity on the same bytes, and each typed failure.

### Session file views

`engine.fsView(session)` returns an `FsView` with the same methods (`readFile`,
`readFileRange`, `readTextFile`, `writeFile`, `appendFile`, `stat`, `lstat`,
`readDir`, `readLink`, `canonicalPath`, `makeDir`, `remove`, `rename`,
`exists`). `fsView(undefined)` is the host filesystem; the engine's `fs*`
methods are shortcuts for it. `fsView("my-session")` is that session's
filesystem snapshot on an engine with `fsSnapshotStore`: the same snapshot
`run_js` mounts when called with that session id (the third `callToolAsync`
argument), so a native write is visible to the next run and a guest write to
the next native read. Each mutating call folds the change into a new snapshot
recorded in the session log with the session's current heap, exactly as a run
does; `listSessionSnapshots(session)` shows the chain. Binding a client session
to engine state therefore needs only the session name: `run_js` resumes the
latest heap and snapshot from the session log, and `awaitExecution(id)` returns
the heap and fs ids an execution produced when a caller submits through the
execution API instead.

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
