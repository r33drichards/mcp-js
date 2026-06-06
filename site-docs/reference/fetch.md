# Network access with fetch

Complete reference for the JS `fetch()` API, the `--fetch-header` CLI flag,
the `--fetch-header-config` JSON schema, and the policy input shape.

## JS fetch() API

`fetch()` is available as `globalThis.fetch` when a fetch policy is configured.
The function signature follows the standard Fetch API subset described below.

### Signature

```js
fetch(url: string, init?: FetchInit): Promise<Response>
```

`url` must be a string; passing any other type throws a `TypeError`.

### FetchInit

| Property | Type | Default | Description |
|---|---|---|---|
| `method` | string | `"GET"` | HTTP method. Normalised to upper-case. |
| `headers` | object | `{}` | Request headers as plain key/value object. Keys are lower-cased before sending. |
| `body` | string | `""` | Request body. Any non-string value is coerced with `String()`. An empty string sends no body. |

### Response

| Property | Type | Description |
|---|---|---|
| `ok` | boolean | `true` if `status` is 200–299. |
| `status` | number | HTTP status code. |
| `statusText` | string | HTTP reason phrase (e.g. `"OK"`). |
| `url` | string | Final URL after any redirects. |
| `headers` | Headers | Response headers object (see below). |
| `redirected` | boolean | `true` if the final URL differs from the requested URL. |
| `type` | string | Always `"basic"`. |
| `bodyUsed` | boolean | Always `false`. |
| `text()` | `() => Promise<string>` | Resolves with the response body as a string. |
| `json()` | `() => Promise<any>` | Resolves with `JSON.parse(body)`. |
| `clone()` | `() => Response` | Returns a shallow clone of the response. |

### Headers

The `response.headers` object exposes the following methods:

| Method | Signature | Description |
|---|---|---|
| `get` | `(name: string) => string \| null` | Returns the header value, or `null` if absent. Name is case-insensitive. |
| `has` | `(name: string) => boolean` | Returns `true` if the header is present. |
| `entries` | `() => [string, string][]` | Returns all header `[name, value]` pairs. |
| `keys` | `() => string[]` | Returns all header names. |
| `values` | `() => string[]` | Returns all header values. |
| `forEach` | `(cb: (value, name, headers) => void) => void` | Iterates over all headers. |

Header names are stored and returned in lower-case.

### Example

```js
const response = await fetch("https://api.example.com/data", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ key: "value" }),
});

if (!response.ok) {
  throw new Error(`HTTP ${response.status}: ${response.statusText}`);
}

const data = await response.json();
console.log(data);
```

## --fetch-header CLI flag

Repeatable. Each use adds one header-injection rule. The argument is a
comma-separated list of `key=value` pairs.

### Common keys

| Key | Required | Description |
|---|---|---|
| `host` | Yes | Target hostname. Exact match, or `*.suffix` to match the suffix and all subdomains. Case-insensitive. |
| `methods` | No | Semicolon-separated list of HTTP methods this rule applies to. Omit to match all methods. Methods are normalised to upper-case. |
| `header` | Yes | Name of the header to inject. |

### Static injection keys

Use these keys together to inject a fixed header value.

| Key | Required | Description |
|---|---|---|
| `value` | Yes (static mode) | Header value to inject. |

Example:

```bash
--fetch-header "host=api.example.com,header=Authorization,value=Bearer my-token"
```

With method restriction:

```bash
--fetch-header "host=api.example.com,methods=GET;HEAD,header=Authorization,value=Bearer my-token"
```

### OAuth client-credentials keys

Use these keys to acquire tokens from a token endpoint. `value` must be absent.

| Key | Required | Default | Description |
|---|---|---|---|
| `token_url` | Yes | — | Token endpoint URL (`https://...`). |
| `client_id` | Yes | — | OAuth client identifier. |
| `client_secret` | Yes | — | OAuth client secret. |
| `scope` | No | — | Scope string passed as-is to the token endpoint. |
| `refresh_buffer_secs` | No | `30` | Refresh the token this many seconds before expiry. |

Example:

