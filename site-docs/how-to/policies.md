# Security policies (OPA/Rego)

Policies are written in [Rego](https://www.openpolicyagent.org/docs/latest/policy-language/) and are evaluated before each capability call. This guide shows focused recipes for common gating tasks. See the [reference](../reference/policies.md) for the full JSON schema and input documents.

## Gate network access by host and method

Write a `fetch.rego` that restricts to GET-only on a specific host:

```rego
package mcp.fetch

default allow = false

allow if {
    input.method == "GET"
    input.url_parsed.host == "api.example.com"
}
```

Enable it:

```bash
mcp-v8 --http-port 3000 \
  --policies-json '{"fetch":{"policies":[{"url":"file:///etc/mcp/fetch.rego"}]}}'
```

To allow a set of hosts, use a Rego set:

```rego
package mcp.fetch

default allow = false

allowed_hosts := {"api.example.com", "cdn.example.com"}

allow if {
    allowed_hosts[input.url_parsed.host]
    input.method == "GET"
}
```

To allow wildcard subdomains (e.g., `*.example.com`):

```rego
allow if {
    endswith(input.url_parsed.host, ".example.com")
}
```

## Gate filesystem access by path prefix

Write a `filesystem.rego` that restricts reads and writes to a specific directory:

```rego
package mcp.filesystem

default allow = false

# Operations with no destination (readFile, writeFile, mkdir, etc.)
allow if {
    startswith(input.path, "/data/workspace/")
    not input.destination
}

# For operations with a destination (rename, copyFile), check both paths.
allow if {
    startswith(input.path, "/data/workspace/")
    startswith(input.destination, "/data/workspace/")
}
```

Enable it:

```bash
mcp-v8 --http-port 3000 \
  --policies-json '{"filesystem":{"policies":[{"url":"file:///etc/mcp/filesystem.rego"}]}}'
```

The `input.operation` field carries the exact operation name (`readFile`, `writeFile`, `mkdir`, `rm`, `rename`, `copyFile`, `appendFile`, `stat`, `exists`, `readdir`). Gate specific operations by checking it:

```rego
# Allow reads everywhere but restrict writes.
allow if {
    input.operation == "readFile"
    startswith(input.path, "/data/")
}

allow if {
    input.operation == "writeFile"
    startswith(input.path, "/data/workspace/")
}
```

## Gate subprocess execution by command

Write a `subprocess.rego` that allows only specific commands:

```rego
package mcp.subprocess

default allow = false

# Deno.Command: allow echo and cat.
allowed_commands := {"echo", "cat"}

allow if {
    input.operation == "command_output"
    allowed_commands[input.command]
}

# child_process.exec: allow commands starting with "echo" or "cat".
allowed_exec_patterns := {"echo", "cat"}

allow if {
    input.operation == "exec"
    some pattern in allowed_exec_patterns
    startswith(input.args[1], pattern)
}
```

Enable it:

```bash
mcp-v8 --http-port 3000 \
  --policies-json '{"subprocess":{"policies":[{"url":"file:///etc/mcp/subprocess.rego"}]}}'
```

The `input.operation` field is `"command_output"` for `Deno.Command`, and `"exec"` for `child_process.exec`. For `exec`, `input.args[1]` is the shell command string.

To allow all subprocess calls only within a specific working directory:

```rego
allow if {
    input.cwd != null
    startswith(input.cwd, "/tmp/sandbox/")
}
```

## Gate ES module imports

Enable external module imports and gate them by package:

```rego
package mcp.modules

default allow = false

# Allow specific npm packages via esm.sh.
allowed_npm := {"lodash-es", "uuid"}

allow if {
    input.specifier_type == "npm"
    input.url_parsed.host == "esm.sh"
    some pkg in allowed_npm
    startswith(input.url_parsed.path, sprintf("/%s", [pkg]))
}

# Allow any URL from trusted CDN hosts.
trusted_hosts := {"cdn.jsdelivr.net", "unpkg.com"}

allow if {
    input.specifier_type == "url"
    trusted_hosts[input.url_parsed.host]
}
```

Enable both flags — `--allow-external-modules` unlocks the import mechanism, and the `modules` policy gates individual imports:

```bash
mcp-v8 --http-port 3000 \
  --allow-external-modules \
  --policies-json '{"modules":{"policies":[{"url":"file:///etc/mcp/modules.rego"}]}}'
```

The `input.specifier_type` is `"npm"` for `npm:` imports resolved via esm.sh, `"jsr"` for `jsr:` imports, and `"url"` for direct URL imports.

## Gate upstream MCP tool calls

Write a `mcp_tools.rego` that allows specific server/tool combinations:

```rego
package mcp.tools

default allow = false

# Allow all tools on the "math" server.
allow if {
    input.server == "math"
}

# Allow only the "query" tool on the "db" server.
allow if {
    input.server == "db"
    input.tool == "query"
}
```

Enable it (requires `--mcp-server` to be configured first):

```bash
mcp-v8 --http-port 3000 \
  --mcp-server math=stdio:math-server \
  --mcp-server db=stdio:db-server \
  --policies-json '{"mcp_tools":{"policies":[{"url":"file:///etc/mcp/mcp_tools.rego"}]}}'
```

The `input.arguments` field contains the tool call arguments as a JSON object (or null if no arguments were provided). Inspect argument values to enforce fine-grained access:

```rego
allow if {
    input.server == "db"
    input.tool == "query"
    input.arguments.database == "readonly_db"
}
```

## Use a remote OPA server instead of a local Rego file

Point the policy URL at a running [OPA](https://www.openpolicyagent.org/) server using `http://` or `https://`:

```json
{
  "fetch": {
    "policies": [{
      "url": "http://opa.internal:8181",
      "policy_path": "mcp/fetch"
    }]
  }
}
```

The server posts the input document to `POST /v1/data/{policy_path}` and reads `result.allow`. The default `policy_path` for each category is listed in the [reference](../reference/policies.md). Override it with `"policy_path"` when your OPA policy lives at a different path.

The OPA server must expose the standard OPA REST API. Start one with:

```bash
opa run --server --addr :8181 fetch.rego
```

## Load a Rego file via file URL

Use a `file://` URL with an absolute path:

```json
{
  "fetch": {
    "policies": [{"url": "file:///etc/mcp/fetch.rego"}]
  }
}
```

Override the default rule name with `"rule"` if your package path differs:

```json
{
  "fetch": {
    "policies": [{
      "url": "file:///etc/mcp/fetch.rego",
      "rule": "data.custom.pkg.allow"
    }]
  }
}
```

## Load a directory of Rego files

Point the `file://` URL at a directory and all `.rego` files in it are loaded into one regorus engine instance. Files are loaded in alphabetical order:

```json
{
  "fetch": {
    "policies": [{"url": "file:///etc/mcp/policies/"}]
  }
}
```

This lets you split a policy into multiple files (e.g., a base file with a default deny and one file per allowed domain).

## Chain multiple policies with `any` mode

By default, all policies in the list must allow (`mode: "all"`). Use `mode: "any"` to allow the call if at least one policy permits it:

```json
{
  "fetch": {
    "mode": "any",
    "policies": [
      {"url": "file:///etc/mcp/fetch-org.rego"},
      {"url": "http://opa.internal:8181", "policy_path": "mcp/fetch"}
    ]
  }
}
```

Mix local and remote evaluators in the same chain. For example, use a fast local allowlist as the first entry and a remote OPA as the authoritative fallback.

## See also

- [Concepts: Security policies](../concepts/policies.md)
- [Reference: Security policies](../reference/policies.md)
- [How-to: Network access with fetch](../how-to/fetch.md)
- [How-to: Filesystem access](../how-to/filesystem.md)
- [How-to: Subprocess execution](../how-to/subprocess.md)
- [How-to: ES module imports](../how-to/module-imports.md)
- [How-to: Calling upstream MCP servers](../how-to/mcp-client.md)
