# Subprocess execution

In this tutorial we'll enable subprocess support, write a permissive policy, and run a shell command from JavaScript that captures its output.

## Prerequisites

- `mcp-v8` installed (see `../install/overview.md`)
- A working directory with write access

## Step 1 — Write a permissive subprocess policy

Create a file `policies/subprocess-allow-all.rego`:

```rego
package mcp.subprocess

default allow = true
```

This policy grants every subprocess call unconditionally.  We'll tighten it in the how-to guide once we know everything works.

## Step 2 — Build the policies JSON config

Create `policies.json` in your working directory:

```json
{
  "subprocess": {
    "policies": [
      { "url": "file://policies/subprocess-allow-all.rego" }
    ]
  }
}
```

## Step 3 — Start the server with the policy

```bash
mcp-v8 --http-port 8080 --policies-json policies.json
```

You should see a log line confirming the policy chain loaded:

```
Subprocess policy chain: 1 evaluator(s), mode=Any
```

## Step 4 — Run a shell command with `child_process.exec`

Call the `run_js` tool with the following JavaScript.  We'll use `child_process.exec`, which passes the command string to `/bin/sh -c`:

```js
const result = await child_process.exec("echo 'Hello from subprocess'");
result.stdout   // "Hello from subprocess\n"
result.code     // 0
result.success  // true
```

A complete `run_js` call over the REST API:

```bash
curl -s -X POST http://localhost:8080/api/exec \
  -H "Content-Type: application/json" \
  -d '{
    "code": "const r = await child_process.exec(\"echo Hello from subprocess\"); r.stdout"
  }'
```

Expected response (stateless mode) or execution ID (stateful mode).  Poll `GET /api/executions/{id}/output` to retrieve the output.

## Step 5 — Capture stdout and stderr separately

```js
const { code, stdout, stderr, success } = await child_process.exec(
  "ls /no-such-dir 2>&1; echo done"
);
// stdout contains the ls error message and "done\n"
// code is 0 because the last command (echo) succeeded
```

## Step 6 — Use `Deno.Command` for direct binary execution

`Deno.Command` executes a binary directly without a shell:

```js
const cmd = new Deno.Command("date", { args: ["-u"] });
const { code, stdout, success } = await cmd.output();
// stdout is a Uint8Array; decode it to a string
const text = new TextDecoder().decode(stdout);
text.trim()  // e.g. "Thu Jun  5 00:00:00 UTC 2026"
```

## What we built

- A permissive subprocess Rego policy loaded via `--policies-json`
- A working `child_process.exec` call that returns `{ stdout, stderr, code, success }`
- A `Deno.Command` call that returns `{ stdout: Uint8Array, stderr: Uint8Array, code, success }`

## See also

- [How-to: Subprocess execution](../how-to/subprocess.md)
- [Concepts: Subprocess execution](../concepts/subprocess.md)
- [Reference: Subprocess execution](../reference/subprocess.md)
- [Security policies (OPA/Rego)](../concepts/policies.md)
- [Filesystem access](../how-to/filesystem.md)
- [CLI flags reference](../reference/cli-flags.md)
