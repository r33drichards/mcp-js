# How to Import npm, JSR, and URL Modules

Load external packages in your JavaScript code at runtime.

## Enable external modules

External module imports are disabled by default. Enable them with:

```bash
mcp-v8 --allow-external-modules
```

## Import npm packages

Use the `npm:` prefix. Packages are fetched from esm.sh at runtime -- no installation needed.

```js
import { camelCase } from "npm:lodash-es@4.17.21";
console.log(camelCase("hello world"));
```

Always pin versions for reproducible results.

## Import JSR packages

Use the `jsr:` prefix:

```js
import { camelCase } from "jsr:@luca/cases@1.0.0";
console.log(camelCase("hello world"));
```

## Import from URLs

Import directly from any URL serving ES modules:

```js
import { pascalCase } from "https://deno.land/x/case/mod.ts";
console.log(pascalCase("hello world"));
```

## Dynamic imports

Dynamic `import()` is supported with top-level `await`:

```js
const { default: dayjs } = await import("npm:dayjs@1.11.10");
console.log(dayjs().format("YYYY-MM-DD"));
```

## Gate modules with OPA policies

Use `--policies-json` with a `modules` section to control which modules can be imported:

```bash
mcp-v8 --allow-external-modules --policies-json '{
  "modules": {
    "policies": [{"url": "file:///etc/mcp-v8/modules.rego"}]
  }
}'
```

Example `modules.rego`:

```rego
package mcp.modules

default allow = false

allow {
    startswith(input.specifier, "npm:lodash")
}

allow {
    startswith(input.specifier, "jsr:@luca/")
}
```

The policy input includes the `specifier` field (the full import string).

See [Policy System](policy-system.md) for more on writing policies.
