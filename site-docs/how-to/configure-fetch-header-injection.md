# Configure Fetch Header Injection

Create a minimal fetch policy first, because header injection requires
`fetch()` to be enabled:

```bash
cat > fetch.rego <<'EOF'
package mcp.fetch

default allow = false

allow if {
  input.method == "GET"
}
EOF
```

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

Add a static header rule on the command line:

```bash
mcp-v8 \
  --stateless \
  --http-port 3000 \
  --policies-json ./policies.json \
  --fetch-header 'host=api.github.com,header=Authorization,value=Bearer TOKEN,methods=GET'
```

Or load rules from a JSON file:

```bash
cat > fetch-headers.json <<'EOF'
[
  {
    "host": "api.github.com",
    "methods": ["GET"],
    "headers": {
      "Authorization": "Bearer TOKEN",
      "X-GitHub-Api-Version": "2022-11-28"
    }
  }
]
EOF
```

```bash
mcp-v8 \
  --stateless \
  --http-port 3000 \
  --policies-json ./policies.json \
  --fetch-header-config ./fetch-headers.json
```

For the runtime behavior behind these rules, see
[Network Access](../concepts/network-access.md).
