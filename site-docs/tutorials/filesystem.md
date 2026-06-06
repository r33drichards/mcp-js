# Filesystem access

We'll enable the `fs` global, write a file, and read it back — all from a `run_js` call.

## Prerequisites

- `mcp-v8` installed (see the [install overview](../install/overview.md))
- A terminal and the `mcp-v8-cli` CLI, or any MCP client

## 1. Write a permissive filesystem policy

The `fs` global is unavailable until a `filesystem` policy is configured. For this tutorial we'll allow every operation. Create the file `/tmp/fs-allow.rego`:

```rego
package mcp.filesystem

default allow = true
```

## 2. Create the policies configuration file

Create `/tmp/fs-policies.json`:

```json
{
  "filesystem": {
    "policies": [
      {"url": "file:///tmp/fs-allow.rego"}
    ]
  }
}
```

## 3. Start the server

```bash
mcp-v8 --http-port 8080 --policies-json /tmp/fs-policies.json
```

The server logs a confirmation line:

```
Filesystem policy chain: 1 evaluator(s), mode=All
```

## 4. Write a file from JS

Send this code via `run_js`:

```js
await fs.writeFile("/tmp/hello.txt", "Hello from mcp-v8!\n");
"done"
```

Expected: the call completes without error and returns `"done"`.

## 5. Read the file back

```js
const contents = await fs.readFile("/tmp/hello.txt");
contents
```

Expected output: `"Hello from mcp-v8!\n"`

## 6. Read as binary

Pass `"buffer"` as the second argument to receive a `Uint8Array` instead of a string:

```js
const buf = await fs.readFile("/tmp/hello.txt", "buffer");
buf.length
```

Expected output: `19` (byte length of the string plus newline).

## 7. List directory contents

```js
const entries = await fs.readdir("/tmp");
entries.filter(e => e === "hello.txt")
```

Expected output: `["hello.txt"]`

## Next steps

The permissive policy used here is not suitable for production. See the how-to guide for restricting access to a specific directory tree.

## See also

- [How-to: Filesystem access](../how-to/filesystem.md)
- [Concepts: Filesystem access](../concepts/filesystem.md)
- [Reference: Filesystem access](../reference/filesystem.md)
- [How-to: Security policies](../how-to/policies.md)
- [Reference: CLI flags](../reference/cli-flags.md)
