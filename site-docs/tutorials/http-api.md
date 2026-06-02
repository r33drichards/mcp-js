# Tutorial: Using the REST API Directly

In this tutorial you will interact with mcp-v8 using curl and the HTTP REST API. You will execute code, poll for results, read console output, and cancel executions -- all without the CLI client.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- `curl` and `jq` installed

## Step 1: Start the server with HTTP transport

```bash
mcp-v8 --http-port 3000
```

## Step 2: Execute JavaScript code

Submit code for execution using a POST request:

```bash
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{
    "code": "const x = 42; x * 2;"
  }' | jq .
```

Expected output:

```json
{
  "execution_id": "exec-abc123",
  "heap_hash": "sha256-...",
  "result": "84"
}
```

The response includes:
- `execution_id` -- unique identifier for this execution
- `heap_hash` -- the hash of the resulting heap snapshot
- `result` -- the return value of the code

## Step 3: Execute TypeScript code

TypeScript works the same way:

```bash
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{
    "code": "interface Point { x: number; y: number; }\nconst p: Point = { x: 3, y: 4 };\nMath.sqrt(p.x ** 2 + p.y ** 2);"
  }' | jq .
```

Expected output:

```json
{
  "execution_id": "exec-def456",
  "heap_hash": "sha256-...",
  "result": "5"
}
```

## Step 4: Resume from a previous heap

Pass the `heap_hash` from a previous execution to continue from that state:

```bash
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{
    "code": "x + 100;",
    "heap_id": "<HEAP_HASH_FROM_STEP_2>"
  }' | jq .
```

Expected output:

```json
{
  "execution_id": "exec-ghi789",
  "heap_hash": "sha256-...",
  "result": "142"
}
```

The variable `x` (set to 42 in Step 2) is available because you resumed from that heap.

## Step 5: Poll for execution status

For long-running code, poll the execution status:

```bash
# Submit a long-running execution
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{
    "code": "let sum = 0; for (let i = 0; i < 1000000; i++) { sum += i; } sum;"
  }' | jq .
```

Check the execution status:

```bash
curl -s http://localhost:3000/execution/<EXECUTION_ID> | jq .
```

Expected output:

```json
{
  "execution_id": "exec-abc123",
  "status": "completed",
  "heap_hash": "sha256-...",
  "result": "499999500000"
}
```

The `status` field can be `running`, `completed`, or `failed`.

## Step 6: Read console output

When code uses `console.log`, the output is stored separately. Retrieve it:

```bash
# Execute code with console output
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{
    "code": "console.log(\"Line 1\"); console.log(\"Line 2\"); console.log(\"Line 3\"); \"done\";"
  }' | jq .
```

Read the console output:

```bash
curl -s "http://localhost:3000/execution/<EXECUTION_ID>/output" | jq .
```

Expected output:

```json
{
  "output": "Line 1\nLine 2\nLine 3\n"
}
```

## Step 7: Paginate console output

For large outputs, use pagination parameters:

```bash
# Read lines 0-9 (first 10 lines)
curl -s "http://localhost:3000/execution/<EXECUTION_ID>/output?offset=0&limit=10" | jq .

# Read lines 10-19
curl -s "http://localhost:3000/execution/<EXECUTION_ID>/output?offset=10&limit=10" | jq .
```

For byte-based pagination:

```bash
# Read first 256 bytes
curl -s "http://localhost:3000/execution/<EXECUTION_ID>/output?byte_offset=0&byte_limit=256" | jq .
```

## Step 8: Cancel a running execution

Start a long-running execution:

```bash
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{
    "code": "while (true) { /* infinite loop */ }"
  }' | jq .
```

Cancel it:

```bash
curl -s -X POST http://localhost:3000/execution/<EXECUTION_ID>/cancel | jq .
```

Expected output:

```json
{
  "status": "cancelled"
}
```

## Step 9: List all executions

```bash
curl -s http://localhost:3000/executions | jq .
```

This returns an array of all executions with their IDs, statuses, and heap hashes.

## Step 10: List sessions

```bash
curl -s http://localhost:3000/sessions | jq .
```

## Step 11: Work with heap tags via the API

Set tags on a heap:

```bash
curl -s -X POST http://localhost:3000/heap/<HEAP_HASH>/tags \
  -H "Content-Type: application/json" \
  -d '{
    "tags": {
      "environment": "production",
      "version": "1.0"
    }
  }' | jq .
```

Get tags:

```bash
curl -s http://localhost:3000/heap/<HEAP_HASH>/tags | jq .
```

Query heaps by tags:

```bash
curl -s -X POST http://localhost:3000/heaps/query \
  -H "Content-Type: application/json" \
  -d '{
    "tags": {
      "environment": "production"
    }
  }' | jq .
```

Delete tags:

```bash
curl -s -X DELETE http://localhost:3000/heap/<HEAP_HASH>/tags \
  -H "Content-Type: application/json" \
  -d '{
    "tags": ["version"]
  }' | jq .
```

## Step 12: A complete workflow with curl

Here is a full workflow scripted with curl:

```bash
#!/bin/bash

# Step 1: Execute initial code
RESULT=$(curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{"code": "var data = [1, 2, 3, 4, 5]; data;"}')

HEAP=$(echo $RESULT | jq -r '.heap_hash')
echo "Initial heap: $HEAP"

# Step 2: Continue from that state
RESULT=$(curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d "{\"code\": \"var sum = data.reduce((a, b) => a + b, 0); sum;\", \"heap_id\": \"$HEAP\"}")

echo "Sum: $(echo $RESULT | jq -r '.result')"
HEAP=$(echo $RESULT | jq -r '.heap_hash')

# Step 3: Tag the result
curl -s -X POST "http://localhost:3000/heap/$HEAP/tags" \
  -H "Content-Type: application/json" \
  -d '{"tags": {"stage": "summed"}}'

echo "Tagged heap: $HEAP"
```

## What you learned

- How to execute code via POST to `/execute`
- How to resume from a heap snapshot by passing `heap_id`
- How to poll execution status via GET to `/execution/<id>`
- How to read and paginate console output
- How to cancel running executions
- How to manage heap tags via the REST API
- How to script complete workflows with curl

Next, learn about the CLI client in [Using the CLI Client](mcp-v8-cli.md).
