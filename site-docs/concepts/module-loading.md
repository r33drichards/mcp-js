# Module Resolution

mcp-v8 supports ES module `import` statements, allowing JavaScript code to use external packages and organize code across multiple files. Module resolution follows a set of conventions that map familiar package specifiers to network-fetchable URLs.

## Resolution Rules

The module loader recognizes four types of specifiers:

### npm: specifiers

```js
import _ from "npm:lodash-es@4.17.21";
```

The `npm:` prefix is stripped and the remainder is appended to `https://esm.sh/`. The example above resolves to `https://esm.sh/lodash-es@4.17.21`. esm.sh serves npm packages as standard ES modules, handling CommonJS-to-ESM conversion transparently.

### jsr: specifiers

```js
import { toCamelCase } from "jsr:@luca/cases@1.0.0";
```

The `jsr:` prefix maps to `https://esm.sh/jsr/`. The example resolves to `https://esm.sh/jsr/@luca/cases@1.0.0`. JSR (JavaScript Registry) packages are served through esm.sh's JSR proxy.

### URL imports

```js
import { z } from "https://esm.sh/zod@3.22.4";
```

Absolute HTTP and HTTPS URLs are used directly. The module is fetched from the specified URL.

### Relative imports

```js
import { helper } from "./utils.js";
```

Relative specifiers (`./`, `../`) are resolved against the referrer module's URL. Since user code runs under a synthetic `file:///main_N.js` URL, relative imports resolve to `file:///` URLs that the loader then tries to fetch -- which typically fails unless there is another module at that path. Relative imports are primarily useful when one dynamically loaded module imports another.

## TypeScript Auto-Stripping

When the module loader fetches a module from the network, the response is checked for TypeScript content. If the response body appears to contain TypeScript (based on content type or file extension), it is passed through the same SWC type-stripping pipeline that processes the main code. This means imported TypeScript modules work transparently.

## The --allow-external-modules Flag

By default, all external module imports (npm:, jsr:, and URL) are disabled. Attempting to import an external module produces an error:

```
External module imports are disabled. Cannot import npm package 'npm:lodash-es@4.17.21'.
Start the server with --allow-external-modules to enable.
```

This default-deny posture exists because module imports represent network access and arbitrary code execution. An agent that can import any npm package can effectively run any code. The `--allow-external-modules` flag explicitly opts into this capability.

Relative imports are always permitted because they resolve against local module URLs and do not involve network access (though they will fail if the target module does not exist).

## Optional Policy Gating

When a module policy chain is configured (via `--policies-json`), every external module import is evaluated against the policy before the module is fetched. The policy input includes:

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

The policy can allow or deny based on any combination of the original specifier, the resolved URL, the host, or the path. This enables fine-grained control: allow only specific packages, restrict to a particular registry, or deny all imports from certain hosts.

Policy evaluation happens at resolution time (before the HTTP request is made) and again at load time (after the response is received). This two-phase check ensures that redirects are also evaluated.

## Fetch Mechanics

Module fetching uses a dedicated reqwest HTTP client with:

- **30-second request timeout** -- Limits total transfer time for a single module fetch.
- **10-second connect timeout** -- Fails fast when a host is unreachable, avoiding long waits on DNS or TCP connect.

These timeouts are separate from the V8 execution timeout. A module fetch that hangs will time out independently, producing a clear error message about the failed import.

## Design Rationale

The decision to route npm and JSR packages through esm.sh rather than implementing a local package manager reflects mcp-v8's design philosophy: the server is a runtime, not a build system. esm.sh handles version resolution, CommonJS-to-ESM conversion, and dependency bundling, so mcp-v8 only needs an HTTP client. This keeps the server simple and avoids the complexity of managing `node_modules` directories or lockfiles.

The default-deny posture for external imports ensures that module access is a conscious configuration choice, not an accident. In security-sensitive deployments, the policy system provides additional control over exactly which modules are permitted.
