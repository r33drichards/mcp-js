# Quick Start: curl

Start the server in stateless HTTP mode:

```bash
mcp-v8 --stateless --http-port 3000
```

Submit a small execution:

```bash
EXECUTION_ID="$(
  curl -s http://localhost:3000/api/exec \
    -H 'content-type: application/json' \
    -d '{"code":"console.log(1 + 1)"}' \
    | jq -r '.execution_id'
)"
```

Read the output:

```bash
curl -s "http://localhost:3000/api/executions/${EXECUTION_ID}/output" | jq -r '.data'
```

Success looks like:

- the server starts
- `/api/exec` returns an execution ID
- the output endpoint returns `2`

See [Run with HTTP](../how-to/run-with-http.md) and
[HTTP API](../reference/http-api.md).
