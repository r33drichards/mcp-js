# Policy System Reference

## --policies-json Format

The `--policies-json` flag accepts either inline JSON or a path to a JSON file.

```json
{
  "fetch": {
    "mode": "all",
    "policies": [
      {"url": "http://opa:8181", "policy_path": "mcp/fetch"},
      {"url": "file:///path/to/fetch.rego", "rule": "data.mcp.fetch.allow"}
    ]
  },
  "modules": {
    "mode": "any",
    "policies": [
      {"url": "file:///path/to/modules.rego"}
    ]
  },
  "filesystem": {
    "mode": "all",
    "policies": [
      {"url": "file:///path/to/fs.rego"}
    ]
  },
  "subprocess": {
    "mode": "all",
    "policies": [
      {"url": "file:///path/to/subprocess.rego"}
    ]
  },
  "mcp_tools": {
    "mode": "all",
    "policies": [
      {"url": "file:///path/to/mcp_tools.rego"}
    ]
  }
}
```

## Top-Level Sections

| Section | Controls | JS API Gated |
|---------|----------|-------------|
| `fetch` | `fetch()` calls | `fetch(url, opts)` |
| `modules` | Module import auditing | `import` declarations |
| `filesystem` | Filesystem operations | `fs.*` methods |
| `subprocess` | Subprocess execution | `Deno.Command`, `child_process.exec` |
| `mcp_tools` | MCP tool calls from JS | `mcp.callTool()` |

All sections are optional. Omitting a section disables that feature entirely (the corresponding JS global is not injected), except for `modules` which only audits when present (module loading is controlled by `--allow-external-modules`).

## OperationPolicies Schema

Each section has the same schema:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `string` | `"all"` | Evaluation mode: `"all"` or `"any"` |
| `policies` | `array` | (required) | Ordered list of policy sources |

## PolicySource Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `url` | `string` | Yes | Policy source URL |
| `policy_path` | `string?` | No | (Remote only) OPA REST API data path |
| `rule` | `string?` | No | (Local only) Regorus eval rule |

## Policy URL Formats

| Scheme | Backend | Description |
|--------|---------|-------------|
| `http://...` | Remote OPA | Queries an OPA REST API server |
| `https://...` | Remote OPA | Queries an OPA REST API server (TLS) |
| `file:///path/to/file.rego` | Local Rego file | Evaluates a single `.rego` file via regorus |
| `file:///path/to/directory/` | Local Rego directory | Loads all `.rego` files in the directory (sorted alphabetically) |

Any other scheme is rejected with an error.

## PolicyChain Modes

| Mode | Value | Logic | Description |
|------|-------|-------|-------------|
| All | `"all"` | AND | All evaluators must return `true`. Default. |
| Any | `"any"` | OR | Any evaluator returning `true` is sufficient. |

An empty policy chain (no evaluators) is permissive and returns `true`.

## Default Policy Paths and Rules

| Section | Default Remote `policy_path` | Default Local `rule` |
|---------|------------------------------|---------------------|
| `fetch` | `mcp/fetch` | `data.mcp.fetch.allow` |
| `modules` | `mcp/modules` | `data.mcp.modules.allow` |
| `filesystem` | `mcp/filesystem` | `data.mcp.filesystem.allow` |
| `subprocess` | `mcp/subprocess` | `data.mcp.subprocess.allow` |
| `mcp_tools` | `mcp/tools` | `data.mcp.tools.allow` |

## Remote OPA Evaluation

For remote policies, the server sends a `POST` request to:

```
{base_url}/v1/data/{policy_path}
```

Request body:

```json
{
  "input": { /* policy input object */ }
}
```

Expected response:

```json
{
  "result": {
    "allow": true
  }
}
```

The policy is considered to allow the operation if `result.allow` is `true`. A missing `result`, missing `allow`, or `allow: false` all result in denial.

Timeout: 5 seconds per OPA request.

## Local Rego Evaluation

For local policies, the server uses the `regorus` Rego engine:

1. Loads the `.rego` file(s) at startup
2. Sets the policy input for each evaluation
3. Evaluates the specified rule (e.g., `data.mcp.fetch.allow`)
4. The result must equal `true` for the operation to be allowed

Evaluation is synchronous and CPU-bound. The regorus engine is protected by a `Mutex`.
