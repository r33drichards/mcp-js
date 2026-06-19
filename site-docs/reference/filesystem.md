# Filesystem access

Complete reference for the `fs` global available in JS code and the policy input passed to the `filesystem` policy chain.

## Availability

The `fs` global is injected only when the server starts with a `filesystem` entry in `--policies-json`. Without it, any reference to `fs` in JS code throws `ReferenceError: fs is not defined`.

## JS API

All methods are `async` and must be `await`ed.

The same methods are also exposed under **`fs.promises`** (an enumerable own
property), and rejected operations carry a Node-style **`code`** (e.g.
`err.code === "ENOENT"`). Together with the `fs.Stats`-like object returned by
`fs.stat`/`fs.lstat`, this lets libraries that expect a Node `fs` / `fs.promises`
interface — such as [`isomorphic-git`](https://isomorphic-git.org/) — consume the
sandbox `fs` object directly. See [`fs.promises`](#fspromises) and
[Error codes](#error-codes) below.

> Both `fs.readFile` and `fs.promises.readFile` follow Node semantics — they
> return a `Uint8Array` by default and a string only when a text encoding (e.g.
> `"utf8"`) is given.

---

### `fs.readFile(path, encoding?)`

Read the entire contents of a file.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Absolute or relative path to the file |
| `encoding` | `string` | no | Pass a text encoding (e.g. `"utf8"`) to receive a string; omit for raw bytes |

**Returns:** `Promise<Uint8Array>` when `encoding` is omitted or `"buffer"` (the Node-style default). `Promise<string>` when a text encoding such as `"utf8"` is given.

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

Retrieve metadata for a file or directory. A final symlink is followed.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to stat |

**Returns:** `Promise<Stats>` — a Node [`fs.Stats`](https://nodejs.org/api/fs.html#class-fsstats)-like object.

**`Stats` object:**

| Member | Type | Description |
|---|---|---|
| `isFile()` | `() => boolean` | True if the path is a regular file |
| `isDirectory()` | `() => boolean` | True if the path is a directory |
| `isSymbolicLink()` | `() => boolean` | True if the path is a symbolic link |
| `isBlockDevice()`, `isCharacterDevice()`, `isFIFO()`, `isSocket()` | `() => boolean` | Always `false` (not represented) |
| `size` | `number` | File size in bytes |
| `mode` | `number` | File mode bits (type + permissions) |
| `ino`, `dev`, `nlink`, `uid`, `gid` | `number` | POSIX metadata (`0`/`1` when unavailable, e.g. on a snapshot overlay) |
| `mtimeMs`, `atimeMs`, `ctimeMs`, `birthtimeMs` | `number` | Timestamps in milliseconds since the Unix epoch (`0` if unavailable) |
| `mtime`, `atime`, `ctime`, `birthtime` | `Date` | The same timestamps as `Date` objects |
| `readonly` | `boolean` | True if the file permissions are read-only |

> **Compatibility note:** `isFile`/`isDirectory`/`isSymbolicLink` are now **methods**
> (call them: `stats.isFile()`), matching Node's `fs.Stats`. Earlier releases exposed
> `isFile`/`isDirectory`/`isSymlink` as boolean properties.

**Errors:**
- `TypeError: fs.stat: path must be a string` — `path` argument is not a string.
- `fs.stat: <path>: <CODE>: <io error>` — path does not exist (`ENOENT`), permission denied (`EACCES`), or other OS error. The rejection's `code` carries `<CODE>`.
- `fs.stat denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"stat"`.

---

### `fs.lstat(path)`

Like [`fs.stat`](#fsstatpath), but does **not** follow a final symlink — it returns
the metadata of the link itself (so `isSymbolicLink()` is `true` for a symlink).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to stat without following a final symlink |

**Returns:** `Promise<Stats>` — same shape as [`fs.stat`](#fsstatpath).

**Errors:** Same shapes as `fs.stat`, prefixed `fs.lstat`.

**Policy operation:** `"lstat"`.

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

### `fs.rmdir(path, options?)`

Remove a directory. Provided for Node `fs.promises` compatibility; backed by the
same operation as [`fs.rm`](#fsrmpath-options).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Directory path to remove |
| `options` | `object` | no | Options object |
| `options.recursive` | `boolean` | no | If `true`, removes the directory and its contents. Default: `false` |

**Returns:** `Promise<void>`

**Errors:** Same as `fs.rm` (e.g. `ENOTEMPTY` when removing a non-empty directory without `recursive`).

**Policy operation:** `"rm"` (same backend op as `fs.rm`).

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

### `fs.symlink(target, path)`

Create a symbolic link at `path` pointing to `target`. Note the Node argument
order: the **target comes first**.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `target` | `string` | yes | The path the link points to |
| `path` | `string` | yes | The link to create |

**Returns:** `Promise<void>`

**Errors:**
- `TypeError: fs.symlink: target must be a string` / `fs.symlink: path must be a string` — argument is not a string.
- `fs.symlink: <path> -> <target>: <CODE>: <io error>` — e.g. the link already exists (`EEXIST`), or symlinks are unsupported on the platform (`ENOSYS`).
- `fs.symlink denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"symlink"`. Policy `path` field: the link being created; policy `destination` field: `target`.

---

### `fs.readlink(path)`

Read the target of a symbolic link.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | `string` | yes | Path to the symbolic link |

**Returns:** `Promise<string>` — the link's target path.

**Errors:**
- `TypeError: fs.readlink: path must be a string` — `path` argument is not a string.
- `fs.readlink: <path>: <CODE>: <io error>` — path does not exist (`ENOENT`) or is not a symlink (`EINVAL`).
- `fs.readlink denied by policy: <path> is not allowed` — policy chain returned false.

**Policy operation:** `"readlink"`.

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

### `fs.promises`

`fs.promises` is an enumerable object exposing the same promise-returning
methods: `readFile`, `writeFile`, `appendFile`, `readdir`, `stat`, `lstat`,
`mkdir`, `rm`, `rmdir`, `unlink`, `rename`, `copyFile`, `readlink`, `symlink`,
and `exists`.

It exists so libraries that detect a Node `fs.promises` interface — by reading
the enumerable `promises` property and binding its methods — work against the
sandbox `fs`. `fs.promises.readFile` follows Node semantics (a `Uint8Array` by
default; a string when a text encoding is supplied).

```js
import git from "npm:isomorphic-git";
await git.init({ fs, dir: "/repo" });   // pass the sandbox `fs` directly
await fs.promises.writeFile("/repo/README.md", "# hi");
await git.add({ fs, dir: "/repo", filepath: "README.md" });
```

---

### Error codes

Rejections from `fs.*` carry a Node-style `code` property where a POSIX code is
known, so callers can branch on it:

```js
try {
  await fs.promises.readFile("/no/such/file");
} catch (e) {
  if (e.code === "ENOENT") { /* handle missing file */ }
}
```

Mapped codes include `ENOENT`, `EEXIST`, `EACCES`, `ENOTDIR`, `EISDIR`,
`ENOTEMPTY`, `EROFS`, `ENOSYS`, and `EINVAL` (from `fs.readlink` on a
non-symlink). The code is also embedded in the error message.

---

## Policy input fields

Every `fs.*` call evaluates the `filesystem` policy chain with the following input object. The Rego entrypoint is `data.mcp.filesystem.allow`.

| Field | Type | Always present | Description |
|---|---|---|---|
| `operation` | `string` | yes | JS method name: one of `readFile`, `writeFile`, `appendFile`, `readdir`, `stat`, `lstat`, `mkdir`, `rm`, `rename`, `copyFile`, `symlink`, `readlink`, `exists` |
| `path` | `string` | yes | The primary path argument (for `symlink`, the link being created) |
| `destination` | `string` | no | Present for `rename` (the `newPath`), `copyFile` (the `dest`), and `symlink` (the `target`) |
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

- [How-to: Filesystem access](../how-to/filesystem.md)
- [Concepts: Filesystem access](../concepts/filesystem.md)
- [Reference: CLI flags](../reference/cli-flags.md)
- [How-to: Security policies](../how-to/policies.md)
- [Reference: MCP tools](../reference/mcp-tools.md)
