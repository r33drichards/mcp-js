# MCP Tools

> Generated from the built-in MCP tool registry. Do not edit this page by hand.

This page documents the MCP tools exposed by `mcp-v8` itself. Upstream MCP
server stubs are not listed here because they depend on your runtime
configuration.

## Modes

- [Stateful mode](#stateful-mode)
- [Stateless mode](#stateless-mode)

## Stateful mode

These tools keep execution records and heap state available across calls.

### Tools

- [`cancel_execution`](#stateful-cancel-execution)
- [`delete_heap_tags`](#stateful-delete-heap-tags)
- [`get_execution`](#stateful-get-execution)
- [`get_execution_output`](#stateful-get-execution-output)
- [`get_heap_tags`](#stateful-get-heap-tags)
- [`list_executions`](#stateful-list-executions)
- [`list_session_snapshots`](#stateful-list-session-snapshots)
- [`list_sessions`](#stateful-list-sessions)
- [`query_heaps_by_tags`](#stateful-query-heaps-by-tags)
- [`run_js`](#stateful-run-js)
- [`set_heap_tags`](#stateful-set-heap-tags)

### `cancel_execution`
<a id="stateful-cancel-execution"></a>

Cancel a running execution. Terminates the V8 isolate.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `execution_id` | `string` | yes | - |

### `delete_heap_tags`
<a id="stateful-delete-heap-tags"></a>

Delete tags from a heap snapshot (stateful mode only). If keys is provided (comma-separated), only those tag keys are removed. If keys is omitted, all tags are deleted.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `heap` | `string` | yes | - |
| `keys` | `string | null` | no | - |

### `get_execution`
<a id="stateful-get-execution"></a>

Get the status and result of an execution. Returns execution_id, status (running/completed/failed/cancelled/timed_out), result (if completed), heap (if stateful), error (if failed), started_at, and completed_at.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `execution_id` | `string` | yes | - |

### `get_execution_output`
<a id="stateful-get-execution-output"></a>

Get paginated console output for an execution. Supports two modes: line-based (line_offset + line_limit) or byte-based (byte_offset + byte_limit). If byte_offset is provided, byte mode takes precedence. Response includes both line and byte coordinates for cross-referencing. Use next_line_offset or next_byte_offset from a previous response to resume reading.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `byte_limit` | `integer | null` | no | - |
| `byte_offset` | `integer | null` | no | - |
| `execution_id` | `string` | yes | - |
| `line_limit` | `integer | null` | no | - |
| `line_offset` | `integer | null` | no | - |

### `get_heap_tags`
<a id="stateful-get-heap-tags"></a>

Get tags for a heap snapshot (stateful mode only). Returns a map of key-value tags associated with the given heap content hash.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `heap` | `string` | yes | - |

### `list_executions`
<a id="stateful-list-executions"></a>

List all executions with their status.

This tool does not take structured parameters.

### `list_session_snapshots`
<a id="stateful-list-session-snapshots"></a>

List all log entries for the current session (stateful mode only). Each entry contains the input heap hash, output heap hash, code executed, and timestamp. Use the fields parameter to select specific fields (comma-separated: index,input_heap,output_heap,code,timestamp).

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `fields` | `string | null` | no | - |

### `list_sessions`
<a id="stateful-list-sessions"></a>

List all named sessions (stateful mode only). Returns an array of session names that have been used via REST session fields or the X-MCP-Session-Id header.

This tool does not take structured parameters.

### `query_heaps_by_tags`
<a id="stateful-query-heaps-by-tags"></a>

Query heap snapshots by tags (stateful mode only). Provide a map of key-value pairs to match. Returns all heaps whose tags contain all the specified key-value pairs.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `tags` | `object<string, string>` | yes | - |

### `run_js`
<a id="stateful-run-js"></a>

run javascript or typescript code in v8

Submits code for **async execution** in stateful MCP mode — returns an execution ID immediately. V8 runs in the background. Use `get_execution` to poll status and result, `get_execution_output` to read console output, and `cancel_execution` to stop a running execution.

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code: the javascript or typescript code to run
- heap (optional): content hash (SHA-256 hex string) from a previous execution to resume that session, or omit for a fresh session
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (minimum: 4, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.
- tags (optional): a JSON object of key-value string pairs to associate with the resulting heap snapshot. Tags can be used to label, categorize, or annotate heaps for later retrieval. Use get_heap_tags, set_heap_tags, delete_heap_tags, and query_heaps_by_tags to manage tags independently.

returns:
- execution_id: UUID of the submitted execution. Use with get_execution, get_execution_output, and cancel_execution.

Session identity for MCP history tracking comes from the `X-MCP-Session-Id` header during initialization, not from a `session` tool parameter.

#### Workflow

1. Call `run_js(code)` → get `execution_id`
2. Call `get_execution(execution_id)` → check `status` (running/completed/failed/cancelled/timed_out)
3. Call `get_execution_output(execution_id)` → read console output (paginated)
4. When status is "completed", `get_execution` returns the `result` and `heap` (content hash). Use `get_execution_output` to read console output.

#### Console Output

`console.log`, `console.info`, `console.warn`, and `console.error` are fully supported. Output is streamed to persistent storage during execution and can be queried in real-time using `get_execution_output`.

Console output supports two pagination modes:
- **Line mode**: `line_offset` + `line_limit` — fetch N lines starting from line M
- **Byte mode**: `byte_offset` + `byte_limit` — fetch N bytes starting from byte M

Both modes return position info in both coordinate systems for cross-referencing. Use `next_line_offset` or `next_byte_offset` from a previous response to resume reading.

#### Return Values

All code runs as ES modules, which support `import`/`export` declarations and **top-level `await`**. Use `console.log()` to output results, then read them via `get_execution_output`.

eg:

```js
const result = 1 + 1;
console.log(result);
```

After execution completes, `get_execution_output` will return `data: "2\n"`.

To return structured data, JSON-stringify it:

```js
const obj = { a: 1, b: 2 };
console.log(JSON.stringify(obj));
```

Top-level `await` is fully supported:

```js
const resp = await fetch("https://example.com/api");
const data = await resp.json();
console.log(JSON.stringify(data));
```

#### Importing Packages

You can import npm packages, JSR packages, and URL modules using ES module `import` syntax. Packages are fetched from esm.sh at runtime — no installation needed.

- **npm**: `import { camelCase } from "npm:lodash-es@4.17.21";`
- **jsr**: `import { camelCase } from "jsr:@luca/cases@1.0.0";`
- **URL**: `import { pascalCase } from "https://deno.land/x/case/mod.ts";`

Always pin versions for reproducible results. Dynamic `import()` is also supported with top-level `await`.

#### Filesystem Access

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

#### Limitations

- **No `fetch` or network access by default**: When the server is started with fetch policies configured via `--policies-json`, a `fetch(url, opts?)` function becomes available. `fetch()` follows the web standard Fetch API — it returns a Promise that resolves to a Response object. Use `await` to get the response: `const resp = await fetch(url)`. The response object has `.ok`, `.status`, `.statusText`, `.url`, `.headers.get(name)`, `.text()`, and `.json()` methods (`.text()` and `.json()` also return Promises). Each request is checked against policy before execution. If the server is also configured with `--fetch-header` or `--fetch-header-config`, matching requests may receive static headers or dynamically acquired OAuth client-credentials bearer tokens before policy evaluation. Headers set directly in JavaScript still win. Without fetch policies, there is no network access.
- **No file system access by default**: Filesystem access requires server configuration with policies. See "Filesystem Access" above.
- **No environment variables**: The runtime does not provide access to environment variables.
- **Limited timers**: `setTimeout(callback, delayMs)` and `clearTimeout(id)` are available and always enabled. `setInterval` / `clearInterval` are not. Timer callbacks only fire while the execution is still running (within the execution timeout window).
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.

#### Subprocess and MCP Tools (when configured)

When the server is started with a subprocess policy (via `--policies-json`), a `Deno.Command` constructor and a `child_process.exec(command, opts?)` function become available for running OS commands. Every invocation is checked against the subprocess Rego policy before execution. Without a subprocess policy, command execution is unavailable.

When the server is configured with upstream MCP servers (`--mcp-server` / `--mcp-config`), a `mcp` global is available: `await mcp.callTool(server, tool, args?)`, `mcp.listTools(server?)`, and `mcp.servers`. Tool calls are gated by the optional MCP tools policy.

In stateful mode, each execution returns a SHA-256 content hash for the heap snapshot — pass it back as the `heap` parameter in the next call to resume from that state. Omit `heap` for a fresh heap.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `code` | `string` | yes | - |
| `execution_timeout_secs` | `integer | null` | no | - |
| `heap` | `string | null` | no | - |
| `heap_memory_max_mb` | `integer | null` | no | - |
| `tags` | `object | null` | no | - |

### `set_heap_tags`
<a id="stateful-set-heap-tags"></a>

Set or replace tags on a heap snapshot (stateful mode only). Provide a map of key-value string pairs. This replaces all existing tags for the heap.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `heap` | `string` | yes | - |
| `tags` | `object<string, string>` | yes | - |


## Stateless mode

These tools execute in isolated runs and return output directly.

### Tools

- [`run_js`](#stateless-run-js)

### `run_js`
<a id="stateless-run-js"></a>

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

#### Console Output

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

#### Importing Packages

You can import npm packages, JSR packages, and URL modules using ES module `import` syntax. Packages are fetched from esm.sh at runtime — no installation needed.

- **npm**: `import { camelCase } from "npm:lodash-es@4.17.21";`
- **jsr**: `import { camelCase } from "jsr:@luca/cases@1.0.0";`
- **URL**: `import { pascalCase } from "https://deno.land/x/case/mod.ts";`

Always pin versions for reproducible results. Dynamic `import()` is also supported with top-level `await`.

#### Filesystem Access

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

#### Limitations

- **No `fetch` or network access by default**: When the server is started with fetch policies configured via `--policies-json`, a `fetch(url, opts?)` function becomes available. `fetch()` follows the web standard Fetch API — it returns a Promise that resolves to a Response object. Use `await` to get the response: `const resp = await fetch(url)`. The response object has `.ok`, `.status`, `.statusText`, `.url`, `.headers.get(name)`, `.text()`, and `.json()` methods (`.text()` and `.json()` also return Promises). Each request is checked against policy before execution. If the server is also configured with `--fetch-header` or `--fetch-header-config`, matching requests may receive static headers or dynamically acquired OAuth client-credentials bearer tokens before policy evaluation. Headers set directly in JavaScript still win. Without fetch policies, there is no network access.
- **No file system access by default**: Filesystem access requires server configuration with policies. See "Filesystem Access" above.
- **No environment variables**: The runtime does not provide access to environment variables.
- **Limited timers**: `setTimeout(callback, delayMs)` and `clearTimeout(id)` are available and always enabled. `setInterval` / `clearInterval` are not. Timer callbacks only fire while the execution is still running (within the execution timeout window).
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.

When the server is started with a subprocess policy (via `--policies-json`), a `Deno.Command` constructor and a `child_process.exec(command, opts?)` function become available for running OS commands, each checked against the subprocess Rego policy. When the server is configured with upstream MCP servers (`--mcp-server` / `--mcp-config`), a `mcp` global is available (`mcp.callTool`, `mcp.listTools`, `mcp.servers`), gated by the optional MCP tools policy.

Each execution starts with a fresh V8 isolate — no state is carried between calls.

Parameters:

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `code` | `string` | yes | - |
| `execution_timeout_secs` | `integer | null` | no | - |
| `heap_memory_max_mb` | `integer | null` | no | - |
