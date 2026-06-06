# Running JavaScript & TypeScript

We'll run our first script on mcp-v8, inspect its console output, and then send a TypeScript snippet — all in a few commands.

## Prerequisites

Install the server binary. The quickest path is Nix:

```bash
nix run github:r33drichards/mcp-js -- --version
```

Or download a prebuilt release and run `install.sh`. See [installation](../install/overview.md) for all options.

## 1. Start the server

Start mcp-v8 in stateless mode with an HTTP port so the REST API is reachable:

```bash
mcp-v8 --http-port 3000 --stateless
```

Stateless mode runs each script in a fresh V8 isolate with no heap persistence — the simplest way to get started. You can switch to stateful mode later.

## 2. Run your first JavaScript

Submit a script via the REST API. The server responds `202` with an `execution_id`:

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H 'Content-Type: application/json' \
  -d '{"code": "console.log(\"hello, world!\");"}'
```

Expected response:

```json
{"execution_id": "e3f1a2b4-5678-..."}
```

## 3. Check the execution status

Poll `GET /api/executions/{id}` until `status` is `completed`:

```bash
curl -s http://localhost:3000/api/executions/e3f1a2b4-5678-...
```

Expected response:

```json
{
  "execution_id": "e3f1a2b4-5678-...",
  "status": "completed",
  "result": "",
  "heap": null,
  "error": null,
  "started_at": "2024-01-15T10:00:00Z",
  "completed_at": "2024-01-15T10:00:01Z"
}
```

`result` is always an empty string — console output is stored separately and fetched in the next step.

## 4. Fetch the console output

```bash
curl -s "http://localhost:3000/api/executions/e3f1a2b4-5678-.../output"
```

Expected response:

```json
{
  "data": "hello, world!\n",
  "start_line": 1,
  "end_line": 1,
  "total_lines": 1,
  "has_more": false
}
```

`data` holds the captured text. `console.log`, `console.info`, `console.warn`, and `console.error` are all captured here. For long output, use `next_line_offset` or `next_byte_offset` to page through it.

## 5. Run TypeScript

TypeScript is accepted directly — the SWC transpiler strips type annotations before execution. No compile step or extra flag is needed:

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H 'Content-Type: application/json' \
  -d '{
    "code": "interface Greeting { message: string; }\nconst g: Greeting = { message: \"hello from TypeScript\" };\nconsole.log(g.message);"
  }'
```

After fetching the output, you'll see:

```
hello from TypeScript
```

TypeScript type syntax is stripped automatically. JSX/TSX is not supported and is rejected with a parse error — use plain TypeScript or JavaScript. Angle-bracket type assertions such as `<string>myValue` are permitted (they are unambiguous when TSX is disabled).

## 6. Call run_js as an MCP tool

If your MCP client is connected to the server, call `run_js` directly as a tool. In stateless mode the tool polls internally and returns the output synchronously:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "console.log(6 * 7);"
  }
}
```

Response:

```json
{"output": "42\n"}
```

If execution fails, the response includes both `output` (any captured lines before the error) and `error`.

## 7. Run TypeScript via the MCP tool

The same TypeScript support applies to the MCP tool:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "const greet = (name: string): string => `Hello, ${name}!`;\nconsole.log(greet(\"world\"));"
  }
}
```

Response:

```json
{"output": "Hello, world!\n"}
```

## Next steps

- Try stateful mode with heap persistence: see [Stateful sessions & heap snapshots](../tutorials/sessions-and-heaps.md).
- For long-running scripts and output streaming, see [Asynchronous execution & output](../tutorials/async-execution.md).

## See also

- [Concepts — how execution works](../concepts/js-execution.md)
- [How-to — recipes for common tasks](../how-to/js-execution.md)
- [Reference — all parameters and return shapes](../reference/js-execution.md)
- [Asynchronous execution & output](../tutorials/async-execution.md)
- [Stateful sessions & heap snapshots](../tutorials/sessions-and-heaps.md)
