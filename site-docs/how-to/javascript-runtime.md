# How to Run JavaScript and TypeScript Code

Execute JavaScript or TypeScript in mcp-v8's sandboxed V8 engine.

## Run JavaScript via the MCP tool

Call the `run_js` tool with your code:

```json
{
  "code": "const x = 1 + 1;\nconsole.log(x);"
}
```

In stateful mode, this returns an `execution_id`. Poll with `get_execution` and read output with `get_execution_output`.

In stateless mode, the result comes back directly as `{ output, error }`.

## Run TypeScript

TypeScript types are stripped automatically via SWC before execution. No type-checking is performed -- invalid types are silently removed.

```json
{
  "code": "const greet = (name: string): string => `Hello, ${name}`;\nconsole.log(greet('world'));"
}
```

## Use top-level await

All code runs as ES modules, so top-level `await` works:

```json
{
  "code": "const resp = await fetch('https://api.example.com/data');\nconst data = await resp.json();\nconsole.log(JSON.stringify(data));"
}
```

Note: `fetch()` requires policy configuration. See [Network Access](network-access.md).

## Return structured data

Use `console.log(JSON.stringify(...))` to return structured output:

```js
const result = { sum: 1 + 1, product: 2 * 3 };
console.log(JSON.stringify(result));
```

## Override execution limits

Pass optional parameters to override server defaults:

```json
{
  "code": "console.log('hello');",
  "heap_memory_max_mb": 16,
  "execution_timeout_secs": 60
}
```

- `heap_memory_max_mb`: V8 heap cap in MB (minimum 4, default 8)
- `execution_timeout_secs`: timeout in seconds (1--300, default 30)

## Run via the HTTP API

```bash
curl -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{"code": "console.log(42);"}'
```

Returns `{ "execution_id": "..." }`. See [HTTP API](http-api.md) for polling and output retrieval.

## Run via the CLI

```bash
mcp-v8-cli exec 'console.log(42);'
```

See [mcp-v8-cli](mcp-v8-cli.md) for full CLI usage.
