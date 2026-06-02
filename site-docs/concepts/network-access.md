# Network Access Model

Network access in mcp-v8 is disabled by default. When enabled through policy configuration, JavaScript code can make HTTP requests using a `fetch()` function that follows the web standard Fetch API. Every request passes through policy evaluation before it is sent, and optional header injection can automatically attach credentials to outgoing requests.

## Disabled by Default

Out of the box, the `fetch()` function does not exist in the JavaScript global scope. Attempting to call it produces a `ReferenceError`. This is intentional: network access is a powerful capability, and mcp-v8 follows a principle of least privilege.

To enable `fetch()`, you must configure a fetch policy chain (via `--policies-json`). The policy chain can be as permissive as "allow everything" or as restrictive as "allow only GET requests to api.example.com". But the key point is that the operator must explicitly choose to enable it.

## The fetch() API

When enabled, `fetch()` follows the web standard Fetch API:

```js
const response = await fetch("https://api.example.com/data", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ query: "test" })
});
```

### The Response Object

The response object provides:

- `response.ok` -- Boolean, true if status is 200-299
- `response.status` -- HTTP status code (number)
- `response.statusText` -- HTTP status text (string)
- `response.url` -- Final URL after redirects (string)
- `response.headers` -- Headers object with `.get(name)`, `.has(name)`, `.entries()`, `.keys()`, `.values()`, `.forEach()`
- `response.redirected` -- Boolean, true if the request was redirected
- `await response.text()` -- Response body as a string (returns a Promise)
- `await response.json()` -- Response body parsed as JSON (returns a Promise)
- `response.clone()` -- Create a copy of the response

The implementation uses reqwest on the Rust side for actual HTTP execution. The JavaScript wrapper formats the result into a Response-like object.

## Policy Evaluation Flow

Every `fetch()` call follows this sequence:

1. **Parse and validate** -- The URL is parsed, headers are normalized (lowercase keys), and the method is validated.
2. **Header injection** -- If header injection rules are configured, matching rules are applied. Injected headers do not override user-provided headers.
3. **Policy evaluation** -- The complete request (URL, method, headers, parsed URL components) is sent to the policy chain as a JSON input. The chain evaluates it against configured Rego rules and/or remote OPA servers.
4. **Allow or deny** -- If the policy returns `allow = true`, the request proceeds. If not, `fetch()` throws an error: `"fetch denied by policy: GET https://example.com is not allowed"`.
5. **Execute** -- The HTTP request is sent via reqwest with a 30-second timeout.
6. **Return** -- The response (status, headers, body) is serialized as JSON and returned to JavaScript.

The policy sees the request _after_ header injection. This is deliberate: it means policies can make decisions based on the actual headers that will be sent, including injected credentials.

## Header Injection and Policy Interaction

Header injection rules add credentials or other headers to outgoing requests based on host and method matching. These rules are applied _before_ policy evaluation. This ordering enables a powerful pattern:

1. Configure a header injection rule that adds an `Authorization` header for `api.example.com`.
2. Configure a fetch policy that requires an `Authorization` header for any request.
3. User code calls `fetch("https://api.example.com/data")` without any auth header.
4. The injection rule adds the `Authorization` header.
5. The policy evaluates the request _with_ the injected header and allows it.

This means the policy can enforce "all requests must be authenticated" without requiring user code to know the credentials.

## Async Op Architecture

The `fetch()` implementation uses deno_core's async op system. The JavaScript function calls `Deno.core.ops.op_fetch(url, method, headersJson, body)`, which returns a Promise. The Rust op function:

1. Clones the policy chain and HTTP client from OpState (before any `.await`).
2. Spawns a new Tokio task for the actual fetch work. This is necessary because the deeply nested async state machine from policy evaluation and reqwest can trigger RefCell re-entrancy issues in deno_core's op driver.
3. The spawned task evaluates the policy, makes the HTTP request, and returns the result.
4. The Promise resolves with a JSON string containing the response.

This architecture means `fetch()` is truly non-blocking: while one request is in flight, other JavaScript code can execute, timers can fire, and other async operations can proceed.
