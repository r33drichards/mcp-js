run javascript or typescript code in v8

Executes code and returns the console output directly. Each call runs in a fresh V8 isolate — no state is carried between calls.

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code (optional): the javascript or typescript code to run. Provide either `code` or `file`.
- file (optional): path to a JavaScript/TypeScript file **on the server's own filesystem** to read and execute instead of inline `code`. Provide either `code` or `file`, not both. This is disabled by default: the server must be started with `--allow-run-js-file` (allow any path) or a `run_js_file` policy in `--policies-json` (allow specific paths/dirs), otherwise the call is rejected. The path is resolved on the server, not uploaded from the client.
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (minimum: 4, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.

returns:
- output: console output from the execution (everything printed via console.log, console.info, console.warn, console.error)
- error: error message if the execution failed, timed out, or was cancelled

## Console Output

Use `console.log()` to produce output. `console.info`, `console.warn`, and `console.error` are also supported (with `[INFO]`, `[WARN]`, `[ERROR]` prefixes respectively).

eg:

```js
const result = 1 + 1;
console.log(result);
```

Returns `output: "2"`.

```js
const obj = { a: 1, b: 2 };
console.log(JSON.stringify(obj));
```

Returns `output: '{"a":1,"b":2}'`.

async/await is supported. The runtime resolves top-level Promises automatically.

## Importing Packages

You can import npm packages, JSR packages, and URL modules using ES module `import` syntax. Packages are fetched from esm.sh at runtime — no installation needed.

- **npm**: `import { camelCase } from "npm:lodash-es@4.17.21";`
- **jsr**: `import { camelCase } from "jsr:@luca/cases@1.0.0";`
- **URL**: `import { pascalCase } from "https://deno.land/x/case/mod.ts";`

Always pin versions for reproducible results. Dynamic `import()` is also supported with top-level `await`.

## Node Built-ins via require()

A CommonJS-style `require()` resolves a small set of Node built-ins — handy when reusing Node-flavored snippets:

- `require('path')` (or `'node:path'`) — a POSIX implementation of Node's `path` module
- `require('fs')` / `require('fs/promises')` — the sandbox `fs` module (see "Filesystem Access"); throws with instructions if the server has no filesystem policy

Anything else throws `MODULE_NOT_FOUND`. There is no CommonJS module resolution — for packages use the ES-module `import` syntax above.

## Filesystem Access

When the server is configured with policies, JavaScript code can use an `fs` module providing Node.js-compatible file operations. Every operation is evaluated against a Rego policy before execution.

**Available operations:**
- `await fs.readFile(path, [encoding])` — Read file as a `Uint8Array` (default, Node semantics) or a string (if a text encoding like `"utf8"` is given)
- `await fs.writeFile(path, data)` — Write string or `Uint8Array` to file
- `await fs.appendFile(path, data)` — Append data to file
- `await fs.readdir(path)` — List directory contents
- `await fs.stat(path)` — Get file metadata
- `await fs.mkdir(path, [options])` — Create directory (supports `{recursive: true}`)
- `await fs.rm(path, [options])` — Delete file or directory (supports `{recursive: true}`)
- `await fs.rename(oldPath, newPath)` — Rename or move file
- `await fs.copyFile(src, dest)` — Copy file
- `await fs.createWriteStream(path)` — Open a streaming write handle (`await w.write(chunk)`, `await w.close()`) for large files
- `await fs.exists(path)` — Check if path exists
- `await fs.unlink(path)` — Delete a file

All operations return Promises and are subject to Rego policy evaluation. Policy input includes `operation`, `path`, `destination` (for rename/copy), `recursive` (for mkdir/rm), and `encoding` (for readFile).

**Synchronous variants:** Node's `fs.*Sync` API is available too — `readFileSync`, `writeFileSync`, `appendFileSync`, `readdirSync`, `statSync`, `lstatSync`, `mkdirSync`, `rmSync`, `rmdirSync`, `unlinkSync`, `renameSync`, `copyFileSync`, `readlinkSync`, `symlinkSync`, `existsSync`. Sync variants are gated by the same policies under the same operation names (`readFileSync` is checked as `readFile`). Per Node's contract, `existsSync` never throws: errors and policy denials read as `false`.

## Limitations

- **No `fetch` or network access by default**: When the server is started with fetch policies configured via `--policies-json`, a `fetch(url, opts?)` function becomes available. `fetch()` follows the web standard Fetch API — it returns a Promise that resolves to a Response object. Use `await` to get the response: `const resp = await fetch(url)`. The response object has `.ok`, `.status`, `.statusText`, `.url`, `.headers.get(name)`, `.text()`, and `.json()` methods (`.text()` and `.json()` also return Promises). Each request is checked against policy before execution. If the server is also configured with `--fetch-header` or `--fetch-header-config`, matching requests may receive static headers or dynamically acquired OAuth client-credentials bearer tokens before policy evaluation. Headers set directly in JavaScript still win. Without fetch policies, there is no network access.
- **No file system access by default**: Filesystem access requires server configuration with policies. See "Filesystem Access" above.
- **No environment variables**: The runtime does not provide access to environment variables.
- **Timers**: `setTimeout`/`clearTimeout` are available; `setInterval` is not — use a loop with an awaited `setTimeout`.
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.

Each execution starts with a fresh V8 isolate — no state is carried between calls.
