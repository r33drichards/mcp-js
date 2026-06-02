# Filesystem Access Reference

The `fs` global is available when the `filesystem` section is configured in `--policies-json`. Every operation is evaluated against the policy chain before execution.

## Operations

All operations return Promises.

### fs.readFile(path, encoding?)

Read a file. Returns a string (UTF-8) or `Uint8Array` depending on encoding.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `path` | `string` | Yes | -- | File path |
| `encoding` | `string?` | No | (UTF-8 text) | `"buffer"` for raw bytes |

**Returns:** `Promise<string>` (default) or `Promise<Uint8Array>` (when encoding is `"buffer"`)

### fs.writeFile(path, data)

Write data to a file. Creates the file if it does not exist.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | File path |
| `data` | `string \| Uint8Array` | Yes | Content to write |

**Returns:** `Promise<void>`

### fs.appendFile(path, data)

Append data to a file.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | File path |
| `data` | `string` | Yes | Content to append |

**Returns:** `Promise<void>`

### fs.readdir(path)

List directory entries.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | Directory path |

**Returns:** `Promise<string[]>`

### fs.stat(path)

Get file or directory metadata.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | File or directory path |

**Returns:** `Promise<object>` with fields:

| Field | Type | Description |
|-------|------|-------------|
| `size` | `number` | File size in bytes |
| `isFile` | `boolean` | `true` if regular file |
| `isDirectory` | `boolean` | `true` if directory |
| `isSymlink` | `boolean` | `true` if symbolic link |
| `modified` | `number?` | Last modified time (seconds since epoch) |
| `accessed` | `number?` | Last accessed time (seconds since epoch) |
| `created` | `number?` | Creation time (seconds since epoch) |

### fs.mkdir(path, options?)

Create a directory.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | Directory path |
| `options` | `object?` | No | `{ recursive: boolean }` |

**Returns:** `Promise<void>`

### fs.rm(path, options?)

Remove a file or directory.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | File or directory path |
| `options` | `object?` | No | `{ recursive: boolean }` |

**Returns:** `Promise<void>`

### fs.rename(oldPath, newPath)

Rename or move a file.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `oldPath` | `string` | Yes | Source path |
| `newPath` | `string` | Yes | Destination path |

**Returns:** `Promise<void>`

### fs.copyFile(src, dest)

Copy a file.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `src` | `string` | Yes | Source file path |
| `dest` | `string` | Yes | Destination file path |

**Returns:** `Promise<void>`

### fs.exists(path)

Check if a path exists.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | File or directory path |

**Returns:** `Promise<boolean>`

### fs.unlink(path)

Remove a file (alias for `rm` without recursive).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | `string` | Yes | File path |

**Returns:** `Promise<void>`

## Policy Input Schema

Each filesystem operation generates this policy input:

```json
{
  "operation": "readFile",
  "path": "/tmp/data.txt",
  "destination": null,
  "recursive": null,
  "encoding": "utf8",
  "mcp_headers": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `operation` | `string` | Operation name: `readFile`, `writeFile`, `appendFile`, `readdir`, `stat`, `mkdir`, `rm`, `rename`, `copyFile`, `exists`, `unlink` |
| `path` | `string` | Primary path |
| `destination` | `string?` | Destination path (for `rename`, `copyFile`) |
| `recursive` | `boolean?` | Recursive flag (for `mkdir`, `rm`) |
| `encoding` | `string?` | Encoding (for `readFile`: `"utf8"` or `"buffer"`) |
| `mcp_headers` | `object?` | X-MCP-* headers from the MCP initialize request |

## Policy Configuration

Filesystem policies are configured in the `filesystem` section of `--policies-json`:

```json
{
  "filesystem": {
    "mode": "all",
    "policies": [
      {"url": "file:///path/to/fs.rego"}
    ]
  }
}
```

Default OPA REST path: `mcp/filesystem`
Default local Rego rule: `data.mcp.filesystem.allow`

## Binary Data Transfer

Binary data (for `readFile` with `"buffer"` encoding and `writeFile` with `Uint8Array`) is transferred directly through deno_core's native `#[buffer]` support. No base64 encoding is used.
