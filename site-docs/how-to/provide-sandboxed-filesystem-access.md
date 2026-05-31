# Provide Sandboxed Filesystem Access

Expose the `fs` module to agent code, but restrict it to a known directory with
policy.

This example allows read, write, append, list, stat, create, remove, rename,
copy, and exists checks only under `/tmp/allowed/`.

## 1. Write the filesystem policy

Create a local Rego policy:

```bash
cat > fs-policy.rego <<'EOF'
package mcp.filesystem

default allow = false

allow if {
  input.operation in [
    "readFile",
    "writeFile",
    "appendFile",
    "readdir",
    "stat",
    "exists",
    "mkdir",
    "rm"
  ]
  startswith(input.path, "/tmp/allowed/")
}

allow if {
  input.operation in ["rename", "copyFile"]
  startswith(input.path, "/tmp/allowed/")
  startswith(input.destination, "/tmp/allowed/")
}
EOF
```

This follows the runtime's filesystem policy path:
`data.mcp.filesystem.allow`.

## 2. Point `--policies-json` at that policy

```bash
FS_POLICY_PATH="$(pwd)/fs-policy.rego"

cat > policies.json <<EOF
{
  "filesystem": {
    "policies": [
      {
        "url": "file://${FS_POLICY_PATH}",
        "rule": "data.mcp.filesystem.allow"
      }
    ]
  }
}
EOF
```

When `filesystem` is configured, `mcp-v8` injects the `fs` module into the
JavaScript runtime and checks each operation against the policy before
execution.

## 3. Start the server

Use HTTP mode for a quick end-to-end test:

```bash
mcp-v8 \
  --stateless \
  --http-port 3000 \
  --policies-json ./policies.json
```

The same `--policies-json` file also works when `mcp-v8` is used as an MCP
server or driven through `mcp-v8-cli`.

## 4. Run a permitted filesystem session

Write a small script that stays inside the allowed directory:

```bash
cat > fs-demo.js <<'EOF'
await fs.mkdir("/tmp/allowed/demo", { recursive: true });
await fs.writeFile("/tmp/allowed/demo/note.txt", "hello from mcp-v8");
await fs.appendFile("/tmp/allowed/demo/note.txt", "\nsecond line");

const text = await fs.readFile("/tmp/allowed/demo/note.txt");
const entries = await fs.readdir("/tmp/allowed/demo");
const stats = await fs.stat("/tmp/allowed/demo/note.txt");
const exists = await fs.exists("/tmp/allowed/demo/note.txt");

let denied;
try {
  await fs.writeFile("/tmp/blocked.txt", "should fail");
} catch (e) {
  denied = e.message;
}

console.log(JSON.stringify({
  text,
  entries,
  size: stats.size,
  exists,
  denied
}));
EOF
```

Submit it:

```bash
EXECUTION_ID="$(
  jq -Rs '{code: .}' < fs-demo.js \
    | curl -s http://localhost:3000/api/exec \
        -H 'content-type: application/json' \
        -d @- \
    | jq -r '.execution_id'
)"
```

Wait for completion and print the output:

```bash
while true; do
  STATUS="$(curl -s "http://localhost:3000/api/executions/${EXECUTION_ID}" | jq -r '.status')"
  if [ "${STATUS}" = "completed" ] || [ "${STATUS}" = "Completed" ]; then
    break
  fi
  sleep 1
done

curl -s "http://localhost:3000/api/executions/${EXECUTION_ID}/output" | jq -r '.data'
```

Expected shape:

```json
{"text":"hello from mcp-v8\nsecond line","entries":["note.txt"],"size":29,"exists":true,"denied":"fs.writeFile denied by policy"}
```

The exact size may differ if you change the file contents, but the denied case
should still include `denied by policy`.

## What this setup gives you

- agent code can use familiar `fs.*` calls
- policy decides which paths and operations are allowed
- rename and copy can require both source and destination to stay in the
  sandbox
- the same policy file works across HTTP, CLI, and MCP entry points

For the capability model behind this setup, see
[Filesystem Access](../concepts/filesystem-access.md). For the JSON config
shape, see [Policy Files](../reference/policy-files.md).
