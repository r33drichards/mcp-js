run javascript or typescript code in v8

Submits code for **async execution** — returns an execution ID immediately. V8 runs in the background. Use `get_execution` to poll status and result, `get_execution_output` to read console output, and `cancel_execution` to stop a running execution.

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code: the javascript or typescript code to run
- heap (optional): content hash (SHA-256 hex string) from a previous execution to resume that session, or omit for a fresh session
- session (optional): a human-readable session name. When provided, a log entry is written recording the input heap, output heap, executed code, and timestamp. Use list_sessions and list_session_snapshots to browse session history.
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (4–64, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.
- tags (optional): a JSON object of key-value string pairs to associate with the resulting heap snapshot. Tags can be used to label, categorize, or annotate heaps for later retrieval. Use get_heap_tags, set_heap_tags, delete_heap_tags, and query_heaps_by_tags to manage tags independently.

returns:
- execution_id: UUID of the submitted execution. Use with get_execution, get_execution_output, and cancel_execution.

## Workflow

1. Call `run_js(code)` → get `execution_id`
2. Call `get_execution(execution_id)` → check `status` (running/completed/failed/cancelled/timed_out)
3. Call `get_execution_output(execution_id)` → read console output (paginated)
4. When status is "completed", `get_execution` returns the `result` (final expression value) and `heap` (content hash)

## Console Output

`console.log`, `console.info`, `console.warn`, and `console.error` are fully supported. Output is streamed to persistent storage during execution and can be queried in real-time using `get_execution_output`.

Console output supports two pagination modes:
- **Line mode**: `line_offset` + `line_limit` — fetch N lines starting from line M
- **Byte mode**: `byte_offset` + `byte_limit` — fetch N bytes starting from byte M

Both modes return position info in both coordinate systems for cross-referencing. Use `next_line_offset` or `next_byte_offset` from a previous response to resume reading.

## Return Values

The final expression value (the last evaluated expression in your code) is available via `get_execution` once the execution completes. To return results, ensure the value you want is the last line of your code.

eg:

```js
const result = 1 + 1;
result;
```

After execution completes, `get_execution` will return `result: "2"`.

You must also jsonify an object, and return it as a string to see its content.

eg:

```js
const obj = {
  a: 1,
  b: 2,
};
JSON.stringify(obj);
```

After execution completes, `get_execution` will return `result: '{"a":1,"b":2}'`.

async/await is supported. The runtime resolves top-level Promises automatically.

## Importing Packages

You can import npm packages, JSR packages, and URL modules using ES module `import` syntax. Packages are fetched from esm.sh at runtime — no installation needed.

- **npm**: `import { camelCase } from "npm:lodash-es@4.17.21";`
- **jsr**: `import { camelCase } from "jsr:@luca/cases@1.0.0";`
- **URL**: `import { pascalCase } from "https://deno.land/x/case/mod.ts";`

Always pin versions for reproducible results. Dynamic `import()` is also supported with top-level `await`.

## Limitations

- **No `fetch` or network access by default**: When the server is started with `--opa-url`, a `fetch(url, opts?)` function becomes available. `fetch()` follows the web standard Fetch API — it returns a Promise that resolves to a Response object. Use `await` to get the response: `const resp = await fetch(url)`. The response object has `.ok`, `.status`, `.statusText`, `.url`, `.headers.get(name)`, `.text()`, and `.json()` methods (`.text()` and `.json()` also return Promises). Each request is checked against an OPA policy before execution. Without `--opa-url`, there is no network access.
- **No file system access by default**: When the server is started with `--opa-url`, a `fs` global becomes available with Node.js-compatible async methods. Every operation is checked against an OPA policy before execution. Available methods: `fs.readFile(path, encoding?)`, `fs.writeFile(path, data)`, `fs.appendFile(path, data)`, `fs.readdir(path)`, `fs.stat(path)`, `fs.mkdir(path, {recursive?})`, `fs.rm(path, {recursive?})`, `fs.unlink(path)`, `fs.rename(oldPath, newPath)`, `fs.copyFile(src, dest)`, `fs.exists(path)`. All methods return Promises. `readFile` returns a string (utf-8) by default, or a `Uint8Array` when called with `"buffer"` as encoding. `writeFile` accepts strings or `Uint8Array`. Without `--opa-url`, there is no file system access. Environment variables are not accessible.
- **No timers**: Functions like `setTimeout` and `setInterval` are not available.
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.

Each execution starts with a fresh V8 isolate — no state is carried between calls.

In stateful mode, each execution returns a SHA-256 content hash for the heap snapshot — pass it back as the `heap` parameter in the next call to resume from that state. Omit `heap` for a fresh session.
