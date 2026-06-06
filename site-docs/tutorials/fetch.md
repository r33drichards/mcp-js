# Network access with fetch

We'll enable outbound HTTP from JavaScript in a few minutes by creating a
permissive fetch policy, starting the server, and running a `fetch()` call.

## Prerequisites

- `mcp-v8` installed (`nix run github:r33drichards/mcp-js`, the install
  script, or a prebuilt release).
- `curl` and `jq` available on the PATH.

## 1. Write a fetch policy

`fetch()` is disabled until a fetch policy exists. Create
`/tmp/fetch-allow-all.rego`:

```rego
package mcp.fetch

default allow = true
```

This allows every outbound request. See
[How-to: fetch](../how-to/fetch.md) for domain-restricted examples, and
[Concepts: Security policies](../concepts/policies.md) for how policies work.

## 2. Create a policies configuration file

Create `/tmp/policies.json` that references the Rego file:

```json
{
  "fetch": {
    "policies": [
      { "url": "file:///tmp/fetch-allow-all.rego" }
    ]
  }
}
```

## 3. Start the server

```bash
mcp-v8 --stateless --http-port=8080 --policies-json=/tmp/policies.json
```

The server logs to stderr. Leave it running in one terminal.

## 4. Run JavaScript that calls fetch()

In a second terminal, submit a JS execution:

```bash
EX=$(curl -s -X POST http://localhost:8080/api/exec \
  -H "Content-Type: application/json" \
  -d '{
    "code": "const r = await fetch(\"https://httpbin.org/get\"); const d = await r.json(); console.log(d.url);"
  }' | jq -r .execution_id)
echo "execution: $EX"
```

Poll until the execution reaches a terminal status:

```bash
curl -s "http://localhost:8080/api/executions/$EX" | jq '{status, output}'
```

Expected output when the execution completes:

```json
{
  "status": "completed",
  "output": "https://httpbin.org/get\n"
}
```

The `console.log` output from JS appears in the `output` field.

## 5. Try a JSON response

```bash
EX=$(curl -s -X POST http://localhost:8080/api/exec \
  -H "Content-Type: application/json" \
  -d '{
    "code": "const r = await fetch(\"https://httpbin.org/json\"); const d = await r.json(); console.log(d.slideshow.title);"
  }' | jq -r .execution_id)
curl -s "http://localhost:8080/api/executions/$EX" | jq .output
```

Expected:

```
"Sample Slide Show\n"
```

## What happened

When `run_js` executes code that calls `fetch()`, the server:

1. Applies any configured header injection rules (none here).
2. Evaluates the fetch policy with the request details.
3. Only if the policy returns `allow = true`, performs the actual HTTP request.
4. Returns the response to the JS runtime.

The JavaScript code never touches the policy or any injected credentials.

## Next steps

- Restrict fetch to specific domains — see
  [How-to: fetch](../how-to/fetch.md#enable-fetch-with-a-domain-allowlist).
- Add an API key header server-side — see
  [How-to: fetch](../how-to/fetch.md#inject-a-static-auth-header).
- Understand the security model — see
  [Concepts: fetch](../concepts/fetch.md).

## See also

- [How-to: Network access with fetch](../how-to/fetch.md)
- [Concepts: Network access with fetch](../concepts/fetch.md)
- [Reference: Network access with fetch](../reference/fetch.md)
- [Concepts: Security policies](../concepts/policies.md)
- [Reference: CLI flags](../reference/cli-flags.md)
