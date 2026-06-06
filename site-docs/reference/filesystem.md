# Filesystem access

Complete reference for the `fs` global available in JS code and the policy input passed to the `filesystem` policy chain.

## Availability

The `fs` global is injected only when the server starts with a `filesystem` entry in `--policies-json`. Without it, any reference to `fs` in JS code throws `ReferenceError: fs is not defined`.

## JS API

All methods are `async` and must be `await`ed.

---

### `fs.readFile(path, encoding?)`

Read the entire contents of a file.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Absolute or relative path to the file |
| `encoding` | `string` | no | Pass `"buffer"` to receive raw bytes; omit for UTF-8 text |

**Returns:** `Promise<string>` when `encoding` is omitted or any value other than `"buffer"`. `Promise<Uint8Array>` when `encoding === "buffer"`.

**Errors:**
- `TypeError: fs.readFile: path must be a string` — `path` argument is not a string.
- `fs.readFile: <path>: <io error>` — file does not exist, permission denied, or other OS error.
- `fs.readFile: invalid UTF-8 in <path>: ...` — file contains non-UTF-8 bytes (text mode only).
- `fs.readFile denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"readFile"`. Policy `encoding` field: `"utf8"` (text mode) or `"buffer"` (binary mode).

---

### `fs.writeFile(path, data)`

Write data to a file, creating or truncating it.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to write |
| `data` | `string \| Uint8Array` | yes | Content to write. `Uint8Array` is written as raw bytes; anything else is coerced with `String(data)` |

**Returns:** `Promise<void>`

**Errors:**
- `TypeError: fs.writeFile: path must be a string` — `path` argument is not a string.
- `fs.writeFile: <path>: <io error>` — permission denied, directory does not exist, or other OS error.
- `fs.writeFile denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"writeFile"`.

---

### `fs.appendFile(path, data)`

Append text to a file. The file is created if it does not exist.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to append to |
| `data` | `string` | yes | Content to append (coerced with `String(data)`) |

**Returns:** `Promise<void>`

**Errors:**
- `TypeError: fs.appendFile: path must be a string` — `path` argument is not a string.
- `fs.appendFile: <path>: <io error>` — permission denied or other OS error.
- `fs.appendFile denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"appendFile"`.

---

### `fs.readdir(path)`

List the entries in a directory.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to the directory |

**Returns:** `Promise<string[]>` — array of entry names (filenames only, not full paths). Order is platform-dependent.

**Errors:**
- `TypeError: fs.readdir: path must be a string` — `path` argument is not a string.
- `fs.readdir: <path>: <io error>` — directory does not exist, is not a directory, permission denied, or other OS error.
- `fs.readdir denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"readdir"`.

---

### `fs.stat(path)`

Retrieve metadata for a file or directory.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to stat |

**Returns:** `Promise<StatResult>`

**`StatResult` object:**

| Field | Type | Description |
|---|---|---|
| `size` | `number` | File size in bytes |
| `isFile` | `boolean` | True if the path is a regular file |
| `isDirectory` | `boolean` | True if the path is a directory |
| `isSymlink` | `boolean` | True if the path is a symbolic link |
| `readonly` | `boolean` | True if the file permissions are read-only |
| `mtimeMs` | `number \| null` | Modification time as milliseconds since Unix epoch; `null` if unavailable |
| `atimeMs` | `number \| null` | Last access time as milliseconds since Unix epoch; `null` if unavailable |
| `birthtimeMs` | `number \| null` | Creation time as milliseconds since Unix epoch; `null` if unavailable on the platform |

**Errors:**
- `TypeError: fs.stat: path must be a string` — `path` argument is not a string.
- `fs.stat: <path>: <io error>` — path does not exist, permission denied, or other OS error.
- `fs.stat denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"stat"`.

---

### `fs.mkdir(path, options?)`

Create a directory.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Directory path to create |
| `options` | `object` | no | Options object |
| `options.recursive` | `boolean` | no | If `true`, creates all intermediate directories (like `mkdir -p`). Default: `false` |

**Returns:** `Promise<void>`

**Errors:**
- `TypeError: fs.mkdir: path must be a string` — `path` argument is not a string.
- `fs.mkdir: <path>: <io error>` — parent directory missing (when `recursive` is false), already exists, permission denied, or other OS error.
- `fs.mkdir denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"mkdir"`. Policy `recursive` field: `true` or `false`.

---

### `fs.rm(path, options?)`

Remove a file or directory.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to remove |
| `options` | `object` | no | Options object |
| `options.recursive` | `boolean` | no | If `true` and the path is a directory, removes it and all its contents. Default: `false` |

**Returns:** `Promise<void>`

**Errors:**
- `TypeError: fs.rm: path must be a string` — `path` argument is not a string.
- `fs.rm: <path>: <io error>` — path does not exist, directory not empty (when `recursive` is false), permission denied, or other OS error.
- `fs.rm denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"rm"`. Policy `recursive` field: `true` or `false`.

