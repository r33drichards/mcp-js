# Filesystem access

Recipes for enabling `fs` access, restricting it to a sandbox directory, and performing common file operations from JS.

## Enable filesystem access

Filesystem access is off by default. To enable it, supply a `filesystem` entry in the `--policies-json` configuration. The value must be a JSON object (or a path to a JSON file) containing a `filesystem` key:

```json
{
  "filesystem": {
    "policies": [
      {"url": "file:///etc/mcp-v8/fs-policy.rego"}
    ]
  }
}
```

Pass it on startup:

```bash
mcp-v8 --http-port 8080 --policies-json /etc/mcp-v8/policies.json
```

Or inline:

```bash
mcp-v8 --http-port 8080 \
  --policies-json '{"filesystem":{"policies":[{"url":"file:///etc/mcp-v8/fs-policy.rego"}]}}'
```

The `fs` global becomes available to JS code as soon as the server starts with a valid policy chain. Without a `filesystem` entry the global is not injected and any attempt to access `fs` will throw a `ReferenceError`.

## Restrict access to a directory

Use a Rego policy that checks `input.path` (and `input.destination` for operations such as `rename` and `copyFile`) against an allowed prefix. Create `/etc/mcp-v8/fs-policy.rego`:

```rego
package mcp.filesystem

default allow = false

allow if {
    startswith(input.path, "/var/mcp-workspace/")
    check_destination
}

# Operations with no destination (readFile, writeFile, stat, etc.) always pass
check_destination if {
    not input.destination
}

# For rename and copyFile, the destination must also be within the sandbox
check_destination if {
    input.destination
    startswith(input.destination, "/var/mcp-workspace/")
}
```

Point your policies configuration at this file and restart the server. Any attempt to read or write outside `/var/mcp-workspace/` will be denied with:

```
fs.<operation> denied by policy: <path> is not allowed
```

## Restrict by operation

You can allow only specific operations by checking `input.operation`:

```rego
package mcp.filesystem

default allow = false

# Allow reads anywhere under the data directory but no writes
allow if {
    input.operation in {"readFile", "readdir", "stat", "exists"}
    startswith(input.path, "/data/")
}
```

Operation names match exactly what the JS wrapper passes: `readFile`, `writeFile`, `appendFile`, `readdir`, `stat`, `mkdir`, `rm`, `rename`, `copyFile`, `exists`.

Note: `fs.unlink` reuses the `rm` op internally, so its policy operation is `"rm"`.

## Use a remote OPA server

Replace the `file://` URL with an `http://` or `https://` URL pointing to a running OPA instance:

```json
{
  "filesystem": {
    "policies": [
      {"url": "http://opa.internal:8181", "policy_path": "mcp/filesystem"}
    ]
  }
}
```

The server POSTs to `http://opa.internal:8181/v1/data/mcp/filesystem` with an `{"input": ...}` body and expects `{"result": {"allow": true|false}}`.

## Chain multiple evaluators

Set `"mode": "all"` (the default) to require every evaluator to return `true`, or `"mode": "any"` to allow if at least one returns `true`:

```json
{
  "filesystem": {
    "mode": "all",
    "policies": [
      {"url": "file:///etc/mcp-v8/fs-path-check.rego"},
      {"url": "http://opa.internal:8181", "policy_path": "mcp/filesystem"}
    ]
  }
}
```

## Common read/write/list operations

### Read a text file

```js
const text = await fs.readFile("/var/mcp-workspace/config.json");
const config = JSON.parse(text);
```

### Read a binary file

```js
const bytes = await fs.readFile("/var/mcp-workspace/image.png", "buffer");
// bytes is a Uint8Array
```

### Write a text file (overwrites)

```js
await fs.writeFile("/var/mcp-workspace/output.txt", "result: ok\n");
```

### Write binary data

```js
const data = new Uint8Array([0x48, 0x69]);
await fs.writeFile("/var/mcp-workspace/blob.bin", data);
```

### Append to a file

```js
await fs.appendFile("/var/mcp-workspace/log.txt", new Date().toISOString() + "\n");
```

The file is created if it does not exist.

### List a directory

```js
const names = await fs.readdir("/var/mcp-workspace");
// returns string[] of filenames (not full paths)
```

### Get file metadata

```js
const info = await fs.stat("/var/mcp-workspace/config.json");
// { size, isFile, isDirectory, isSymlink, readonly, mtimeMs, atimeMs, birthtimeMs }
```

### Create a directory

```js
await fs.mkdir("/var/mcp-workspace/reports", { recursive: true });
```

### Check existence

```js
const exists = await fs.exists("/var/mcp-workspace/config.json");
```

### Remove a file

```js
await fs.unlink("/var/mcp-workspace/temp.txt");
// or equivalently:
await fs.rm("/var/mcp-workspace/temp.txt");
```

### Remove a directory tree

```js
await fs.rm("/var/mcp-workspace/old-reports", { recursive: true });
```

### Rename or move

```js
await fs.rename("/var/mcp-workspace/draft.txt", "/var/mcp-workspace/final.txt");
```

### Copy a file

```js
await fs.copyFile("/var/mcp-workspace/template.txt", "/var/mcp-workspace/output.txt");
```

## See also

- [Concepts: Filesystem access](../concepts/filesystem.md)
- [Reference: Filesystem access](../reference/filesystem.md)
- [How-to: Security policies](../how-to/policies.md)
- [Reference: CLI flags](../reference/cli-flags.md)
