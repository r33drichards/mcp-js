# Tutorial: Reading and Writing Files

In this tutorial you will enable filesystem access in mcp-v8, write an OPA policy to control which paths are accessible, and use the Node.js-compatible `fs` module to read and write files from JavaScript.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Familiarity with OPA policies ([Policy System](policy-system.md))

## Step 1: Create a filesystem policy

Filesystem access is gated by OPA policy. Without a policy, all file operations are denied. Create `fs-policy.json`:

```json
{
  "filesystem": {
    "rego_source": "package mcp_v8.filesystem\n\ndefault allow = false\n\nallow {\n  startswith(input.path, \"/tmp/mcp-v8-tutorial/\")\n}"
  }
}
```

This policy allows all file operations (read and write) under `/tmp/mcp-v8-tutorial/`, and denies everything else.

## Step 2: Create the working directory

```bash
mkdir -p /tmp/mcp-v8-tutorial
```

## Step 3: Start the server with the policy

```bash
mcp-v8 --http-port 3000 --policies-json fs-policy.json
```

## Step 4: Write a file

Use the `fs` module to write a file:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const fs = require("fs");

fs.writeFileSync("/tmp/mcp-v8-tutorial/hello.txt", "Hello from mcp-v8!\n");
"File written successfully";
'
```

Expected output:

```
File written successfully
```

## Step 5: Read the file back

```bash
mcp-v8-cli --http-port 3000 exec --code '
const fs = require("fs");

const content = fs.readFileSync("/tmp/mcp-v8-tutorial/hello.txt", "utf-8");
content;
'
```

Expected output:

```
Hello from mcp-v8!
```

## Step 6: Write and read JSON data

A common pattern is writing structured data as JSON:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const fs = require("fs");

const data = {
  users: [
    { name: "Alice", role: "admin" },
    { name: "Bob", role: "editor" },
    { name: "Charlie", role: "viewer" },
  ],
  updatedAt: new Date().toISOString(),
};

fs.writeFileSync(
  "/tmp/mcp-v8-tutorial/users.json",
  JSON.stringify(data, null, 2)
);
"JSON written";
'
```

Read it back:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const fs = require("fs");

const raw = fs.readFileSync("/tmp/mcp-v8-tutorial/users.json", "utf-8");
const data = JSON.parse(raw);
data.users.map(u => `${u.name} (${u.role})`);
'
```

Expected output:

```
["Alice (admin)", "Bob (editor)", "Charlie (viewer)"]
```

## Step 7: Append to a file

```bash
mcp-v8-cli --http-port 3000 exec --code '
const fs = require("fs");

fs.appendFileSync("/tmp/mcp-v8-tutorial/log.txt", "Event 1: Started\n");
fs.appendFileSync("/tmp/mcp-v8-tutorial/log.txt", "Event 2: Processing\n");
fs.appendFileSync("/tmp/mcp-v8-tutorial/log.txt", "Event 3: Completed\n");

const content = fs.readFileSync("/tmp/mcp-v8-tutorial/log.txt", "utf-8");
content;
'
```

Expected output:

```
Event 1: Started
Event 2: Processing
Event 3: Completed
```

## Step 8: List directory contents

```bash
mcp-v8-cli --http-port 3000 exec --code '
const fs = require("fs");

const files = fs.readdirSync("/tmp/mcp-v8-tutorial/");
files;
'
```

Expected output:

```
["hello.txt", "log.txt", "users.json"]
```

## Step 9: Check if a file exists and get stats

```bash
mcp-v8-cli --http-port 3000 exec --code '
const fs = require("fs");

const exists = fs.existsSync("/tmp/mcp-v8-tutorial/hello.txt");
const stats = fs.statSync("/tmp/mcp-v8-tutorial/hello.txt");

({
  exists,
  size: stats.size,
  isFile: stats.isFile(),
  isDirectory: stats.isDirectory(),
});
'
```

## Step 10: Test policy denial

Try to access a path outside the allowed directory:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const fs = require("fs");
fs.readFileSync("/etc/passwd", "utf-8");
'
```

This will fail with a policy denial error. The OPA policy ensures JavaScript code can only access the paths you have explicitly permitted.

## Step 11: Write a more granular policy

For finer control, you can separate read and write permissions:

```json
{
  "filesystem": {
    "rego_source": "package mcp_v8.filesystem\n\ndefault allow = false\n\nallow {\n  startswith(input.path, \"/tmp/mcp-v8-tutorial/\")\n  input.operation == \"read\"\n}\n\nallow {\n  startswith(input.path, \"/tmp/mcp-v8-tutorial/output/\")\n  input.operation == \"write\"\n}"
  }
}
```

This policy:
- Allows reading anything under `/tmp/mcp-v8-tutorial/`
- Allows writing only under `/tmp/mcp-v8-tutorial/output/`
- Denies writing to any other path, even within the tutorial directory

## What you learned

- Filesystem access requires an OPA policy that allows it
- The `fs` module provides Node.js-compatible file operations
- You can read, write, append, list directories, and check file stats
- Policy input includes `path` and `operation` for granular control
- Paths outside the allowed policy are denied

Next, learn about clustering in [Setting Up a Multi-Node Cluster](clustering.md).
