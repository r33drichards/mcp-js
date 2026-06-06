# Subprocess execution

Complete reference for the subprocess JS API, options, return shapes, errors, and policy input fields.

## Enabling subprocess

Subprocess is disabled unless the server is started with a `--policies-json` value that contains a `subprocess` key.  No subprocess key means no subprocess capability, regardless of any default-allow policy for other features.

```json
{
  "subprocess": {
    "policies": [
      { "url": "file:///path/to/subprocess.rego" }
    ]
  }
}
```

The policy entrypoint is `data.mcp.subprocess.allow`.

---

## `Deno.Command`

### Constructor

```js
new Deno.Command(command, options?)
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | `string` | yes | Binary name or absolute path to execute |
| `options.args` | `string[]` | no | Argument vector passed to the process (default `[]`) |
| `options.cwd` | `string` | no | Working directory for the child process |
| `options.env` | `Record<string, string>` | no | Additional environment variables merged into the process environment |

### `cmd.output()` → `Promise<CommandOutput>`

Spawns the process, waits for it to exit, and returns all buffered output.

```js
const { code, success, stdout, stderr, signal } = await cmd.output();
```

**Return shape — `CommandOutput`:**

| Field | Type | Description |
|-------|------|-------------|
| `code` | `number` | Exit code; `-1` if the process was killed by a signal with no exit code |
| `success` | `boolean` | `true` if `code === 0` |
| `stdout` | `Uint8Array` | Raw bytes written to stdout |
| `stderr` | `Uint8Array` | Raw bytes written to stderr |
| `signal` | `null` | Always `null` (signal reporting not implemented) |

`stdout` and `stderr` are always `Uint8Array`.  Use `new TextDecoder().decode(stdout)` to obtain a UTF-8 string.

### `cmd.outputSync()`

Not implemented.  Throws `Error: Deno.Command.outputSync() is not supported; use output() instead`.

### `cmd.spawn()`

Not yet implemented.  Throws `Error: Deno.Command.spawn() is not yet supported; use output() instead`.

---

## `child_process.exec`

### Signature

```js
child_process.exec(command, options?) → Promise<ExecOutput>
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | `string` | yes | Shell command string passed to `/bin/sh -c` (POSIX) or `cmd /C` (Windows) |
| `options.cwd` | `string` | no | Working directory for the shell process |
| `options.env` | `Record<string, string>` | no | Additional environment variables |
| `options.encoding` | `"utf8"` \| `"buffer"` | no | Output encoding; default `"utf8"` |
| `options.timeout` | `number` | no | Reserved; currently has no effect |

**Return shape — `ExecOutput`:**

| Field | Type | Description |
|-------|------|-------------|
| `code` | `number` | Exit code; `-1` if unavailable |
| `success` | `boolean` | `true` if `code === 0` |
| `stdout` | `string` \| `Uint8Array` | stdout as UTF-8 string (`encoding="utf8"`) or raw bytes (`encoding="buffer"`) |
| `stderr` | `string` \| `Uint8Array` | stderr, same encoding as stdout |

---

## Policy input document

The following JSON object is passed to the Rego policy as `input` on every subprocess call.

| Field | Type | When present | Description |
|-------|------|--------------|-------------|
| `operation` | `string` | always | `"command_output"` for `Deno.Command`, `"exec"` for `child_process.exec` |
| `command` | `string` | always | The executable: exact value for `Deno.Command`; `/bin/sh` (POSIX) or `cmd` (Windows) for `child_process.exec` |
| `args` | `string[]` | always | Argument vector: `options.args` for `Deno.Command`; `["-c", "<shell command>"]` for `child_process.exec` |
| `cwd` | `string` | if provided | Working directory passed in options |
| `env` | `object` | if provided | Environment key-value pairs passed in options |

`cwd` and `env` are omitted from the document (not serialized) when not supplied by the caller.

### Example input for `Deno.Command`

```json
{
  "operation": "command_output",
  "command": "ls",
  "args": ["-la", "/tmp"],
  "cwd": "/tmp"
}
```

### Example input for `child_process.exec`

```json
{
  "operation": "exec",
  "command": "/bin/sh",
  "args": ["-c", "echo hello"]
}
```

---

## Errors

| Condition | Error message |
|-----------|---------------|
| Subprocess capability not enabled (no policy config) | `subprocess: internal error — no subprocess config available` |
| Policy denies the call | `subprocess.{operation} denied by policy: '{command}' is not allowed` |
| Policy evaluation error | `subprocess.{operation}: policy error: {detail}` |
| Process failed to start (binary not found, permission error, etc.) | `subprocess: failed to execute '{command}': {os error}` |
| `child_process.exec` process failed to start | `subprocess.exec: failed to execute '{command}': {os error}` |
| `Deno.Command` `command` argument is not a string | `TypeError: Deno.Command: command must be a string` |
| `args` is not an array | `TypeError: Deno.Command: args must be an array` |
| `child_process.exec` `command` argument is not a string | `TypeError: child_process.exec: command must be a string` |

---

## Server configuration

`--policies-json` accepts either an inline JSON string or a file path.  The `subprocess` object within it follows the `OperationPolicies` shape:

```json
{
  "subprocess": {
    "mode": "any",
    "policies": [
      { "url": "file:///path/to/policy.rego" },
      { "url": "http://opa.internal:8181", "policy_path": "mcp/subprocess" }
    ]
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `"any"` \| `"all"` | `"any"` | `"any"`: allow if any evaluator allows; `"all"`: require every evaluator to allow |
| `policies[].url` | `string` | — | `file://` (local file or directory) or `http(s)://` (remote OPA) |
| `policies[].policy_path` | `string` | `"mcp/subprocess"` | OPA policy path for remote evaluators |
| `policies[].rule` | `string` | `"data.mcp.subprocess.allow"` | Rego rule entrypoint for local evaluators |

## See also

- [Quick-start: Subprocess execution](../tutorials/subprocess.md)
- [How-to: Subprocess execution](../how-to/subprocess.md)
- [Concepts: Subprocess execution](../concepts/subprocess.md)
- [Security policies (OPA/Rego)](../reference/policies.md)
- [Filesystem access](../reference/filesystem.md)
- [CLI flags reference](../reference/cli-flags.md)
