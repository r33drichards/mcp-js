# Security policies (OPA/Rego)

Every host capability in mcp-v8 — network access, filesystem operations, subprocess execution, ES module imports, and upstream MCP tool calls — is **off by default**. A capability becomes available only when a Rego policy is loaded for it. In this tutorial we'll enable `fetch`, confirm a request is allowed, then flip the policy to deny.

## Prerequisites

- mcp-v8 installed (`mcp-v8 --help` works). See [install overview](../install/overview.md).
- `curl` available in your shell.
- An HTTP endpoint to fetch. We'll use `https://httpbin.org/get` throughout.

## 1. Write a minimal allow-all fetch policy

Create `fetch.rego` in a directory of your choice:

```rego
package mcp.fetch

default allow = true
```

The package `mcp.fetch` and the rule `allow` are required by the default entrypoint. When `allow` evaluates to `true`, the request proceeds; when `false` (or undefined), it is denied.

## 2. Start the server with the policy

Pass `--policies-json` with a JSON object that references the file using a `file://` URL:

```bash
mcp-v8 --stateless --http-port 3000 \
  --policies-json "{\"fetch\":{\"policies\":[{\"url\":\"file://$(pwd)/fetch.rego\"}]}}"
```

Or save the JSON to a file and pass the path:

```bash
cat > policies.json <<'EOF'
{
  "fetch": {
    "policies": [{"url": "file:///path/to/fetch.rego"}]
  }
}
EOF
mcp-v8 --stateless --http-port 3000 --policies-json policies.json
```

Replace `/path/to/fetch.rego` with the absolute path to the file you created. The server logs `Loaded policies configuration from --policies-json` when the config parses successfully.

## 3. Submit JS code that calls fetch

In a second terminal, submit a snippet via the REST API:

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H 'Content-Type: application/json' \
  -d '{"code":"const r = await fetch(\"https://httpbin.org/get\"); console.log(r.status);"}'
```

This returns `{"execution_id":"..."}`. Poll until it completes, then read the output:

```bash
# poll status
curl -s http://localhost:3000/api/executions/<execution_id>
# read output once status is "completed"
curl -s http://localhost:3000/api/executions/<execution_id>/output
```

Expected output in `data`:

```
200
```

The fetch succeeded because the policy unconditionally allows everything.

## 4. Flip the policy to deny all traffic

Stop the server. Edit `fetch.rego`:

```rego
package mcp.fetch

default allow = false
```

Restart the server with the same command as step 2. Re-submit the same JS snippet. The execution will now report an error — the `data` field will contain a message like:

```
fetch denied by policy: GET https://httpbin.org/get is not allowed
```

The fetch is blocked without changing any JS code, only the policy.

## 5. Write a conditional policy

A realistic policy grants access to specific hosts using the parsed URL:

```rego
package mcp.fetch

default allow = false

allow if {
    input.method == "GET"
    input.url_parsed.host == "httpbin.org"
}
```

With this policy, GET requests to `httpbin.org` are allowed; all other hosts or methods are denied. The `input.url_parsed` object provides `host`, `port`, `path`, `query`, and `scheme` for fine-grained matching.

## Next steps

- See the [how-to guide](../how-to/policies.md) to gate filesystem, subprocess, modules, and MCP tool calls.
- See the [reference](../reference/policies.md) for the full JSON schema and every input field.
- See the [concepts page](../concepts/policies.md) for how policy chains and evaluation modes work.

## See also

- [Concepts: Security policies](../concepts/policies.md)
- [How-to: Security policies](../how-to/policies.md)
- [Reference: Security policies](../reference/policies.md)
- [Quick-start: Network access with fetch](../tutorials/fetch.md)
- [Quick-start: Filesystem access](../tutorials/filesystem.md)
- [Quick-start: Subprocess execution](../tutorials/subprocess.md)
- [Quick-start: ES module imports](../tutorials/module-imports.md)
- [Quick-start: Calling upstream MCP servers](../tutorials/mcp-client.md)
