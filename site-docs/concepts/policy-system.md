# OPA Policy Architecture

mcp-v8 uses Open Policy Agent (OPA) policies to control what JavaScript code is allowed to do. Every external operation -- network requests, filesystem access, module imports, MCP tool calls, and subprocess execution -- can be gated by a policy that decides whether to allow or deny the operation. This architecture gives operators fine-grained control over the sandbox's capabilities without modifying the server code.

## Two Backends

The policy system supports two evaluation backends:

### Remote OPA Server

When a policy URL starts with `http://` or `https://`, mcp-v8 queries a remote OPA server via its REST API. The request is `POST {base_url}/v1/data/{policy_path}` with the input as the JSON body. The response is expected to contain `{ "result": { "allow": true/false } }`.

Remote OPA is appropriate when policies are managed centrally, need to be updated without restarting mcp-v8, or when the same policies are shared across multiple services.

### Local Regorus Engine

When a policy URL starts with `file://`, mcp-v8 evaluates the Rego policy locally using the `regorus` crate, a Rust-native OPA implementation. The URL can point to a single `.rego` file or a directory of `.rego` files (non-recursive, sorted alphabetically).

Local evaluation is fast (no network round-trip), works offline, and keeps policy files alongside the server configuration. It is the simpler choice for most deployments.

## PolicyChain with All/Any Modes

Multiple policy evaluators can be composed into a `PolicyChain` with one of two evaluation modes:

### All mode (default)

Every evaluator in the chain must return `allow = true` for the operation to proceed. If any evaluator denies, the operation is denied. This is AND logic: all policies must agree.

Use All mode when you want layered security -- for example, a base policy that allows only HTTPS URLs combined with a team-specific policy that further restricts to certain domains.

### Any mode

The operation is allowed if at least one evaluator returns `allow = true`. This is OR logic: any policy can grant permission.

Use Any mode when you have alternative authorization paths -- for example, allowing an operation if either a team policy approves it or an admin override policy approves it.

An empty chain (no evaluators) is permissive: it allows all operations. This is the default when no policies are configured.

## Five Policy Domains

The policy system covers five operation domains:

### 1. Fetch (network requests)

Controls `fetch()` calls from JavaScript. The policy input includes:

- `operation` -- Always `"fetch"`
- `url` -- The full URL being requested
- `method` -- HTTP method (GET, POST, etc.)
- `headers` -- Request headers (after header injection rules are applied)
- `url_parsed` -- Decomposed URL: scheme, host, port, path, query

### 2. Filesystem

Controls `fs.*` operations. The policy input includes:

- `operation` -- The fs method name (readFile, writeFile, mkdir, rm, etc.)
- `path` -- The target file/directory path
- `destination` -- For rename/copy operations, the target path
- `recursive` -- Whether the operation is recursive
- `encoding` -- The requested encoding (utf8, buffer)
- `mcp_headers` -- MCP session headers, enabling per-session access control

### 3. Modules

Controls ES module imports. The policy input includes:

- `specifier` -- The original import specifier (e.g., `"npm:lodash-es@4.17.21"`)
- `specifier_type` -- The type: npm, jsr, url, or relative
- `resolved_url` -- The URL that will be fetched
- `url_parsed` -- Decomposed resolved URL

### 4. Subprocess

Controls subprocess execution. The policy input includes:

- `operation` -- `"command_output"` (Deno.Command) or `"exec"` (child_process.exec)
- `command` -- The command being executed
- `args` -- Command arguments

### 5. MCP Tools

Controls `mcp.callTool()` calls to upstream MCP servers. The policy input includes:

- `operation` -- Always `"mcp_call_tool"`
- `server` -- The upstream server name
- `tool` -- The tool being called
- `arguments` -- The tool call arguments

## The --policies-json Format

Policies are configured through a single JSON document (passed inline or as a file path via `--policies-json`). The structure maps domain names to policy configurations:

```json
{
  "fetch": {
    "mode": "all",
    "policies": [
      {"url": "http://opa:8181", "policy_path": "mcp/fetch"},
      {"url": "file:///etc/policies/fetch.rego"}
    ]
  },
  "filesystem": {
    "mode": "all",
    "policies": [
      {"url": "file:///etc/policies/fs.rego", "rule": "data.mcp.filesystem.allow"}
    ]
  },
  "modules": {
    "mode": "any",
    "policies": [
      {"url": "file:///etc/policies/modules/"}
    ]
  },
  "mcp_tools": {
    "mode": "all",
    "policies": [
      {"url": "file:///etc/policies/mcp_tools.rego"}
    ]
  },
  "subprocess": {
    "mode": "all",
    "policies": [
      {"url": "file:///etc/policies/subprocess.rego"}
    ]
  }
}
```

Each policy source specifies a URL and optional overrides:

- `policy_path` -- (Remote only) The OPA REST API data path. Defaults are per-domain (e.g., `mcp/fetch` for fetch policies).
- `rule` -- (Local only) The Rego evaluation rule. Defaults are per-domain (e.g., `data.mcp.fetch.allow`).

Domains that are not present in the JSON are unrestricted. A domain that is present but has an empty `policies` array is also unrestricted (empty chain allows all).

## Design Rationale

The policy system is designed around the principle that mcp-v8 should be secure by default and progressively enabled. Without any policy configuration:

- `fetch()` is not available (no fetch config means no HTTP access)
- `fs` is not available (no fs config means no filesystem access)
- External modules are disabled (requires `--allow-external-modules`)
- Subprocess execution is not available (no subprocess config)
- MCP tool calls follow whatever policies are configured

Each capability is explicitly opted into through CLI flags and policy configuration. OPA policies then provide the fine-grained control over what is permitted within each enabled capability.
