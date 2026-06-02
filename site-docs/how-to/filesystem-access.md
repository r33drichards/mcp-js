# How to Enable and Use Filesystem Access

Give JavaScript code access to the filesystem via an OPA-gated `fs` module.

## Enable filesystem access

Filesystem access is disabled by default. Enable it with a filesystem policy:

```bash
mcp-v8 --policies-json '{"filesystem": {"policies": [{"url": "file:///etc/mcp-v8/fs.rego"}]}}'
```

## Write a filesystem policy

Allow reads from `/data` and writes to `/tmp`:

```rego
package mcp.filesystem

default allow = false

allow {
    input.operation == "readFile"
    startswith(input.path, "/data/")
}

allow {
    input.operation == "writeFile"
    startswith(input.path, "/tmp/")
}

allow {
    input.operation == "readdir"
    startswith(input.path, "/data/")
}

allow {
    input.operation == "stat"
}
```

Policy input fields: `operation`, `path`, `destination` (for rename/copy), `recursive` (for mkdir/rm), `encoding` (for readFile).

## Use the fs module in JavaScript

The `fs` global provides Node.js-compatible async file operations:

```js
// Read a file
const content = await fs.readFile("/data/config.json");
console.log(content);

// Read as binary
const bytes = await fs.readFile("/data/image.png", "buffer");
console.log(bytes.length);

// Write a file
await fs.writeFile("/tmp/output.txt", "Hello world");

// Append to a file
await fs.appendFile("/tmp/log.txt", "new line\n");

// List directory
const entries = await fs.readdir("/data/");
console.log(JSON.stringify(entries));

// File metadata
const info = await fs.stat("/data/config.json");
console.log(JSON.stringify(info));

// Create directory
await fs.mkdir("/tmp/mydir", { recursive: true });

// Delete file or directory
await fs.rm("/tmp/mydir", { recursive: true });

// Rename / move
await fs.rename("/tmp/old.txt", "/tmp/new.txt");

// Copy
await fs.copyFile("/data/source.txt", "/tmp/copy.txt");

// Check existence
const exists = await fs.exists("/data/config.json");
console.log(exists);

// Delete a file
await fs.unlink("/tmp/output.txt");
```

All operations return Promises and are evaluated against the Rego policy before execution.
