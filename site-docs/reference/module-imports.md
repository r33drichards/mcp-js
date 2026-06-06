# ES module imports

Complete reference for the `--allow-external-modules` flag, supported import specifier forms, and the `modules` policy.

## Server flag

### `--allow-external-modules`

| Property | Value |
|----------|-------|
| Type | boolean flag (no value argument) |
| Default | `false` |
| Source | `server/src/cli.rs` |

Enables resolution and fetching of external ES modules. When absent (the default) any `import` specifier that resolves to an external URL — `npm:`, `jsr:`, `https://`, or `http://` — is rejected at the resolve step with an error of the form:

```
External module imports are disabled. Cannot import npm package 'npm:uuid'.
Start the server with --allow-external-modules to enable.
```

No network traffic is attempted. Relative specifiers (`./foo.js`) are not affected by this flag.

## Supported import specifier forms

| Specifier form | `allow_external` gate | Resolved URL |
|---|---|---|
| `npm:<package>[@version][/path]` | yes | `https://esm.sh/<package>[@version][/path]` |
| `jsr:<scope>/<package>[@version][/path]` | yes | `https://esm.sh/jsr/<scope>/<package>[@version][/path]` |
| `https://<host>/…` | yes | used as-is |
| `http://<host>/…` | yes | used as-is |
| `./relative` or `../relative` | no | resolved against referrer URL |

Only `https` and `http` schemes are loadable. Attempts to load from any other scheme (e.g. `file://`) fail with:

```
Cannot load module '<url>': only https/http modules are supported
```

## TypeScript modules

If the final response URL (after redirects) has a path ending in `.ts` or `.tsx`, the response body is transpiled with the same swc-based transpiler used for inline code before being evaluated as JavaScript.

## Fetch timeouts

| Parameter | Value |
|-----------|-------|
| Connect timeout | 10 seconds |
| Total request timeout | 30 seconds |

These values are compile-time constants (`MODULE_FETCH_CONNECT_TIMEOUT`, `MODULE_FETCH_TIMEOUT` in `engine/module_loader.rs`).

## Modules policy

### Entrypoint

```
data.mcp.modules.allow
```

### Configuration

Supplied via `--policies-json` under the `modules` key:

```json
{
  "modules": {
    "policies": [
      { "url": "file:///path/to/policies/modules.rego" }
    ]
  }
}
```

Or with a remote OPA server:

```json
{
  "modules": {
    "policies": [
      { "url": "http://opa:8181", "policy_path": "mcp/modules" }
    ]
  }
}
```

Multiple entries in `policies` form a chain; all must return `true` for the import to be allowed.

### Policy input shape

The input document passed to the policy on every module load:

| Field | Type | Description |
|-------|------|-------------|
| `specifier` | string | Fully resolved URL of the module being loaded |
| `specifier_type` | string | `"npm"`, `"jsr"`, or `"url"` (see derivation below) |
| `resolved_url` | string | Same as `specifier` |
| `url_parsed.scheme` | string | URL scheme, e.g. `"https"` |
| `url_parsed.host` | string | Host of the resolved URL, e.g. `"esm.sh"` |
| `url_parsed.path` | string | Path component, e.g. `"/lodash-es"` |

#### `specifier_type` derivation

| Condition (evaluated against resolved URL string) | `specifier_type` |
|---|---|
| URL contains `"esm.sh/jsr/"` | `"jsr"` |
| URL contains `"esm.sh/"` (but not `"esm.sh/jsr/"`) | `"npm"` |
| All other HTTP/HTTPS URLs | `"url"` |

### Example policy input

```json
{
  "specifier": "https://esm.sh/lodash-es",
  "specifier_type": "npm",
  "resolved_url": "https://esm.sh/lodash-es",
  "url_parsed": {
    "scheme": "https",
    "host": "esm.sh",
    "path": "/lodash-es"
  }
}
```

### Policy evaluation timing

The policy is evaluated inside the `load` phase, after `allow_external` has been confirmed true but before the HTTP GET request is sent. A `false` or undefined result from the policy entrypoint returns the following error to the executing code:

```
Module import denied by policy: '<url>' is not allowed by the module policy
```

### Example policy

See `policies/modules.rego` in the repository for a complete allowlist example covering `npm:`, `jsr:`, URL hosts, and wildcard host suffixes.

## Interaction with `--allow-external-modules`

The `allow_external` flag and the modules policy are independent controls:

| `--allow-external-modules` | policy configured | Result |
|---|---|---|
| absent (false) | any | all external imports blocked at resolve |
| present | absent | all resolved URLs are loaded without policy check |
| present | present | URLs are loaded only if policy returns `true` |

## See also

- [Quick-start: ES module imports](../tutorials/module-imports.md)
- [How-to: ES module imports](../how-to/module-imports.md)
- [Concepts: ES module imports](../concepts/module-imports.md)
- [Reference: CLI flags](../reference/cli-flags.md)
- [Reference: Security policies](../reference/policies.md)
- [Reference: WebAssembly modules](../reference/wasm-modules.md)
