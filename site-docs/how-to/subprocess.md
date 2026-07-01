# Subprocess execution

Practical recipes for enabling subprocess support, restricting what commands can run, and working with process output.

## Enable subprocess

Subprocess is **disabled by default**.  To enable it, supply a `subprocess` key in the policies JSON passed to `--policies-json`.  Without it, any call to `Deno.Command` or `child_process.exec` throws immediately.

Create a minimal `policies.json`:

```json
{
  "subprocess": {
    "policies": [
      { "url": "file:///path/to/subprocess.rego" }
    ]
  }
}
```

Start the server:

```bash
mcp-v8 --http-port 8080 --policies-json /path/to/policies.json
```

## Allow only specific commands via Rego

### Allow a fixed set of binaries (`Deno.Command`)

`Deno.Command` uses `operation == "command_output"`.  The `input.command` field holds the binary name or path exactly as passed to the constructor.

```rego
package mcp.subprocess

default allow = false

allowed_commands := {"echo", "date", "uname"}

allow if {
    input.operation == "command_output"
    allowed_commands[input.command]
}
```

### Allow specific shell-command prefixes (`child_process.exec`)

`child_process.exec` uses `operation == "exec"`.  The command string passed to the shell is in `input.args[1]` (because the actual binary is `/bin/sh` with `-c` as `args[0]`).

```rego
package mcp.subprocess

default allow = false

allow if {
    input.operation == "exec"
    startswith(input.args[1], "echo ")
}

allow if {
    input.operation == "exec"
    startswith(input.args[1], "ls ")
}
```

### Restrict by working directory

You can gate all subprocess operations on the `cwd` field:

```rego
package mcp.subprocess

default allow = false

allow if {
    input.cwd != null
    startswith(input.cwd, "/tmp/sandbox/")
}
```

### Combine multiple rules

Rules compose naturally: add `allow` clauses for each permitted case.  The policy evaluator uses `data.mcp.subprocess.allow` as its entrypoint.

## Capture stdout

`child_process.exec` returns stdout as a UTF-8 string by default:

```js
const { stdout } = await child_process.exec("hostname");
stdout.trim();  // "myhost"
```

`Deno.Command.output()` always returns `stdout` as a `Uint8Array`; decode it:

```js
const cmd = new Deno.Command("hostname");
const { stdout } = await cmd.output();
new TextDecoder().decode(stdout).trim();  // "myhost"
```

## Capture stderr

Both APIs expose a `stderr` field alongside `stdout`:

```js
const { stdout, stderr, code } = await child_process.exec("ls /nonexistent");
// stderr: "ls: /nonexistent: No such file or directory\n"
// code:   1
// success: false
```

## Check the exit code

Both APIs return `code` (integer) and `success` (boolean):

```js
const { code, success } = await child_process.exec("false");
// code:    1
// success: false
```

`code` is `-1` when the process was killed by a signal and no numeric exit code is available.

## Get binary output

Set `encoding: "buffer"` on `child_process.exec` to receive `stdout`/`stderr` as `Uint8Array` instead of strings:

```js
const { stdout } = await child_process.exec("cat /bin/sh", { encoding: "buffer" });
// stdout is Uint8Array containing raw bytes
```

`Deno.Command.output()` always returns `Uint8Array` regardless of content.

## Set a working directory

Pass `cwd` in the options object:

```js
const { stdout } = await child_process.exec("pwd", { cwd: "/tmp" });
// stdout: "/tmp\n"

const cmd = new Deno.Command("pwd", { cwd: "/var" });
const { stdout } = await cmd.output();
new TextDecoder().decode(stdout).trim();  // "/var"
```

## Inject environment variables

```js
const { stdout } = await child_process.exec("printenv MY_VAR", {
  env: { MY_VAR: "hello" },
});
// stdout: "hello\n"
```

The environment provided replaces additional variables; the process inherits the server's environment unless you override it.

## Use a remote OPA server

Point the policy URL at a running OPA instance:

```json
{
  "subprocess": {
    "policies": [
      { "url": "http://opa.internal:8181", "policy_path": "mcp/subprocess" }
    ]
  }
}
```

The server posts the input document to OPA and checks `data.mcp.subprocess.allow`.

## See also

- [Concepts: Subprocess execution](../concepts/subprocess.md)
- [Reference: Subprocess execution](../reference/subprocess.md)
- [Security policies (OPA/Rego)](../how-to/policies.md)
- [Filesystem access](../how-to/filesystem.md)
- [CLI flags reference](../reference/cli-flags.md)
