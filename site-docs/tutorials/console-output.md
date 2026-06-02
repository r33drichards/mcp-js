# Tutorial: Working with Console Output

In this tutorial you will learn how mcp-v8 captures console output from JavaScript executions. You will use `console.log`, `console.info`, `console.warn`, and `console.error`, and learn how to read output with pagination in both line and byte modes.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Familiarity with stateful mode ([Execution Model](execution-model.md))

## Step 1: Start the server

```bash
mcp-v8 --http-port 3000
```

## Step 2: Generate console output at different levels

Run code that uses all four console methods:

```bash
mcp-v8-cli --http-port 3000 exec --code '
console.log("This is a log message");
console.info("This is an info message");
console.warn("This is a warning");
console.error("This is an error");
"done";
'
```

Note the execution ID from the response. In stateful mode, the return value and console output are separate -- the return value comes back in the execution result, while console output is retrieved with a separate call.

## Step 3: Read all console output

Retrieve the console output for the execution:

```bash
mcp-v8-cli --http-port 3000 output --execution-id <EXECUTION_ID>
```

Expected output:

```
This is a log message
This is an info message
This is a warning
This is an error
```

All four console methods are captured. The output preserves the order in which the messages were emitted.

## Step 4: Generate a large amount of output

Create an execution with many lines of output to demonstrate pagination:

```bash
mcp-v8-cli --http-port 3000 exec --code '
for (let i = 1; i <= 100; i++) {
  console.log(`Line ${i}: ${"x".repeat(50)}`);
}
"generated 100 lines";
'
```

Note the execution ID.

## Step 5: Read output in line mode with pagination

You can paginate output by lines. Read the first 10 lines:

```bash
mcp-v8-cli --http-port 3000 output --execution-id <EXECUTION_ID> --offset 0 --limit 10
```

This returns lines 1 through 10. To read the next page:

```bash
mcp-v8-cli --http-port 3000 output --execution-id <EXECUTION_ID> --offset 10 --limit 10
```

This returns lines 11 through 20. Continue incrementing the offset to page through all the output.

## Step 6: Read output in byte mode

You can also paginate by bytes, which is useful when lines vary significantly in length:

```bash
mcp-v8-cli --http-port 3000 output --execution-id <EXECUTION_ID> --byte-offset 0 --byte-limit 256
```

This returns the first 256 bytes of output. To get the next chunk:

```bash
mcp-v8-cli --http-port 3000 output --execution-id <EXECUTION_ID> --byte-offset 256 --byte-limit 256
```

Byte mode gives you precise control over how much data you transfer per request.

## Step 7: Combine console output with structured results

A common pattern is to use `console.log` for progress reporting and return structured data as the result:

```bash
mcp-v8-cli --http-port 3000 exec --code '
console.log("Starting data processing...");

const data = [10, 20, 30, 40, 50];
console.log(`Processing ${data.length} items`);

const sum = data.reduce((a, b) => a + b, 0);
const avg = sum / data.length;
console.log(`Sum: ${sum}, Average: ${avg}`);

console.log("Processing complete.");

// Return structured result
JSON.stringify({ sum, avg, count: data.length });
'
```

The return value is the JSON string `{"sum":150,"avg":30,"count":5}`. The console output tracks the progress separately:

```
Starting data processing...
Processing 5 items
Sum: 150, Average: 30
Processing complete.
```

## Step 8: Use console output for debugging

When an AI agent is iterating on code, console output is invaluable for debugging:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const obj = { a: 1, b: { c: 2, d: [3, 4] } };
console.log("Object:", JSON.stringify(obj, null, 2));

try {
  const result = obj.b.e.f;
} catch (e) {
  console.error("Caught error:", e.message);
}

"debug session complete";
'
```

The console output captures both the formatted object and the caught error, giving the agent full visibility into what happened.

## What you learned

- mcp-v8 captures output from `console.log`, `console.info`, `console.warn`, and `console.error`
- In stateful mode, console output is retrieved separately from the return value using `get_execution_output`
- Output can be paginated in line mode (offset/limit by line number) or byte mode (offset/limit by bytes)
- Console output is useful for progress reporting and debugging, while return values carry structured results

Next, learn how to organize snapshots in [Organizing Heap Snapshots with Tags](heap-tags.md).
