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
  const result = JSON.parse(engine.callTool(
    "run_js", JSON.stringify({ code: "console.log(6 * 7)" }),
    undefined, undefined,
  ));
  console.log(result.output); // 42
} finally {
  engine.close();
  engine.uniffiDestroy();
}
```

`callTool` is synchronous and blocks the Node event loop. Use a dedicated
worker for production workloads that must not block their host thread.
The native engine awaits Promises in the executed JavaScript. Execution errors
and deadlines are returned in JSON; native API failures throw.

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
