# Tutorial: Using the CLI Client

In this tutorial you will learn how to use `mcp-v8-cli`, the command-line client for mcp-v8. You will execute code, list and inspect executions, read output, and cancel running jobs.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed: `curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install-cli.sh | sudo bash`
- Server running: `mcp-v8 --http-port 3000`

## Step 1: Verify the CLI is installed

```bash
mcp-v8-cli --help
```

You should see the available commands and options.

## Step 2: Execute code with `exec`

The `exec` command runs JavaScript or TypeScript:

```bash
mcp-v8-cli --http-port 3000 exec --code 'Math.PI * 2'
```

Expected output:

```
6.283185307179586
```

## Step 3: Execute multi-line code

For longer code, use single quotes to wrap multi-line strings:

```bash
mcp-v8-cli --http-port 3000 exec --code '
function fibonacci(n) {
  if (n <= 1) return n;
  let a = 0, b = 1;
  for (let i = 2; i <= n; i++) {
    [a, b] = [b, a + b];
  }
  return b;
}

const results = [];
for (let i = 0; i < 10; i++) {
  results.push(fibonacci(i));
}
results;
'
```

Expected output:

```
[0, 1, 1, 2, 3, 5, 8, 13, 21, 34]
```

## Step 4: Resume from a heap snapshot

After an execution, note the heap hash in the output. Use `--heap-id` to continue from that state:

```bash
# First execution
mcp-v8-cli --http-port 3000 exec --code '
var counter = 0;
counter;
'

# Note the heap hash, then resume
mcp-v8-cli --http-port 3000 exec --code '
counter += 10;
counter;
' --heap-id <HEAP_HASH>
```

Expected output:

```
10
```

## Step 5: List all executions

View all executions the server has tracked:

```bash
mcp-v8-cli --http-port 3000 list-executions
```

This shows execution IDs, statuses, and associated heap hashes.

## Step 6: Get execution details

Inspect a specific execution:

```bash
mcp-v8-cli --http-port 3000 get-execution --execution-id <EXECUTION_ID>
```

This shows the full details of the execution including its status, result, and heap hash.

## Step 7: Read console output

Retrieve the console output for an execution:

```bash
# First, create an execution with console output
mcp-v8-cli --http-port 3000 exec --code '
for (let i = 1; i <= 5; i++) {
  console.log(`Processing item ${i}`);
}
"done";
'

# Then read the output
mcp-v8-cli --http-port 3000 output --execution-id <EXECUTION_ID>
```

Expected output:

```
Processing item 1
Processing item 2
Processing item 3
Processing item 4
Processing item 5
```

## Step 8: Paginate console output

For large outputs, use pagination:

```bash
# Read first 3 lines
mcp-v8-cli --http-port 3000 output --execution-id <EXECUTION_ID> --offset 0 --limit 3

# Read next 3 lines
mcp-v8-cli --http-port 3000 output --execution-id <EXECUTION_ID> --offset 3 --limit 3
```

## Step 9: Cancel a running execution

Start a long-running execution and then cancel it:

```bash
# Start a slow execution
mcp-v8-cli --http-port 3000 exec --code '
let i = 0;
while (true) { i++; }
' &

# Cancel it (use the execution ID from the output)
mcp-v8-cli --http-port 3000 cancel --execution-id <EXECUTION_ID>
```

## Step 10: List sessions

View all sessions:

```bash
mcp-v8-cli --http-port 3000 list-sessions
```

## Step 11: List session snapshots

View all snapshots in a specific session:

```bash
mcp-v8-cli --http-port 3000 list-session-snapshots --session-id <SESSION_ID>
```

## Step 12: Work with heap tags

Set tags on a heap:

```bash
mcp-v8-cli --http-port 3000 set-heap-tags \
  --heap-id <HEAP_HASH> \
  --tags '{"project": "tutorial", "status": "complete"}'
```

Get tags:

```bash
mcp-v8-cli --http-port 3000 get-heap-tags --heap-id <HEAP_HASH>
```

Query heaps by tags:

```bash
mcp-v8-cli --http-port 3000 query-heaps-by-tags --tags '{"project": "tutorial"}'
```

Delete tags:

```bash
mcp-v8-cli --http-port 3000 delete-heap-tags --heap-id <HEAP_HASH> --tags '["status"]'
```

## Step 13: Common CLI patterns

Here are some useful patterns for working with the CLI:

**Quick one-liner evaluation:**
```bash
mcp-v8-cli --http-port 3000 exec --code 'JSON.stringify({a: 1, b: 2}, null, 2)'
```

**Chain of executions (in a script):**
```bash
#!/bin/bash
HEAP=$(mcp-v8-cli --http-port 3000 exec --code 'var x = 1; x;' | grep heap | awk '{print $2}')
mcp-v8-cli --http-port 3000 exec --code 'x += 1; x;' --heap-id "$HEAP"
```

**Execute from a file:**
```bash
mcp-v8-cli --http-port 3000 exec --code "$(cat script.js)"
```

## What you learned

- How to execute code with `exec --code`
- How to resume from heap snapshots with `--heap-id`
- How to list and inspect executions
- How to read and paginate console output
- How to cancel running executions
- How to manage heap tags from the CLI
- Common CLI patterns for scripting

Next, learn about MCP pass-through in [Connecting Upstream MCP Servers](mcp-pass-through.md).
