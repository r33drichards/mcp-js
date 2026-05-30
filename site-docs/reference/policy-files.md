# Policy Files

`--policies-json` accepts either inline JSON or a path to a JSON file. The CLI
help documents the first-pass shape as:

```json
{
  "fetch": {
    "mode": "all",
    "policies": [
      {
        "url": "file:///abs/path/fetch.rego",
        "policy_path": "mcp/fetch",
        "rule": "data.mcp.fetch.allow"
      }
    ]
  },
  "modules": {
    "mode": "all",
    "policies": []
  }
}
```

Useful keys:

- top-level operation keys: `fetch`, `modules`, `filesystem`,
  `mcp_tools`, and `subprocess`
- per-operation keys: `mode` and `policies`
- `mode`: `"all"` or `"any"`; omitted means `"all"`
- `policies`: ordered list of policy sources
- per-policy keys:
  - `url`: required; `http://` and `https://` point at remote OPA, `file://`
    points at a local `.rego` file or directory
  - `policy_path`: optional remote OPA data path
  - `rule`: optional local Regorus rule path

For local fetch policies, omitting `rule` uses the default
`data.mcp.fetch.allow`.

Minimal local Rego from `server/tests/local_opa_policy.rs`:

```rego
package mcp.fetch
default allow = false

allow if {
    input.method == "GET"
}
```

That test is wired with a policy entry like
`{"url":"file:///.../fetch.rego","policy_path":null,"rule":null}`, which is
enough for fetch gating because the default fetch rule path is applied.

For the behavior model behind these configuration keys, see
[Policy System](../concepts/policy-system.md),
[Network Access](../concepts/network-access.md), and
[Filesystem Access](../concepts/filesystem-access.md).
