# ES module imports

Recipes for enabling external ES module imports and restricting which modules are importable.

## Enable external module imports

Pass `--allow-external-modules` when starting the server. Without this flag every `import` statement that resolves to an external URL is rejected immediately at the resolve step — no network traffic is attempted.

```bash
mcp-v8 --http-port=3000 --allow-external-modules
```

The server logs `External module imports: ENABLED` on startup.

## Use the supported specifier forms

| Form | Example | Resolves to |
|------|---------|-------------|
| `npm:<pkg>` | `import _ from "npm:lodash-es"` | `https://esm.sh/lodash-es` |
| `jsr:<scope>/<pkg>` | `import { camelCase } from "jsr:@luca/cases"` | `https://esm.sh/jsr/@luca/cases` |
| Full URL | `import { format } from "https://esm.sh/date-fns"` | used as-is |

Relative specifiers (`./utils.js`) are resolved against the referring module's URL using standard URL resolution.

## Restrict importable modules with a policy

Use a `modules` OPA/Rego policy to allowlist specific packages, hosts, or URL patterns. The policy is evaluated at load time; if it returns `false` the fetch is blocked and execution fails with a policy-denial error.

### Step 1 — Write the Rego policy

Create `policies/modules.rego` (the example in the repo is a good starting point):

```rego
package mcp.modules

default allow = false

allow if {
    input.specifier_type == "npm"
    input.url_parsed.host == "esm.sh"
    startswith(input.url_parsed.path, "/lodash-es")
}
```

### Step 2 — Load the policy inline

Pass the policy file path or inline JSON to `--policies-json`:

```bash
mcp-v8 --http-port=3000 \
  --allow-external-modules \
  --policies-json '{"modules":{"policies":[{"url":"file:///path/to/policies/modules.rego"}]}}'
```

The server logs how many policies were loaded for the `modules` chain.

### Step 3 — Verify the policy is enforced

An import not matched by the policy returns:

```
Module import denied by policy: 'https://esm.sh/axios' is not allowed by the module policy
```

## Use the docker-compose.module-policy.yml pattern

The repo ships `docker-compose.module-policy.yml`, which runs an OPA server alongside two mcp-v8 instances — one with modules disabled (the default) and one with `--allow-external-modules` and a remote OPA policy:

```bash
docker compose -f docker-compose.module-policy.yml up
```

- `mcp-default` (port 3001) — modules disabled; demonstrates the default posture.
- `mcp-opa-policy` (port 3002) — modules enabled, all imports gated by the OPA server.

The `mcp-opa-policy` service is configured with:

```
--allow-external-modules
--policies-json={"modules":{"policies":[{"url":"http://opa:8181","policy_path":"mcp/modules"}]}}
```

This points the modules policy chain at the OPA server's `mcp/modules` decision endpoint (`data.mcp.modules.allow`). The OPA server is loaded with the files in `policies/` at startup.

To test the allowlisted packages against the OPA-gated instance:

```bash
mcp-v8-cli --url http://localhost:3002 run_js \
  --code 'import { v4 } from "npm:uuid"; console.log(v4());'
```

To confirm a blocked package is denied:

```bash
mcp-v8-cli --url http://localhost:3002 run_js \
  --code 'import axios from "npm:axios"; console.log("ok");'
# => Module import denied by policy
```

## Use a remote OPA server independently

```bash
mcp-v8 --http-port=3000 \
  --allow-external-modules \
  --policies-json '{"modules":{"policies":[{"url":"http://opa:8181","policy_path":"mcp/modules"}]}}'
```

The `url` entry delegates every module-allow decision to the remote OPA server. You can combine a local Rego file with a remote OPA entry in the same `policies` array; both must allow for the import to proceed.

## See also

- [Concepts: ES module imports](../concepts/module-imports.md)
- [Reference: ES module imports](../reference/module-imports.md)
- [How-to: Security policies](../how-to/policies.md)
- [How-to: WebAssembly modules](../how-to/wasm-modules.md)
- [How-to: Running JavaScript & TypeScript](../how-to/js-execution.md)
