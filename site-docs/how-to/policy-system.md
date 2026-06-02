# How to Write and Configure OPA Policies

Gate fetch, filesystem, module, subprocess, and MCP tool operations with Open Policy Agent (OPA) Rego policies.

## Policy configuration overview

Pass policies via `--policies-json` as inline JSON or a path to a JSON file:

```bash
# Inline
mcp-v8 --policies-json '{"fetch": {"policies": [{"url": "file:///etc/mcp-v8/fetch.rego"}]}}'

# File path
mcp-v8 --policies-json /etc/mcp-v8/policies.json
```

## Configuration schema

```json
{
  "fetch": {
    "mode": "all",
    "policies": [
      { "url": "file:///path/to/fetch.rego" },
      { "url": "http://opa-server:8181", "policy_path": "mcp/fetch" }
    ]
  },
  "filesystem": {
    "policies": [{ "url": "file:///path/to/fs.rego" }]
  },
  "modules": {
    "policies": [{ "url": "file:///path/to/modules.rego" }]
  },
  "subprocess": {
    "policies": [{ "url": "file:///path/to/subprocess.rego" }]
  },
  "mcp_tools": {
    "policies": [{ "url": "file:///path/to/mcp_tools.rego" }]
  }
}
```

Each section is optional. Only sections you include are enforced.

## Evaluation modes

- `"all"` (default) -- all policies in the chain must allow (AND logic)
- `"any"` -- any single policy allowing is sufficient (OR logic)

## Policy source types

**Local Rego files** -- evaluated in-process via the regorus engine:

```json
{ "url": "file:///etc/mcp-v8/fetch.rego" }
```

Point to a directory to load all `.rego` files in it:

```json
{ "url": "file:///etc/mcp-v8/policies/" }
```

**Remote OPA server** -- queries OPA's REST API:

```json
{
  "url": "http://opa-server:8181",
  "policy_path": "mcp/fetch"
}
```

## Write a fetch policy

The policy input for fetch includes `url`, `method`, and `headers`:

```rego
package mcp.fetch

default allow = false

allow {
    startswith(input.url, "https://api.example.com/")
    input.method == "GET"
}
```

## Write a filesystem policy

The policy input includes `operation`, `path`, `destination`, `recursive`, and `encoding`:

```rego
package mcp.filesystem

default allow = false

allow {
    input.operation == "readFile"
    startswith(input.path, "/data/")
}

allow {
    input.operation == "writeFile"
    startswith(input.path, "/tmp/")
}
```

## Write a modules policy

The policy input includes `specifier` (the import string):

```rego
package mcp.modules

default allow = false

allow {
    startswith(input.specifier, "npm:lodash")
}
```

## Write a subprocess policy

The policy input includes `command`, `args`, `cwd`, and `env`:

```rego
package mcp.subprocess

default allow = false

allow {
    input.command == "echo"
}

allow {
    input.command == "ls"
    input.cwd == "/tmp"
}
```

## Write an MCP tools policy

The policy input includes details about the MCP tool being called:

```rego
package mcp.tools

default allow = false

allow {
    input.server == "my-server"
    input.tool == "safe-tool"
}
```

## Override default rule paths

For local policies, you can override the eval rule:

```json
{
  "url": "file:///path/to/policy.rego",
  "rule": "data.custom.package.allow"
}
```

For remote policies, override the API path:

```json
{
  "url": "http://opa:8181",
  "policy_path": "custom/path"
}
```

Default rule paths per operation:

| Operation | Rule | API path |
|-----------|------|----------|
| fetch | `data.mcp.fetch.allow` | `mcp/fetch` |
| filesystem | `data.mcp.filesystem.allow` | `mcp/filesystem` |
| modules | `data.mcp.modules.allow` | `mcp/modules` |
| subprocess | `data.mcp.subprocess.allow` | `mcp/subprocess` |
| mcp_tools | `data.mcp.tools.allow` | `mcp/tools` |
