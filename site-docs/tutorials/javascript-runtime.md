# Tutorial: Running Your First JS/TS Code in mcp-v8

In this tutorial you will execute JavaScript and TypeScript code inside the mcp-v8 V8 engine. You will learn how basic execution works, how TypeScript is supported out of the box, how to use async/await, and how console output is captured.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- A terminal

## Step 1: Start the mcp-v8 server

Open a terminal and start the server with HTTP transport so the CLI can connect:

```bash
mcp-v8 --http-port 3000
```

You should see output indicating the server is listening. Leave this running and open a second terminal for the remaining steps.

## Step 2: Run a simple JavaScript expression

Use the CLI client to execute a basic expression:

```bash
mcp-v8-cli --http-port 3000 exec --code '1 + 1'
```

Expected output:

```
2
```

The `run_js` tool evaluates the code and returns the result of the last expression.

## Step 3: Run multi-line JavaScript

You can run more complex code. Try defining a function and calling it:

```bash
mcp-v8-cli --http-port 3000 exec --code '
function greet(name) {
  return `Hello, ${name}!`;
}
greet("world");
'
```

Expected output:

```
Hello, world!
```

## Step 4: Use console.log for output

The V8 engine captures all console output. Try using `console.log`:

```bash
mcp-v8-cli --http-port 3000 exec --code '
console.log("Step 1: Initialize");
console.log("Step 2: Process");
console.log("Step 3: Complete");
"done";
'
```

The return value is `"done"`, and the console output is captured separately. You can retrieve it using the output commands (covered in the [Console Output tutorial](console-output.md)).

## Step 5: Run TypeScript code

mcp-v8 supports TypeScript natively -- no configuration needed. The server transpiles TypeScript to JavaScript before execution:

```bash
mcp-v8-cli --http-port 3000 exec --code '
interface User {
  name: string;
  age: number;
}

function describeUser(user: User): string {
  return `${user.name} is ${user.age} years old`;
}

const alice: User = { name: "Alice", age: 30 };
describeUser(alice);
'
```

Expected output:

```
Alice is 30 years old
```

Type annotations are stripped during transpilation. This means TypeScript types help you write correct code, but they are not enforced at runtime.

## Step 6: Use async/await

The runtime supports top-level async/await. You can write asynchronous code directly:

```bash
mcp-v8-cli --http-port 3000 exec --code '
async function delay(ms: number): Promise<string> {
  return new Promise(resolve => {
    setTimeout(() => resolve(`Waited ${ms}ms`), ms);
  });
}

const result = await delay(100);
result;
'
```

Expected output:

```
Waited 100ms
```

## Step 7: Work with modern JavaScript features

The V8 engine supports modern ECMAScript features including destructuring, optional chaining, nullish coalescing, and more:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const data = {
  users: [
    { name: "Alice", address: { city: "Portland" } },
    { name: "Bob", address: null },
  ]
};

const cities = data.users.map(u => u.address?.city ?? "Unknown");
cities;
'
```

Expected output:

```
["Portland", "Unknown"]
```

## Step 8: Handle errors

When code throws an error, mcp-v8 returns the error information:

```bash
mcp-v8-cli --http-port 3000 exec --code '
function riskyOperation() {
  throw new Error("Something went wrong");
}
riskyOperation();
'
```

The output will contain the error message and stack trace, which is useful for debugging.

## What you learned

- How to start mcp-v8 with HTTP transport and connect with the CLI
- How to execute JavaScript expressions and multi-line code
- That TypeScript is supported natively with automatic transpilation
- How to use async/await at the top level
- That console output is captured separately from return values
- That modern JavaScript features are fully supported
- How errors are reported back to the caller

Next, learn about the two execution modes in [Understanding Stateful vs Stateless Execution](execution-model.md).
