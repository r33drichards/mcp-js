# First Run

The fastest way to verify `mcp-v8` is to start it in stateless HTTP mode and
submit a small execution through the REST API or CLI.

## Start the server

```bash
cargo build --release -p server
./target/release/server --stateless --http-port 3000
```

Stateless mode starts each execution from a fresh V8 isolate. That avoids heap
storage setup and is the simplest way to confirm the server is working.

## Submit an execution

```bash
curl -s http://localhost:3000/api/exec \
  -H 'content-type: application/json' \
  -d '{"code":"console.log(1 + 1)"}'
```

The server returns an `execution_id`. Poll `/api/executions/{id}` until the
status becomes `completed`, then read `/api/executions/{id}/output` to see the
console output.

## What success looks like

- the server starts without configuration errors
- `/api/exec` returns an execution ID
- the execution reaches `completed`
- the output endpoint returns `2`

Once that works, continue with [Transport Walkthrough](transport-walkthrough.md)
to choose a transport model, or move into [How-to](../how-to/install-server.md)
for task-focused setup instructions.
