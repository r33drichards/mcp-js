# Node.js native UniFFI E2E

Run JavaScript in the embedded mcp-js engine from a Node.js TypeScript program.
This uses `uniffi-bindgen-react-native` and its `@ubjs/node` Node-API runtime to
load the same native shared library used by Python. There is no HTTP server,
Python bridge, per-engine handwritten Node addon, or runtime subprocess.

## Run locally (Linux x86_64)

Build the shared library using the existing Python-loadable packaging crate;
despite its name it exposes the canonical language-independent UniFFI engine:

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
npm ci --prefix node
nix develop --command bash node/scripts/generate.sh \
  target/python-uniffi/release/libmcp_v8_uniffi.so
npm run typecheck --prefix node
timeout 90s npm test --prefix node
```

The generated bindings live in ignored `node/generated/` and contain the
absolute library path. Regenerate after moving or rebuilding the library.
This is a tested integration example, not a published portable npm package.

The program in `tests/engine.test.ts` uses this API:

```ts
import { Engine } from "./generated/index";

const engine = Engine.createStateless(64n, 1n);
try {
  const result = JSON.parse(await engine.callToolAsync(
    "run_js", JSON.stringify({ code: "console.log(6 * 7)" }),
    undefined, undefined,
  ));
  console.log(result.output); // 42
} finally {
  engine.close();
  if (Engine.instanceOf(engine)) engine.uniffiDestroy();
}
```

`callToolAsync` returns a Promise and does not block the Node event loop. The
same positional arguments are accepted by synchronous `callTool`, which is
useful in scripts but blocks the host thread. The native engine awaits Promises
in the executed JavaScript. Execution errors and deadlines are returned in
JSON; native API failures reject the Promise (or throw from `callTool`).

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
snapshots with labels. Directory stores default to paths under `dataDir`; S3
stores take a `bucket` and an optional cache `path`, and both axes must share
them. WASM modules cannot be combined with heap persistence.
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

## Generator compatibility

The upstream release `0.31.0-5` (commit
`0f7fc67e8dd98ad43af3787d45c3c570d469a145`) targets UniFFI 0.31, while mcp-js
uses 0.32. `patches/ubrn-uniffi-0.32.patch` adapts that pinned generator and its
Cargo lockfile to 0.32's metadata/pipeline APIs. It preserves the generated
contract-version and API-checksum validation; it does not downgrade or patch
the engine's ABI. New 0.32 Box/Set types are explicitly rejected by this
limited compatibility patch (the engine API does not use them).

The patch modifies MPL-2.0 upstream source; upstream license notices remain
intact. The Node runtime packages are pinned to the matching upstream release.

## CI coverage

`.github/workflows/node-uniffi-e2e.yml` builds the real native library, generates
and typechecks bindings, then runs Node with a hard process deadline. Assertions
cover `42`, guest Promise awaiting, JavaScript exceptions, native timeout and
recovery, lifecycle enums, idempotent shutdown, execution after shutdown, and
invalid limits. Subprocess APIs are disabled during imports and engine calls.
There are no mocked execution results, skipped native tests, or forced-success
exits; a crash or shutdown hang fails the job.
