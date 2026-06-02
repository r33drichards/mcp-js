# How to Enable and Use fetch()

Enable network access in the JavaScript runtime via OPA-gated `fetch()`.

## Enable fetch with a policy

There is no network access by default. To enable `fetch()`, configure a fetch policy:

```bash
mcp-v8 --policies-json '{"fetch": {"policies": [{"url": "file:///etc/mcp-v8/fetch.rego"}]}}'
```

## Write a permissive fetch policy

Allow all outbound requests:

```rego
package mcp.fetch

default allow = true
```

## Write a restrictive fetch policy

Allow only specific hosts and methods:

```rego
package mcp.fetch

default allow = false

allow {
    input.url == "https://api.example.com/"
    input.method == "GET"
}

allow {
    startswith(input.url, "https://api.github.com/")
}
```

The policy input includes `url`, `method`, and `headers`.

## Use fetch() in JavaScript

`fetch()` follows the web-standard Fetch API:

```js
const resp = await fetch("https://api.example.com/data");
const data = await resp.json();
console.log(JSON.stringify(data));
```

Available response properties and methods:

- `.ok`, `.status`, `.statusText`, `.url`
- `.headers.get(name)`
- `.text()` -- returns a Promise
- `.json()` -- returns a Promise

## POST requests

```js
const resp = await fetch("https://api.example.com/items", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ name: "test" })
});
console.log(resp.status);
```

## Combine with header injection

For APIs that need authentication, combine with `--fetch-header` or `--fetch-header-config` to inject headers automatically. See [Fetch Header Injection](fetch-header-injection.md).
