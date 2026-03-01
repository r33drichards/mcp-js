run javascript or typescript code in v8

Submits code for **async execution** — returns an execution ID immediately. V8 runs in the background. Use `get_execution` to poll status and result, `get_execution_output` to read console output, and `cancel_execution` to stop a running execution.

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code: the javascript or typescript code to run
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (4–64, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.

returns:
- execution_id: UUID of the submitted execution. Use with get_execution, get_execution_output, and cancel_execution.

## Workflow

1. Call `run_js(code)` → get `execution_id`
2. Call `get_execution(execution_id)` → check `status` (running/completed/failed/cancelled/timed_out)
3. Call `get_execution_output(execution_id)` → read console output (paginated)
4. When status is "completed", `get_execution` returns the `result` (final expression value)

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

## Limitations

- **`async`/`await` and Promises**: Fully supported. If your code returns a Promise, the runtime resolves it automatically.
- **No `fetch` or network access by default**: When the server is started with `--opa-url`, a `fetch(url, opts?)` function becomes available. `fetch()` follows the web standard Fetch API — it returns a Promise that resolves to a Response object. Use `await` to get the response: `const resp = await fetch(url)`. The response object has `.ok`, `.status`, `.statusText`, `.url`, `.headers.get(name)`, `.text()`, and `.json()` methods (`.text()` and `.json()` also return Promises). Each request is checked against an OPA policy before execution. Without `--opa-url`, there is no network access.
- **No file system access**: The runtime does not provide access to the local file system or environment variables.
- **No `npm install` or external packages**: You cannot install or import npm packages. Only standard JavaScript (ECMAScript) built-ins are available.
- **No timers**: Functions like `setTimeout` and `setInterval` are not available.
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.

Each execution starts with a fresh V8 isolate — no state is carried between calls.
