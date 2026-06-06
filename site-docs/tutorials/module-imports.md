# ES module imports

This tutorial walks through enabling ES module imports and running JavaScript code inside mcp-v8 that fetches and uses an external package.

## Prerequisites

- mcp-v8 installed (`nix run github:r33drichards/mcp-js`, `install.sh`, or Docker).
- `mcp-v8-cli` installed for sending tool calls.

## Step 1 — Start the server with external modules enabled

By default, all ES module imports are blocked. Pass `--allow-external-modules` to lift that restriction:

```bash
mcp-v8 --http-port=3000 --allow-external-modules
```

You should see the following log line confirming the flag took effect:

```
External module imports: ENABLED
```

## Step 2 — Run JS that imports an npm package

We'll import `uuid` from [esm.sh](https://esm.sh) using the `npm:` specifier, which the module loader automatically resolves to `https://esm.sh/uuid`.

Use `mcp-v8-cli` to call the `run_js` tool:

```bash
mcp-v8-cli --url http://localhost:3000 run_js \
  --code 'import { v4 as uuidv4 } from "npm:uuid"; console.log(uuidv4());'
```

Expected output (your UUID will differ):

```
3d2f1a0e-8b4c-47de-a91f-0c5e9d123456
```

## Step 3 — Import a JSR package

`jsr:` specifiers resolve to `https://esm.sh/jsr/<package>`:

```bash
mcp-v8-cli --url http://localhost:3000 run_js \
  --code 'import { camelCase } from "jsr:@luca/cases"; console.log(camelCase("hello world"));'
```

Expected output:

```
helloWorld
```

## Step 4 — Import directly from a URL

You can also import by full URL:

```bash
mcp-v8-cli --url http://localhost:3000 run_js \
  --code 'import { format } from "https://esm.sh/date-fns"; console.log(format(new Date("2025-01-01"), "yyyy-MM-dd"));'
```

Expected output:

```
2025-01-01
```

## Step 5 — Verify that the default server blocks imports

Start a second server on a different port *without* the flag:

```bash
mcp-v8 --http-port=3001
```

Attempt the same import:

```bash
mcp-v8-cli --url http://localhost:3001 run_js \
  --code 'import { v4 } from "npm:uuid"; console.log(v4());'
```

The execution fails with:

```
External module imports are disabled. Cannot import npm package 'npm:uuid'.
Start the server with --allow-external-modules to enable.
```

This confirms that the default posture is deny-all.

## See also

- [How-to: ES module imports](../how-to/module-imports.md)
- [Concepts: ES module imports](../concepts/module-imports.md)
- [Reference: ES module imports](../reference/module-imports.md)
- [How-to: Security policies](../how-to/policies.md)
- [Quick-start: Running JavaScript & TypeScript](../tutorials/js-execution.md)
- [Quick-start: WebAssembly modules](../tutorials/wasm-modules.md)
