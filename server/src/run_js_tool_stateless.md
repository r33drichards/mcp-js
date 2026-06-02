run javascript or typescript code in v8

Executes code and returns the console output directly. Each call runs in a fresh V8 isolate — no state is carried between calls.

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code: the javascript or typescript code to run
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

## Filesystem Access

When the server is configured with policies, JavaScript code can use an `fs` module providing Node.js-compatible file operations. Every operation is evaluated against a Rego policy before execution.

**Available operations:**
- `await fs.readFile(path, [encoding])` — Read file as UTF-8 string (default) or `Uint8Array` (if `encoding="buffer"`)
- `await fs.writeFile(path, data)` — Write string or `Uint8Array` to file
- `await fs.appendFile(path, data)` — Append data to file
- `await fs.readdir(path)` — List directory contents
- `await fs.stat(path)` — Get file metadata
- `await fs.mkdir(path, [options])` — Create directory (supports `{recursive: true}`)
- `await fs.rm(path, [options])` — Delete file or directory (supports `{recursive: true}`)
- `await fs.rename(oldPath, newPath)` — Rename or move file
- `await fs.copyFile(src, dest)` — Copy file
- `await fs.exists(path)` — Check if path exists
- `await fs.unlink(path)` — Delete a file

All operations return Promises and are subject to Rego policy evaluation. Policy input includes `operation`, `path`, `destination` (for rename/copy), `recursive` (for mkdir/rm), and `encoding` (for readFile).

## Limitations

- **No `fetch` or network access by default**: When the server is started with fetch policies configured via `--policies-json`, a `fetch(url, opts?)` function becomes available. `fetch()` follows the web standard Fetch API — it returns a Promise that resolves to a Response object. Use `await` to get the response: `const resp = await fetch(url)`. The response object has `.ok`, `.status`, `.statusText`, `.url`, `.headers.get(name)`, `.text()`, and `.json()` methods (`.text()` and `.json()` also return Promises). Each request is checked against policy before execution. If the server is also configured with `--fetch-header` or `--fetch-header-config`, matching requests may receive static headers or dynamically acquired OAuth client-credentials bearer tokens before policy evaluation. Headers set directly in JavaScript still win. Without fetch policies, there is no network access.
- **No file system access by default**: Filesystem access requires server configuration with policies. See "Filesystem Access" above.
- **No environment variables**: The runtime does not provide access to environment variables.
- **Limited timers**: `setTimeout(callback, delayMs)` and `clearTimeout(id)` are available and always enabled. `setInterval` / `clearInterval` are not. Timer callbacks only fire while the execution is still running (within the execution timeout window).
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.

When the server is started with a subprocess policy (via `--policies-json`), a `Deno.Command` constructor and a `child_process.exec(command, opts?)` function become available for running OS commands, each checked against the subprocess Rego policy. When the server is configured with upstream MCP servers (`--mcp-server` / `--mcp-config`), a `mcp` global is available (`mcp.callTool`, `mcp.listTools`, `mcp.servers`), gated by the optional MCP tools policy.

Each execution starts with a fresh V8 isolate — no state is carried between calls.
