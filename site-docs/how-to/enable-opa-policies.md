# Enable OPA Policies

Create a minimal local fetch policy:

```bash
cat > fetch.rego <<'EOF'
package mcp.fetch

default allow = false

allow if {
  input.method == "GET"
}
EOF
```

Point `--policies-json` at a JSON config file:

```bash
POLICY_PATH="$(pwd)/fetch.rego"
cat > policies.json <<EOF
{
  "fetch": {
    "policies": [
      {
        "url": "file://${POLICY_PATH}",
        "rule": "data.mcp.fetch.allow"
      }
    ]
  }
}
EOF
```

Start the server with that policy configuration:

```bash
mcp-v8 \
  --stateless \
  --http-port 3000 \
  --policies-json ./policies.json
```

`--policies-json` enables the policy chain used for fetch, module import
auditing, and filesystem access depending on the configuration you provide. For
local fetch policies, the default rule path is `data.mcp.fetch.allow`; this
example sets it explicitly.
