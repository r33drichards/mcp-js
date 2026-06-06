# Running JavaScript & TypeScript

Focused recipes for common execution tasks. These assume the server is running with an HTTP or SSE transport. For the execution model, see [Concepts](../concepts/js-execution.md).

## How to run TypeScript

Pass TypeScript code as the `code` parameter. The server strips type annotations with the SWC transpiler before handing the code to V8. No flag or config is required:

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H 'Content-Type: application/json' \
  -d '{
    "code": "type Point = { x: number; y: number };\nconst p: Point = { x: 1, y: 2 };\nconsole.log(JSON.stringify(p));"
  }'
```

Plain JavaScript is valid TypeScript and always works. JSX/TSX is **not** supported — code containing JSX is rejected with a parse error, so write plain TypeScript or JavaScript. Angle-bracket type assertions (`<T>value`) are permitted because TSX is disabled.

## How to capture console output

Submit the execution, then read its output with `get_execution_output` (MCP) or
`GET /api/executions/{id}/output` (REST).

**Line-based paging** (default — returns up to 100 lines starting from line 1):

```bash
curl -s "http://localhost:3000/api/executions/{id}/output?line_offset=1&line_limit=50"
```

**Byte-based paging** (when `byte_offset` is provided it takes precedence over line params):

```bash
curl -s "http://localhost:3000/api/executions/{id}/output?byte_offset=0&byte_limit=8192"
```

To page through long output, use `next_line_offset` (or `next_byte_offset`) from the previous response as the offset for the next request, and continue until `has_more` is `false`.

Console methods and their prefixes in the captured text:

| Method | Prefix |
|---|---|
| `console.log(...)` | _(none)_ |
| `console.debug(...)` | _(none)_ |
| `console.trace(...)` | _(none)_ |
| `console.info(...)` | `[INFO] ` |
| `console.warn(...)` | `[WARN] ` |
| `console.error(...)` | `[ERROR] ` |

## How to set a per-call timeout

Pass `execution_timeout_secs` to override the server default (30 s). The value must be 1–300:

**Via the MCP tool (stateful)**:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "for (let i = 0; i < 1e10; i++) {}\nconsole.log('done');",
    "execution_timeout_secs": 5
  }
}
```

**Via REST**:

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H 'Content-Type: application/json' \
  -d '{"code": "while (true) {}", "execution_timeout_secs": 5}'
```

When the timeout fires, the V8 isolate is terminated and the execution status becomes `timed_out`. Any console output produced before the termination is still available.

## How to set a per-call memory cap

Pass `heap_memory_max_mb` to override the server default (8 MB). The effective value is clamped to a minimum of 8 MB:

**Via the MCP tool**:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "const buf = new Uint8Array(32 * 1024 * 1024); console.log('allocated');",
    "heap_memory_max_mb": 64
  }
}
```

**Via REST**:

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H 'Content-Type: application/json' \
  -d '{"code": "const buf = new Uint8Array(32*1024*1024); console.log(buf.length);", "heap_memory_max_mb": 64}'
```

If the V8 heap exceeds the cap, the isolate is terminated and the execution status becomes `failed` with an error message containing "Out of memory".

## How to return structured results from a script

Scripts have no explicit return value. Serialize data to JSON and log it:

```js
const stats = {
  count: 42,
  labels: ["a", "b", "c"],
};
console.log(JSON.stringify(stats));
```

The consumer reads `data` from the output page and parses the JSON string. For multiple results, log one JSON line per result and parse them individually.

## How to use setTimeout

`setTimeout` and `clearTimeout` are available. `setInterval` is not provided. Because code runs as an ES module, top-level `await` works:

```js
await new Promise(resolve => setTimeout(resolve, 500));
console.log("500 ms elapsed");
```

Long-running periodic work should be modelled as a loop:

```js
for (let i = 0; i < 5; i++) {
  await new Promise(resolve => setTimeout(resolve, 200));
  console.log(`tick ${i}`);
}
```

## How to target a specific heap snapshot (stateful mode only)

Pass a heap content hash in `heap` to resume from a previously saved V8 state:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "console.log(typeof myVar !== 'undefined' ? myVar : 'not set');",
    "heap": "a3f4b2c1d5e6..."
  }
}
```

Omitting `heap` starts from a fresh isolate. See [Stateful sessions & heap snapshots](../how-to/sessions-and-heaps.md) for how to obtain and manage heap hashes.

## How to tag an output heap (stateful mode only)

Pass `tags` to attach arbitrary key-value metadata to the output snapshot, which you can later query with `query_heaps_by_tags`:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "globalThis.counter = 1;",
    "tags": {"env": "test", "version": "2"}
  }
}
```

## See also

- [Concepts — execution model](../concepts/js-execution.md)
- [Reference — all parameters and return shapes](../reference/js-execution.md)
- [Asynchronous execution & output](../how-to/async-execution.md)
- [Stateful sessions & heap snapshots](../how-to/sessions-and-heaps.md)
