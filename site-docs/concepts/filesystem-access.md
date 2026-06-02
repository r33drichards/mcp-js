# Filesystem Access Model

Filesystem access in mcp-v8 is disabled by default. When enabled through policy configuration, JavaScript code gains access to a Node.js-compatible `fs` module where every operation is gated by an OPA policy. This design gives agents the ability to read and write files while giving operators precise control over what paths and operations are permitted.

## Disabled by Default

Like network access, filesystem access does not exist in the JavaScript global scope unless explicitly configured. Without a filesystem policy chain in `--policies-json`, the `fs` object is not injected, and any attempt to access it produces a `ReferenceError`.

## The fs API

When enabled, `globalThis.fs` provides these async operations:

```js
// Read a file as UTF-8 text
const text = await fs.readFile("/tmp/data.txt");

// Read a file as binary (Uint8Array)
const bytes = await fs.readFile("/tmp/data.bin", "buffer");

// Write text to a file
await fs.writeFile("/tmp/out.txt", "hello");

// Write binary data to a file
await fs.writeFile("/tmp/out.bin", new Uint8Array([1, 2, 3]));

// Append to a file
await fs.appendFile("/tmp/out.txt", " world");

// List directory contents
const entries = await fs.readdir("/tmp");  // string[]

// Get file metadata
const info = await fs.stat("/tmp/data.txt");
// { size, isFile, isDirectory, isSymlink, readonly, mtimeMs, atimeMs, birthtimeMs }

// Create a directory
await fs.mkdir("/tmp/newdir", { recursive: true });

// Remove a file or directory
await fs.rm("/tmp/data.txt");
await fs.rm("/tmp/newdir", { recursive: true });

// Rename/move
await fs.rename("/tmp/old.txt", "/tmp/new.txt");

// Copy a file
await fs.copyFile("/tmp/a.txt", "/tmp/b.txt");

// Check existence
const exists = await fs.exists("/tmp/data.txt");  // boolean
```

All operations are async and return Promises. There are no synchronous filesystem methods. This is intentional: async operations integrate cleanly with the deno_core event loop and do not block the V8 thread during I/O.

## Policy Input Schema

Every filesystem operation is evaluated against the policy chain before execution. The policy input includes:

```json
{
  "operation": "readFile",
  "path": "/tmp/data.txt",
  "destination": null,
  "recursive": null,
  "encoding": "utf8",
  "mcp_headers": { "x-mcp-session-id": "abc-123" }
}
```

Fields vary by operation:

- `operation` -- The fs method name: readFile, writeFile, appendFile, readdir, stat, mkdir, rm, rename, copyFile, exists, unlink
- `path` -- The primary path being operated on
- `destination` -- Present for rename and copyFile (the target path)
- `recursive` -- Present for mkdir and rm (boolean)
- `encoding` -- Present for readFile ("utf8" or "buffer")
- `mcp_headers` -- MCP session headers from the transport layer, enabling per-session access control

The `mcp_headers` field is particularly useful for multi-tenant deployments. A Rego policy can use it to restrict each session to its own directory:

```rego
package mcp.filesystem
default allow = false

allow if {
  session_id := input.mcp_headers["x-mcp-session-id"]
  startswith(input.path, concat("/", ["/data/workspace", session_id]))
}
```

## Binary Data via Uint8Array

Binary file operations use `Uint8Array` for data transfer. This is handled through deno_core's native `#[buffer]` support, which transfers binary data directly between JavaScript and Rust without base64 encoding. This makes binary file operations efficient even for large files (subject to the V8 heap and ArrayBuffer allocator limits).

When reading:
- `fs.readFile(path)` returns a UTF-8 string
- `fs.readFile(path, "buffer")` returns a `Uint8Array`

When writing:
- `fs.writeFile(path, string)` writes UTF-8 text
- `fs.writeFile(path, uint8array)` writes raw bytes

## OPA Gating

The policy evaluation flow for filesystem operations is:

1. The JavaScript fs wrapper calls the appropriate Rust op (e.g., `op_fs_read_file_text`).
2. The op extracts the `FsConfig` from OpState, including the policy chain and MCP headers.
3. A new Tokio task is spawned (to avoid RefCell issues in the op driver).
4. The task constructs the policy input and evaluates it against the chain.
5. If the policy denies the operation, an error is returned: `"fs.readFile denied by policy: /secret/file is not allowed"`.
6. If the policy allows the operation, the actual I/O is performed using Tokio's async filesystem APIs.

The spawned-task pattern ensures that policy evaluation (which may involve a remote OPA HTTP call) does not conflict with deno_core's internal futures management.
