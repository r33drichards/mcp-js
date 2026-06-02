# How to Read Paginated Console Output

Retrieve `console.log`, `console.info`, `console.warn`, and `console.error` output from executions.

## Read output (stateful mode)

After submitting code with `run_js`, use `get_execution_output` to read console output:

```
get_execution_output({ execution_id: "abc-123" })
```

Response:

```json
{
  "execution_id": "abc-123",
  "data": "Hello world\n",
  "start_line": 0,
  "end_line": 1,
  "total_lines": 1,
  "next_line_offset": 1,
  "start_byte": 0,
  "end_byte": 12,
  "total_bytes": 12,
  "next_byte_offset": 12,
  "has_more": false,
  "status": "completed"
}
```

## Paginate by lines

Fetch a window of lines using `line_offset` and `line_limit`:

```
get_execution_output({
  execution_id: "abc-123",
  line_offset: 0,
  line_limit: 50
})
```

Use `next_line_offset` from the response to fetch the next page:

```
get_execution_output({
  execution_id: "abc-123",
  line_offset: 50,
  line_limit: 50
})
```

Continue until `has_more` is `false`.

## Paginate by bytes

For byte-level control, use `byte_offset` and `byte_limit`:

```
get_execution_output({
  execution_id: "abc-123",
  byte_offset: 0,
  byte_limit: 4096
})
```

Use `next_byte_offset` to continue.

## Stream output while running

You can call `get_execution_output` while the execution is still running. Output is streamed to storage in real-time. Check the `status` field in the response to know if the execution is still producing output.

## Read output via the HTTP API

```bash
curl "http://localhost:3000/api/executions/abc-123/output?line_offset=0&line_limit=100"
```

## Read output via the CLI

```bash
mcp-v8-cli executions output abc-123 --line-offset 0 --line-limit 100
```

## Console output prefixes

- `console.log()` -- no prefix
- `console.info()` -- `[INFO]` prefix
- `console.warn()` -- `[WARN]` prefix
- `console.error()` -- `[ERROR]` prefix
