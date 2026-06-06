# WebAssembly modules — how-to

Recipes for loading WASM modules into mcp-v8, controlling memory caps, and using
the bundled SQLite example.

## Load a module via --wasm-module

```bash
mcp-v8 --wasm-module mylib=/path/to/mylib.wasm
```

The flag is repeatable; add it once per module:

```bash
mcp-v8 \
  --wasm-module math=/opt/wasm/math.wasm \
  --wasm-module codec=/opt/wasm/codec.wasm
```

## Load modules via --wasm-config

For more than a handful of modules, use a JSON config file instead of multiple
flags:

```bash
mcp-v8 --wasm-config /etc/mcp-v8/wasm.json
```

Each key in the JSON object is a module name; each value is either a plain path
string or an object that also sets a per-module memory cap:

```json
{
  "math": "/opt/wasm/math.wasm",
  "sqlite": {
    "path": "/opt/wasm/sqlite3.wasm",
    "max_memory_bytes": 33554432
  }
}
```

You can combine `--wasm-module` and `--wasm-config` freely. Duplicate names across
either source are rejected at startup.

## Set a per-module memory cap

Append `:max_memory` to the path in the `--wasm-module` flag:

```bash
# 32 MiB cap for this module only
mcp-v8 --wasm-module heavy=/opt/wasm/heavy.wasm:32m
```

Accepted size suffixes: `k`/`K` (kibibytes), `m`/`M` (mebibytes), `g`/`G`
(gibibytes). A plain integer is treated as bytes.

The cap is enforced as a structural pre-execution check: the module's declared
initial memory page count and initial table element count must fit within the
budget. Modules that exceed it are rejected before any V8 code runs.

## Set the default memory cap

`--wasm-default-max-memory` applies to every module that does not have its own
per-module cap. The server default is `16m`.

```bash
mcp-v8 --wasm-default-max-memory 32m \
       --wasm-module math=/opt/wasm/math.wasm \
       --wasm-module heavy=/opt/wasm/heavy.wasm:64m
```

In this example `math` is checked against 32 MiB and `heavy` against 64 MiB.

## Build and use the SQLite example

The repository ships a ready-made Emscripten build script for SQLite.

**Prerequisites:** [Emscripten SDK](https://emscripten.org/docs/getting_started/downloads.html)
installed and activated (`source /path/to/emsdk_env.sh`).

```bash
cd examples/sqlite-wasm
bash build.sh
```

The script downloads the SQLite 3.49.1 amalgamation and compiles it to
`examples/sqlite-wasm/sqlite3.wasm` with `STANDALONE_WASM=1` and
`ALLOW_MEMORY_GROWTH=1`.

Start mcp-v8 with the resulting module:

```bash
mcp-v8 --stateless \
       --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

Because the SQLite WASM module has WASI imports, mcp-v8 places it under the global
`__wasm_sqlite` (a compiled `WebAssembly.Module` object) rather than
auto-instantiating it. Your JS code must provide the required WASI stubs and
instantiate it manually:

```js
// Enumerate what the module needs
var moduleImports = WebAssembly.Module.imports(__wasm_sqlite);

// Build an import object with stubs for each import
var importObject = {};
for (var i = 0; i < moduleImports.length; i++) {
  var imp = moduleImports[i];
  if (!importObject[imp.module]) importObject[imp.module] = {};
  if (imp.kind === "function") {
    importObject[imp.module][imp.name] = function () { return 0; };
  }
}

var instance = new WebAssembly.Instance(__wasm_sqlite, importObject);
var exports = instance.exports;
// exports.sqlite3_open, exports.malloc, etc. are now available
```

The complete working example — including proper WASI stubs, a UTF-8
encoder/decoder, and a thin `SQLite` wrapper class — lives in
`examples/sqlite-wasm/example.js`. Send its contents as the `code` parameter of
`run_js` to run it end-to-end.

## See also

- [WebAssembly modules — concepts](../concepts/wasm-modules.md)
- [WebAssembly modules reference](../reference/wasm-modules.md)
- [CLI flags reference](../reference/cli-flags.md)
- [ES module imports](../how-to/module-imports.md)
