# Network access with fetch

Recipes for enabling outbound HTTP, restricting which URLs are reachable, and
injecting authentication headers server-side.

## Enable fetch with a domain allowlist

`fetch()` is available in JS only when the server is started with a
`--policies-json` configuration that includes a `fetch` section. Without it,
any `fetch()` call throws an error.

Create a policy that allows only specific domains (`fetch.rego`):

```rego
package mcp.fetch

default allow = false

allow if {
    input.method == "GET"
    input.url_parsed.host == "api.example.com"
}
```

Create the policies configuration (`policies.json`):

```json
{
  "fetch": {
    "policies": [
      { "url": "file:///path/to/fetch.rego" }
    ]
  }
}
```

Start the server:

```bash
mcp-v8 --http-port=8080 --policies-json=/path/to/policies.json
```

Any `fetch()` call where the policy returns `false` raises a JS error:
`fetch denied by policy: GET https://other.example.com/ is not allowed`.

For the full policy input shape, see the
[Reference page](../reference/fetch.md#policy-input-shape).

## Enable fetch with no per-request restrictions

To allow every outbound request (useful in trusted environments), configure a
fetch section with an empty `policies` list:

```json
{
  "fetch": {
    "policies": []
  }
}
```

An empty chain always allows. You can also pass inline JSON directly to the
flag:

```bash
mcp-v8 --http-port=8080 --policies-json='{"fetch":{"policies":[]}}'
```

## Inject a static auth header

Use `--fetch-header` to attach a fixed header value to every request targeting
a specific host. The header is injected by the server before the policy
evaluates, so JS code never sees or supplies the credential.

```bash
mcp-v8 --http-port=8080 \
  --policies-json=/path/to/policies.json \
  --fetch-header "host=api.example.com,header=Authorization,value=Bearer my-token"
```

Restrict injection to specific HTTP methods with `methods=` (semicolon-separated):

```bash
--fetch-header "host=api.example.com,methods=GET;POST,header=X-Api-Key,value=abc123"
```

If JS code already sets the same header, the JS-provided value takes precedence
and the injected value is skipped.

The flag is repeatable; each `--fetch-header` adds one rule. Rules are
evaluated in declaration order and the first matching rule wins per header.

## Inject OAuth client-credentials tokens

For services that require a bearer token from an OAuth token endpoint, use the
OAuth form of `--fetch-header`:

```bash
mcp-v8 --http-port=8080 \
  --policies-json=/path/to/policies.json \
  --fetch-header "host=api.example.com,header=Authorization,token_url=https://auth.example.com/token,client_id=my-client,client_secret=my-secret,scope=read:all"
```

The server acquires a token via the `client_credentials` grant, caches it in
memory, and refreshes it automatically before expiry. The default refresh
buffer is 30 seconds (token is considered expired 30 s before its stated
expiry). Override with `refresh_buffer_secs=`:

```bash
--fetch-header "host=api.example.com,header=Authorization,token_url=https://auth.example.com/token,client_id=my-client,client_secret=my-secret,scope=read:all,refresh_buffer_secs=60"
```

The injected header value has the form `<token_type> <access_token>`, where
`token_type` comes from the token endpoint response (defaults to `Bearer`).

If the token endpoint also returns a `refresh_token`, the server uses the
refresh-token grant on subsequent refreshes and falls back to
`client_credentials` if the refresh fails.

You cannot mix `value=` and OAuth keys (`token_url`, `client_id`,
`client_secret`) in the same `--fetch-header` entry.

## Use a --fetch-header-config JSON file

For many rules or for secrets that should not appear on the command line, put
rules in a JSON file:

```bash
mcp-v8 --http-port=8080 \
  --policies-json=/path/to/policies.json \
  --fetch-header-config=/path/to/header-rules.json
```

The file must be a JSON array of rule objects. Each rule has:

- `host` (string, required) — exact hostname or `*.suffix` wildcard.
- `methods` (array of strings, optional) — restrict to these HTTP methods;
  omit or use `[]` to match all methods.
- Exactly one of:
  - `headers` — object mapping header names to static values.
  - `auth` — OAuth client-credentials configuration object.

Static example:

```json
[
  {
    "host": "api.example.com",
    "methods": ["GET", "POST"],
    "headers": {
      "Authorization": "Bearer my-token",
      "X-Tenant-Id": "acme"
    }
  }
]
```

OAuth example:

```json
[
  {
    "host": "api.example.com",
    "auth": {
      "type": "oauth_client_credentials",
      "header": "Authorization",
      "token_url": "https://auth.example.com/token",
      "client_id": "my-client",
      "client_secret": "my-secret",
      "scope": "read:all",
      "refresh_buffer_secs": 30
    }
  }
]
```

`headers` and `auth` are mutually exclusive per rule. The only supported
`auth.type` value is `"oauth_client_credentials"`. Unknown JSON fields cause a
startup error.

`--fetch-header` (CLI) and `--fetch-header-config` (file) can be used
together; CLI rules are appended first.

## Use a wildcard host pattern

Both `--fetch-header` and the JSON config support a `*.` prefix to match a
hostname and all its subdomains:

```bash
--fetch-header "host=*.github.com,header=Authorization,value=Bearer ghp_token"
```

`*.github.com` matches `api.github.com`, `raw.github.com`, and `github.com`
itself. Matching is case-insensitive.

## See also

- [Quick-start: Network access with fetch](../tutorials/fetch.md)
- [Concepts: Network access with fetch](../concepts/fetch.md)
- [Reference: Network access with fetch](../reference/fetch.md)
- [Concepts: Security policies](../concepts/policies.md)
- [Reference: CLI flags](../reference/cli-flags.md)
