# Module Loading Reference

## Specifier Formats

| Specifier Format | Resolution | Example |
|-----------------|------------|---------|
| `npm:<pkg>@<ver>` | `https://esm.sh/<pkg>@<ver>` | `npm:lodash-es@4.17.21` -> `https://esm.sh/lodash-es@4.17.21` |
| `jsr:@<scope>/<pkg>@<ver>` | `https://esm.sh/jsr/@<scope>/<pkg>@<ver>` | `jsr:@luca/cases@1.0.0` -> `https://esm.sh/jsr/@luca/cases@1.0.0` |
| `https://...` | Direct URL pass-through | `https://esm.sh/lodash-es` |
| `http://...` | Direct URL pass-through | `http://example.com/module.js` |
| `./...` or `../...` | Resolved relative to referrer | `./utils.js` |

## External Module Control

External modules (npm:, jsr:, and URL imports) are disabled by default.

| Flag | Default | Description |
|------|---------|-------------|
| `--allow-external-modules` | `false` | Enable external module imports |

When disabled, any `import` using `npm:`, `jsr:`, `https:`, or `http:` specifiers is rejected with an error at both resolution time and load time (defense-in-depth).

Relative imports (`./`, `../`) are always allowed.

## Network Fetch Configuration

| Constant | Value | Description |
|----------|-------|-------------|
| Module fetch timeout | 30 seconds | Total request timeout (DNS + connect + transfer) |
| Module connect timeout | 10 seconds | Connect-phase timeout for unreachable hosts |

## Policy Input Schema

When a module policy chain is configured via `--policies-json`, each module import is audited against the policy. The policy input has this schema:

```json
{
  "specifier": "npm:lodash-es@4.17.21",
  "specifier_type": "npm",
  "resolved_url": "https://esm.sh/lodash-es@4.17.21",
  "url_parsed": {
    "scheme": "https",
    "host": "esm.sh",
    "path": "/lodash-es@4.17.21"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `specifier` | `string` | Original specifier as written in code |
| `specifier_type` | `string` | One of: `npm`, `jsr`, `url`, `relative` |
| `resolved_url` | `string` | The URL that will be fetched |
| `url_parsed.scheme` | `string` | URL scheme (`https`, `http`) |
| `url_parsed.host` | `string` | URL host |
| `url_parsed.path` | `string` | URL path |

## Policy Configuration

Module policies are configured in the `modules` section of `--policies-json`:

```json
{
  "modules": {
    "mode": "all",
    "policies": [
      {
        "url": "file:///path/to/modules.rego",
        "rule": "data.mcp.modules.allow"
      }
    ]
  }
}
```

Default OPA REST path: `mcp/modules`
Default local Rego rule: `data.mcp.modules.allow`
