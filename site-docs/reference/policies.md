# Security policies (OPA/Rego)

Complete reference for the `--policies-json` flag, the policy source JSON schema, and the exact input documents passed to Rego evaluators for each capability category.

## Flag

```
--policies-json <VALUE>
```

`<VALUE>` is either:

- An inline JSON object (the value must start with `{`).
- A path to a file whose contents are a JSON object.

The JSON is deserialized into `PoliciesConfig`. Invalid JSON or an unsupported URL scheme is a fatal startup error.

## Top-level JSON schema

```json
{
  "fetch":      <OperationPolicies | null>,
  "modules":    <OperationPolicies | null>,
  "filesystem": <OperationPolicies | null>,
  "mcp_tools":  <OperationPolicies | null>,
  "subprocess": <OperationPolicies | null>
}
```

All five keys are optional. Omitting a key means the corresponding capability is not available.

## `OperationPolicies` schema

```json
{
  "mode":     "all" | "any",
  "policies": [ <PolicySource>, ... ]
}
```

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `mode` | string | no | `"all"` | `"all"`: every evaluator must allow. `"any"`: at least one must allow. |
| `policies` | array | yes | — | Ordered list of policy sources. At least one entry is expected. |

An empty `policies` array produces a chain that always allows.

## `PolicySource` schema

```json
{
  "url":         "<string>",
  "policy_path": "<string | null>",
  "rule":        "<string | null>"
}
```

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `url` | string | **yes** | — | Scheme determines the evaluator type. See below. |
| `policy_path` | string | no | category default | Remote OPA only: path under `/v1/data/` (e.g., `"mcp/fetch"`). |
| `rule` | string | no | category default | Local regorus only: fully-qualified Rego rule to evaluate (e.g., `"data.mcp.fetch.allow"`). |

### URL schemes

| Prefix | Evaluator | Notes |
|---|---|---|
| `http://` or `https://` | Remote OPA | Posts to `{url}/v1/data/{policy_path}`, reads `result.allow`. Timeout: 5 seconds. |
| `file://` (file) | Local regorus | Loads the `.rego` file into regorus. |
| `file://` (directory) | Local regorus | Loads all `.rego` files in the directory (sorted alphabetically). Non-`.rego` files are ignored. Empty directory is a startup error. |

Any other scheme is a fatal startup error.

## Category defaults

| Category | Remote default `policy_path` | Local default `rule` |
|---|---|---|
| `fetch` | `mcp/fetch` | `data.mcp.fetch.allow` |
| `modules` | `mcp/modules` | `data.mcp.modules.allow` |
| `filesystem` | `mcp/filesystem` | `data.mcp.filesystem.allow` |
| `mcp_tools` | `mcp/tools` | `data.mcp.tools.allow` |
| `subprocess` | `mcp/subprocess` | `data.mcp.subprocess.allow` |

## Input documents

### `fetch`

Passed to the policy on every `fetch()` call from JS.

| Field | Type | Description |
|---|---|---|
| `operation` | string | Always `"fetch"`. |
| `url` | string | The raw URL string. |
| `method` | string | HTTP method in uppercase (e.g., `"GET"`, `"POST"`). |
| `headers` | object | Request headers as `{name: value}` pairs, including any injected by `--fetch-header` rules. |
| `url_parsed.scheme` | string | URL scheme (e.g., `"https"`). |
| `url_parsed.host` | string | Hostname without port (e.g., `"api.example.com"`). |
| `url_parsed.port` | number\|null | Explicit port number, or `null` if not present in the URL. |
| `url_parsed.path` | string | URL path (e.g., `"/v1/data"`). |
| `url_parsed.query` | string | Query string without `?`, or `""` if none. |

Example input:

```json
{
  "operation": "fetch",
  "url": "https://api.example.com/v1/data?limit=10",
  "method": "GET",
  "headers": {"Accept": "application/json"},
  "url_parsed": {
    "scheme": "https",
    "host": "api.example.com",
    "port": null,
    "path": "/v1/data",
    "query": "limit=10"
  }
}
```

Example Rego:

```rego
package mcp.fetch

default allow = false

allowed_hosts := {"api.example.com", "cdn.example.com"}

allow if {
    allowed_hosts[input.url_parsed.host]
    input.method == "GET"
}
```

### `filesystem`

Passed to the policy on every `fs.*` operation.

| Field | Type | Description |
|---|---|---|
| `operation` | string | Operation name. See table below. |
| `path` | string | Absolute path of the file or directory being operated on. |
| `destination` | string\|omitted | Present for `rename` and `copyFile`; the target path. Omitted for all other operations. |
| `recursive` | boolean\|omitted | Present for `mkdir`; whether `{recursive: true}` was requested. |
| `encoding` | string\|omitted | Present for text reads/writes; e.g., `"utf8"`. |
| `mcp_headers` | object\|omitted | `X-MCP-*` headers from session initialization, keyed by lowercased name with `x-mcp-` prefix stripped. Omitted when no headers were captured. |

Operation names:

