# WebAssembly modules

We'll compile a small WebAssembly module, load it into mcp-v8 with a single flag,
and call one of its exported functions from JavaScript in under five minutes.

## 1. Create a WAT source file

Save the following as `math.wat`. The module exports one function and has no
imports, so mcp-v8 will automatically instantiate it.

```wat
(module
  (func (export "add") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add)
)
```

## 2. Compile to WASM

Install [WABT](https://github.com/WebAssembly/wabt) if you don't have it, then
compile:

```bash
wat2wasm math.wat -o math.wasm
```

## 3. Start mcp-v8 with the module

Pass `--wasm-module name=path` to register the module. The name you choose becomes
a JavaScript global.

```bash
mcp-v8 --stateless --wasm-module math=math.wasm
```

mcp-v8 validates the binary, confirms the declared memory fits within the 16 MiB
default cap, and — because `math.wasm` has no imports — automatically instantiates
it. The module's `exports` object is available as the global `math` in every
execution.

## 4. Call the export from JavaScript

Send the `run_js` tool the following code:

```js
console.log(math.add(21, 21));
```

Expected output:

```
42
```

No `import` statement is needed. The `math` global is already in scope when your
code starts running.

## See also

- [How to use WebAssembly modules](../how-to/wasm-modules.md)
- [WebAssembly modules — concepts](../concepts/wasm-modules.md)
- [WebAssembly modules reference](../reference/wasm-modules.md)
- [Running JavaScript & TypeScript](../tutorials/js-execution.md)
- [CLI flags reference](../reference/cli-flags.md)