```bash
--fetch-header "host=api.example.com,header=Authorization,token_url=https://auth.example.com/token,client_id=my-client,client_secret=my-secret,scope=read:all"
```

`value` and the OAuth keys (`token_url`, `client_id`, `client_secret`) are
mutually exclusive in a single `--fetch-header` entry.

## --fetch-header-config JSON schema

Path to a JSON file containing an array of rule objects. Useful when there are
many rules or when secrets should not appear on the command line.

```bash
--fetch-header-config=/path/to/header-rules.json
```

The file is a JSON array. Each element has the following shape:

```json
{
  "host":    "<hostname or *.suffix>",
  "methods": ["GET", "POST"],
  "headers": { "<HeaderName>": "<value>", "...": "..." }
}
```

or (OAuth form):

```json
{
  "host":    "<hostname or *.suffix>",
  "methods": [],
  "auth": {
    "type":                "oauth_client_credentials",
    "header":              "<HeaderName>",
    "token_url":           "https://auth.example.com/token",
    "client_id":           "<client-id>",
    "client_secret":       "<client-secret>",
    "scope":               "<optional scope>",
    "refresh_buffer_secs": 30
  }
}
```

### Field reference

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `host` | string | Yes | — | Exact hostname or `*.suffix` wildcard. |
| `methods` | string[] | No | `[]` (all methods) | HTTP methods to match. |
| `headers` | object | One of `headers`/`auth` | — | Static header name→value map. |
| `auth` | object | One of `headers`/`auth` | — | OAuth configuration (see below). |

### auth object fields

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `type` | string | Yes | — | Must be `"oauth_client_credentials"`. |
| `header` | string | Yes | — | Header to inject the token into. |
| `token_url` | string | Yes | — | Token endpoint URL. |
| `client_id` | string | Yes | — | OAuth client identifier. |
| `client_secret` | string | Yes | — | OAuth client secret. |
| `scope` | string | No | — | Scope string, passed to the token endpoint. |
| `refresh_buffer_secs` | number | No | `30` | Refresh buffer in seconds. |

`headers` and `auth` are mutually exclusive. Extra JSON fields at any level
cause a startup error (`deny_unknown_fields`).

`--fetch-header` (CLI) and `--fetch-header-config` (file) may be combined; CLI
rules are loaded first, file rules are appended.

## Policy input shape

The fetch policy is evaluated for every `fetch()` call after header injection.
The entrypoint is `data.mcp.fetch.allow`.

```json
{
  "operation":  "fetch",
  "url":        "https://api.example.com/v1/items?q=foo",
  "method":     "GET",
  "headers": {
    "content-type":  "application/json",
    "authorization": "Bearer <injected-or-js-supplied-token>"
  },
  "url_parsed": {
    "scheme": "https",
    "host":   "api.example.com",
    "port":   null,
    "path":   "/v1/items",
    "query":  "q=foo"
  }
}
```

| Field | Type | Description |
|---|---|---|
| `operation` | string | Always `"fetch"`. |
| `url` | string | Full request URL as a string. |
| `method` | string | HTTP method, upper-cased. |
| `headers` | object | All request headers at evaluation time, including injected ones. Keys are lower-cased. |
| `url_parsed.scheme` | string | URL scheme (`"http"` or `"https"`). |
| `url_parsed.host` | string | Hostname without port. |
| `url_parsed.port` | number or null | Explicit port from the URL, or `null`. |
| `url_parsed.path` | string | URL path. |
| `url_parsed.query` | string | Query string without leading `?`; empty string if absent. |

## Defaults

| Setting | Default |
|---|---|
| `refresh_buffer_secs` | `30` |
| `methods` (CLI and JSON) | `[]` — matches all HTTP methods |
| HTTP client timeout | 30 seconds |
| `token_type` (if absent from token endpoint) | `"Bearer"` |

## See also

- [How-to: Network access with fetch](../how-to/fetch.md)
- [Concepts: Network access with fetch](../concepts/fetch.md)
- [Concepts: Security policies](../concepts/policies.md)
- [Reference: CLI flags](../reference/cli-flags.md)
