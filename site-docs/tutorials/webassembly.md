# Tutorial: Running WebAssembly

In this tutorial you will learn how to run WebAssembly (WASM) modules inside mcp-v8. You will work with inline WASM bytes, pre-loaded WASM modules, and a practical SQLite WASM example.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- A `.wasm` file for the pre-loaded module steps (instructions to obtain one are included)

## Step 1: Understand WASM support in mcp-v8

mcp-v8 supports WebAssembly through two mechanisms:

1. **Inline WASM**: Compile and instantiate WASM bytes directly in JavaScript code
2. **Pre-loaded modules**: Pass WASM files at server startup with `--wasm-module`, making them available as globals

If a pre-loaded WASM module has no imports (it is self-contained), mcp-v8 auto-instantiates it and exposes its exports directly.

## Step 2: Run inline WASM bytes

You can compile and run WASM directly from JavaScript. Here is a minimal example that defines an `add` function in raw WASM bytes:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// Minimal WASM module that exports an "add" function
const wasmBytes = new Uint8Array([
  0x00, 0x61, 0x73, 0x6d, // magic number (\0asm)
  0x01, 0x00, 0x00, 0x00, // version 1
  // Type section: one function type (i32, i32) -> i32
  0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f,
  // Function section: one function using type 0
  0x03, 0x02, 0x01, 0x00,
  // Export section: export "add" as function 0
  0x07, 0x07, 0x01, 0x03, 0x61, 0x64, 0x64, 0x00, 0x00,
  // Code section: function body (local.get 0, local.get 1, i32.add)
  0x0a, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6a, 0x0b,
]);

const module = await WebAssembly.compile(wasmBytes);
const instance = await WebAssembly.instantiate(module);
instance.exports.add(40, 2);
'
```

Expected output:

```
42
```

## Step 3: Pre-load a WASM module at startup

For frequently used WASM modules, pre-load them when starting the server. First, stop any running server, then restart with the `--wasm-module` flag:

```bash
mcp-v8 --http-port 3000 --wasm-module calculator=/path/to/calculator.wasm
```

The format is `--wasm-module name=path`. The module is loaded once at startup and made available to all executions.

## Step 4: Use a pre-loaded module

If the WASM module has no imports (self-contained), mcp-v8 auto-instantiates it and exposes its exports on a global object:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// Access the pre-loaded and auto-instantiated module
const result = globalThis.calculator.add(10, 20);
result;
'
```

Expected output:

```
30
```

The module name you specified in `--wasm-module calculator=...` becomes the global variable name.

## Step 5: Use WASM configuration

For more complex WASM setups, use `--wasm-config` with a JSON configuration file:

Create a file called `wasm-config.json`:

```json
{
  "modules": {
    "calculator": {
      "path": "/path/to/calculator.wasm"
    },
    "crypto": {
      "path": "/path/to/crypto.wasm"
    }
  }
}
```

Start the server with this config:

```bash
mcp-v8 --http-port 3000 --wasm-config wasm-config.json
```

Both modules are now available as globals.

## Step 6: Run SQLite with WASM

A practical use case for WASM in mcp-v8 is running SQLite. You can load the SQLite WASM build:

```bash
mcp-v8 --http-port 3000 --wasm-module sqlite=/path/to/sqlite3.wasm
```

Then use it from JavaScript:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// Use the SQLite WASM module to create an in-memory database
// The exact API depends on the SQLite WASM build you are using

// Example with a typical SQLite WASM API:
const db = new globalThis.sqlite.Database(":memory:");

db.exec("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)");
db.exec("INSERT INTO users VALUES (1, '\''Alice'\'', 30)");
db.exec("INSERT INTO users VALUES (2, '\''Bob'\'', 25)");
db.exec("INSERT INTO users VALUES (3, '\''Charlie'\'', 35)");

const results = db.exec("SELECT * FROM users WHERE age > 28");
results;
'
```

This gives AI agents the ability to create and query databases entirely within the V8 sandbox.

## Step 7: WASM with imports

If a WASM module requires imports (it is not self-contained), mcp-v8 loads the module but does not auto-instantiate it. You need to provide the imports yourself in JavaScript:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// For WASM modules that require imports, you get the WebAssembly.Module
// and must instantiate it yourself with the required import object
const importObject = {
  env: {
    log: (value) => console.log("WASM says:", value),
    memory: new WebAssembly.Memory({ initial: 1 }),
  }
};

// If "mymodule" was loaded with --wasm-module but has imports,
// it is available as a WebAssembly.Module (not auto-instantiated)
const instance = await WebAssembly.instantiate(globalThis.mymodule, importObject);
instance.exports.run();
'
```

## What you learned

- How to compile and run inline WASM bytes in JavaScript
- How to pre-load WASM modules at server startup with `--wasm-module`
- That self-contained WASM modules are auto-instantiated and exposed as globals
- How to use `--wasm-config` for multiple WASM modules
- How to use SQLite WASM for in-memory databases
- That WASM modules with imports require manual instantiation in JavaScript

Next, learn about security policies in [Setting Up OPA Policies](policy-system.md).