| `operation` value | JS API |
|---|---|
| `readFile` | `fs.readFile()` (text or buffer) |
| `writeFile` | `fs.writeFile()` |
| `appendFile` | `fs.appendFile()` |
| `mkdir` | `fs.mkdir()` |
| `rm` | `fs.rm()` |
| `rename` | `fs.rename()` |
| `copyFile` | `fs.copyFile()` |
| `stat` | `fs.stat()` |
| `exists` | `fs.exists()` |
| `readdir` | `fs.readdir()` |

Example Rego:

```rego
package mcp.filesystem

default allow = false

# Operations with no destination (readFile, writeFile, mkdir, etc.)
allow if {
    startswith(input.path, "/data/workspace/")
    not input.destination
}

# Operations with a destination (rename, copyFile): check both paths.
allow if {
    startswith(input.path, "/data/workspace/")
    startswith(input.destination, "/data/workspace/")
}
```

### `subprocess`

Passed to the policy on every `Deno.Command` or `child_process.exec` call.

| Field | Type | Description |
|---|---|---|
| `operation` | string | `"command_output"` for `Deno.Command.output()`; `"exec"` for `child_process.exec()`. |
| `command` | string | The executable name or path (e.g., `"echo"`, `"/bin/sh"`). |
| `args` | array of strings | Arguments. For `exec`, `args[0]` is `"-c"` and `args[1]` is the shell command string. |
| `cwd` | string\|omitted | Working directory, or omitted if not specified. |
| `env` | object\|omitted | Environment variable overrides as `{KEY: VALUE}`, or omitted if not specified. |

Example Rego:

```rego
package mcp.subprocess

default allow = false

allowed_commands := {"echo", "cat"}

allow if {
    input.operation == "command_output"
    allowed_commands[input.command]
}

allow if {
    input.operation == "exec"
    some pattern in allowed_commands
    startswith(input.args[1], pattern)
}
```

### `modules`

Passed to the policy on every ES module import when `--allow-external-modules` is active and a `modules` policy is configured.

| Field | Type | Description |
|---|---|---|
| `specifier` | string | The raw module specifier as resolved to a URL. |
| `specifier_type` | string | `"npm"` for esm.sh-hosted npm packages; `"jsr"` for esm.sh-hosted JSR packages; `"url"` for all other URL imports. |
| `resolved_url` | string | The resolved module URL (currently identical to `specifier`). |
| `url_parsed.scheme` | string | URL scheme (e.g., `"https"`). |
| `url_parsed.host` | string | Hostname (e.g., `"esm.sh"`). |
| `url_parsed.path` | string | URL path (e.g., `"/lodash-es"`). |

`specifier_type` is determined by the resolved URL: `"npm"` if the URL contains `esm.sh/` (but not `esm.sh/jsr/`); `"jsr"` if it contains `esm.sh/jsr/`; `"url"` otherwise.

Example Rego:

```rego
package mcp.modules

default allow = false

allowed_npm := {"lodash-es", "uuid", "zod"}

allow if {
    input.specifier_type == "npm"
    input.url_parsed.host == "esm.sh"
    some pkg in allowed_npm
    startswith(input.url_parsed.path, sprintf("/%s", [pkg]))
}

allow if {
    input.specifier_type == "url"
    input.url_parsed.host == "cdn.jsdelivr.net"
}
```

### `mcp_tools`

Passed to the policy on every `mcp.callTool()` call.

| Field | Type | Description |
|---|---|---|
| `operation` | string | Always `"mcp_call_tool"`. |
| `server` | string | The upstream MCP server name as given to `--mcp-server`. |
| `tool` | string | The tool name being called. |
| `arguments` | object\|null | Tool call arguments as a JSON object, or `null` if none were provided. |

Example Rego:

```rego
package mcp.tools

default allow = false

allow if {
    input.server == "math"
    input.tool == "add"
}

allow if {
    input.server == "utils"
}
```

## Complete example configuration

```json
{
  "fetch": {
    "mode": "all",
    "policies": [
      {
        "url": "http://opa.internal:8181",
        "policy_path": "mcp/fetch"
      },
      {
        "url": "file:///etc/mcp/fetch-local.rego"
      }
    ]
  },
  "filesystem": {
    "policies": [
      {
        "url": "file:///etc/mcp/filesystem.rego",
        "rule": "data.mcp.filesystem.allow"
      }
    ]
  },
  "subprocess": {
    "policies": [
      {"url": "file:///etc/mcp/subprocess.rego"}
    ]
  },
  "modules": {
    "policies": [
      {"url": "file:///etc/mcp/modules/"}
    ]
  },
  "mcp_tools": {
    "mode": "any",
    "policies": [
      {"url": "file:///etc/mcp/mcp_tools.rego"}
    ]
  }
}
```

## See also

- [Quick-start: Security policies](../tutorials/policies.md)
- [How-to: Security policies](../how-to/policies.md)
- [Concepts: Security policies](../concepts/policies.md)
- [Reference: Network access with fetch](../reference/fetch.md)
- [Reference: Filesystem access](../reference/filesystem.md)
- [Reference: Subprocess execution](../reference/subprocess.md)
- [Reference: ES module imports](../reference/module-imports.md)
- [Reference: Calling upstream MCP servers](../reference/mcp-client.md)
