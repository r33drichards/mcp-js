# Tutorial: Understanding Stateful vs Stateless Execution

In this tutorial you will set up mcp-v8 in both stateful and stateless modes, run code in each, and compare how they behave. You will see how stateful mode preserves variables across executions while stateless mode starts fresh each time.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Two free terminal windows

## Step 1: Start mcp-v8 in stateful mode (default)

Stateful mode is the default. Start the server:

```bash
mcp-v8 --http-port 3000
```

In stateful mode, the server exposes these tools:

- `run_js` -- execute code and get an execution ID
- `get_execution` -- check execution status
- `get_execution_output` -- read console output
- `cancel_execution` -- stop a running execution
- `list_executions` -- list all executions
- `list_sessions` -- list all sessions
- `list_session_snapshots` -- list snapshots for a session
- `get_heap_tags` / `set_heap_tags` / `delete_heap_tags` / `query_heaps_by_tags` -- manage tags

## Step 2: Run code in stateful mode -- define a variable

```bash
mcp-v8-cli --http-port 3000 exec --code '
var counter = 0;
counter += 1;
counter;
'
```

Expected output:

```
1
```

Note the execution ID returned. The server has taken a heap snapshot of the V8 state, including your `counter` variable.

## Step 3: Resume from the previous state

In stateful mode, you can continue from where you left off by passing the heap hash from the previous execution:

```bash
mcp-v8-cli --http-port 3000 exec --code '
counter += 1;
counter;
' --heap-id <HEAP_HASH_FROM_STEP_2>
```

Expected output:

```
2
```

The `counter` variable persisted because the server restored the heap snapshot before running your new code. Each execution produces a new snapshot, creating a chain of states.

## Step 4: Stop the stateful server

Press `Ctrl+C` in the server terminal to stop it.

## Step 5: Start mcp-v8 in stateless mode

Now start the server with the `--stateless` flag:

```bash
mcp-v8 --http-port 3001 --stateless
```

In stateless mode, the server exposes only the `run_js` tool. There are no sessions, snapshots, or execution tracking. Code runs, returns its output directly, and the state is discarded.

## Step 6: Run code in stateless mode

```bash
mcp-v8-cli --http-port 3001 exec --code '
var counter = 0;
counter += 1;
counter;
'
```

Expected output:

```
1
```

## Step 7: Try to resume -- observe the difference

Run the same continuation code:

```bash
mcp-v8-cli --http-port 3001 exec --code '
counter += 1;
counter;
'
```

This will fail with an error like `counter is not defined`, because stateless mode does not preserve any state between executions. Each call starts with a fresh V8 isolate.

## Step 8: Compare the two modes side by side

| Feature | Stateful (default) | Stateless (`--stateless`) |
|---|---|---|
| State preserved between calls | Yes (via heap snapshots) | No |
| Tools available | `run_js` + 10 management tools | `run_js` only |
| Output returned | Via `get_execution_output` | Directly in response |
| Storage required | Yes (local or S3) | No |
| Use case | Interactive sessions, iterative development | One-shot scripts, simple evaluation |

## Step 9: Understand when to use each mode

**Use stateful mode when:**

- An AI agent needs to build up state over multiple interactions
- You want to branch from previous states (try different approaches)
- You need execution history and session tracking
- You want to tag and organize heap snapshots

**Use stateless mode when:**

- Each execution is independent
- You want minimal resource usage
- You do not need to track execution history
- You want the simplest possible setup

## What you learned

- Stateful mode (default) preserves V8 heap state as snapshots between executions
- Stateless mode (`--stateless`) starts fresh each time and returns output directly
- Stateful mode provides additional tools for session management, execution tracking, and tagging
- You can resume from any previous snapshot by passing its heap hash
- The choice between modes depends on whether your use case needs persistent state

Next, dive deeper into stateful sessions in [Building a Stateful Session](sessions-and-heaps.md).
