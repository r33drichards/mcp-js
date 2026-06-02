# Network Access (Fetch API) Reference

The `fetch()` function is available when the `fetch` section is configured in `--policies-json`. Every request is evaluated against the policy chain before execution.

## fetch() Signature

```js
const response = await fetch(url, options?)
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | `string` | Yes | Request URL |
| `options` | `RequestInit?` | No | Request configuration |

## RequestInit

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `method` | `string` | `"GET"` | HTTP method |
| `headers` | `object` | `{}` | Request headers (key-value string pairs) |
| `body` | `string?` | `null` | Request body |

## Response Properties

| Property | Type | Description |
|----------|------|-------------|
| `ok` | `boolean` | `true` if status is 200-299 |
| `status` | `number` | HTTP status code |
| `statusText` | `string` | HTTP status text |
| `url` | `string` | Final URL after redirects |
| `redirected` | `boolean` | Whether the request was redirected |
| `type` | `string` | Response type (always `"basic"`) |
| `bodyUsed` | `boolean` | Whether the body has been consumed |

## Response Methods

| Method | Return Type | Description |
|--------|-------------|-------------|
| `text()` | `Promise<string>` | Body as UTF-8 string |
| `json()` | `Promise<any>` | Body parsed as JSON |
| `clone()` | `Response` | Create a copy of the response |

## Headers Object

The `response.headers` object provides:

| Method | Signature | Description |
|--------|-----------|-------------|
| `get` | `get(name: string): string?` | Get header value by name |
| `has` | `has(name: string): boolean` | Check if header exists |
| `entries` | `entries(): [string, string][]` | All header name-value pairs |
| `keys` | `keys(): string[]` | All header names |
| `values` | `values(): string[]` | All header values |
| `forEach` | `forEach(fn: (value, name) => void)` | Iterate over headers |

## Fetch HTTP Client Configuration

| Property | Value |
|----------|-------|
| Request timeout | 30 seconds |

## Fetch Policy Input Schema

Each `fetch()` call generates this policy input:

```json
{
  "operation": "fetch",
  "url": "https://api.example.com/data",
  "method": "GET",
  "headers": {
    "Content-Type": "application/json"
  },
  "url_parsed": {
    "scheme": "https",
    "host": "api.example.com",
    "port": "",
    "path": "/data",
    "query": ""
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `operation` | `string` | Always `"fetch"` |
| `url` | `string` | Full request URL |
| `method` | `string` | HTTP method (uppercase) |
| `headers` | `object` | Request headers |
| `url_parsed.scheme` | `string` | URL scheme |
| `url_parsed.host` | `string` | URL host |
| `url_parsed.port` | `string` | URL port (empty string if default) |
| `url_parsed.path` | `string` | URL path |
| `url_parsed.query` | `string` | URL query string |

## Policy Configuration

Fetch policies are configured in the `fetch` section of `--policies-json`:

```json
{
  "fetch": {
    "mode": "all",
    "policies": [
      {"url": "file:///path/to/fetch.rego"}
    ]
  }
}
```

Default OPA REST path: `mcp/fetch`
Default local Rego rule: `data.mcp.fetch.allow`