---

### `fs.unlink(path)`

Remove a file. Alias for `fs.rm(path)` without the `recursive` option.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to the file to remove |

**Returns:** `Promise<void>`

**Errors:** Same as `fs.rm`.

**Policy operation:** `"rm"` (same backend op as `fs.rm`). Policy `recursive` field: `false`.

---

### `fs.rename(oldPath, newPath)`

Rename or move a file or directory.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `oldPath` | `string` | yes | Current path |
| `newPath` | `string` | yes | Destination path |

**Returns:** `Promise<void>`

**Errors:**
- `TypeError: fs.rename: oldPath must be a string` — `oldPath` argument is not a string.
- `TypeError: fs.rename: newPath must be a string` — `newPath` argument is not a string.
- `fs.rename: <oldPath> -> <newPath>: <io error>` — source does not exist, cross-device move not supported, permission denied, or other OS error.
- `fs.rename denied by policy: <oldPath> is not allowed` — policy chain returned false.

**Policy operation:** `"rename"`. Policy `path` field: `oldPath`; policy `destination` field: `newPath`.

---

### `fs.copyFile(src, dest)`

Copy a file. Overwrites the destination if it exists.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `src` | `string` | yes | Source file path |
| `dest` | `string` | yes | Destination file path |

**Returns:** `Promise<void>`

**Errors:**
- `TypeError: fs.copyFile: src must be a string` — `src` argument is not a string.
- `TypeError: fs.copyFile: dest must be a string` — `dest` argument is not a string.
- `fs.copyFile: <src> -> <dest>: <io error>` — source does not exist, permission denied, or other OS error.
- `fs.copyFile denied by policy: <src> is not allowed` — policy chain returned false.

**Policy operation:** `"copyFile"`. Policy `path` field: `src`; policy `destination` field: `dest`.

---

### `fs.exists(path)`

Check whether a path exists.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to check |

**Returns:** `Promise<boolean>` — `true` if the path exists and is accessible; `false` if it does not exist or cannot be checked.

**Errors:**
- `TypeError: fs.exists: path must be a string` — `path` argument is not a string.
- `fs.exists denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"exists"`.

---

## Policy input fields

Every `fs.*` call evaluates the `filesystem` policy chain with the following input object. The Rego entrypoint is `data.mcp.filesystem.allow`.

| Field | Type | Always present | Description |
|---|---|---|---|
| `operation` | `string` | yes | JS method name: one of `readFile`, `writeFile`, `appendFile`, `readdir`, `stat`, `mkdir`, `rm`, `rename`, `copyFile`, `exists` |
| `path` | `string` | yes | The primary path argument |
| `destination` | `string` | no | Present for `rename` (the `newPath`) and `copyFile` (the `dest`) |
| `recursive` | `boolean` | no | Present for `mkdir` and `rm`; reflects `options.recursive` |
| `encoding` | `string` | no | Present for `readFile` only: `"utf8"` (text mode) or `"buffer"` (binary mode) |
| `mcp_headers` | `object` | no | MCP session headers forwarded by the server when configured |

### Example input — text read

```json
{
  "operation": "readFile",
  "path": "/var/mcp-workspace/config.json",
  "encoding": "utf8"
}
```

### Example input — rename with destination

```json
{
  "operation": "rename",
  "path": "/var/mcp-workspace/draft.txt",
  "destination": "/var/mcp-workspace/final.txt"
}
```

### Example input — recursive mkdir

```json
{
  "operation": "mkdir",
  "path": "/var/mcp-workspace/reports/2024",
  "recursive": true
}
```

## Policies configuration schema

The `filesystem` key in `--policies-json` accepts an `OperationPolicies` object:

| Field | Type | Default | Description |
|---|---|---|---|
| `mode` | `"all"` \| `"any"` | `"all"` | `"all"`: every evaluator must allow; `"any"`: at least one must allow |
| `policies` | `PolicySource[]` | — | One or more policy sources |

Each `PolicySource`:

| Field | Type | Required | Description |
|---|---|---|---|
| `url` | `string` | yes | `file:///path/to/policy.rego` (local file or directory of `.rego` files), `http://…` or `https://…` (remote OPA instance) |
| `policy_path` | `string` | no | Remote OPA path segment; defaults to `mcp/filesystem` |
| `rule` | `string` | no | Rego rule to evaluate for local files; defaults to `data.mcp.filesystem.allow` |

## See also

- [Quick-start: Filesystem access](../tutorials/filesystem.md)
- [How-to: Filesystem access](../how-to/filesystem.md)
- [Concepts: Filesystem access](../concepts/filesystem.md)
- [Reference: CLI flags](../reference/cli-flags.md)
- [How-to: Security policies](../how-to/policies.md)
- [Reference: MCP tools](../reference/mcp-tools.md)
