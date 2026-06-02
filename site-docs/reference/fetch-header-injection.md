# Fetch Header Injection Reference

Automatically inject headers into outgoing `fetch()` requests based on host and method matching. Supports static headers and OAuth 2.0 client credentials.

## --fetch-header CLI Syntax

### Static Header

```
--fetch-header host=<HOST>,header=<NAME>,value=<VALUE>[,methods=<M1>;<M2>]
```

### OAuth Client Credentials

```
--fetch-header host=<HOST>,header=<NAME>,token_url=<URL>,client_id=<ID>,client_secret=<SECRET>[,scope=<SCOPE>][,refresh_buffer_secs=<SECS>][,methods=<M1>;<M2>]
```

Can be specified multiple times.

### CLI Key Reference

| Key | Required | Description |
|-----|----------|-------------|
| `host` | Yes | Host pattern to match |
| `header` | Yes | Header name to inject |
| `value` | For static | Static header value |
| `token_url` | For OAuth | Token endpoint URL |
| `client_id` | For OAuth | OAuth client ID |
| `client_secret` | For OAuth | OAuth client secret |
| `scope` | No | OAuth scope |
| `refresh_buffer_secs` | No | Seconds before expiry to refresh (default: 30) |
| `methods` | No | Semicolon-separated HTTP methods (empty = all methods) |

Static (`value`) and OAuth (`token_url`, `client_id`, `client_secret`) keys cannot be mixed in the same rule.

## --fetch-header-config JSON Format

Path to a JSON file containing an array of rule objects.

```json
[
  {
    "host": "api.github.com",
    "methods": ["GET", "POST"],
    "headers": {
      "Authorization": "Bearer ghp_xxxx",
      "X-Custom": "value"
    }
  },
  {
    "host": "*.internal.example.com",
    "auth": {
      "type": "oauth_client_credentials",
      "header": "Authorization",
      "token_url": "https://auth.example.com/token",
      "client_id": "my-client",
      "client_secret": "my-secret",
      "scope": "api:read",
      "refresh_buffer_secs": 30
    }
  }
]
```

### Rule Object Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `host` | `string` | Yes | Host pattern to match |
| `methods` | `string[]` | No | HTTP methods to match (empty = all) |
| `headers` | `object` | Exclusive with `auth` | Static headers map (name -> value) |
| `auth` | `object` | Exclusive with `headers` | OAuth configuration |

A rule must have either `headers` or `auth`, not both, and not neither. Unknown fields are rejected (`deny_unknown_fields`).

### Auth Object Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `type` | `string` | Yes | -- | Must be `"oauth_client_credentials"` |
| `header` | `string` | Yes | -- | Header name to inject (e.g., `"Authorization"`) |
| `token_url` | `string` | Yes | -- | OAuth token endpoint URL |
| `client_id` | `string` | Yes | -- | OAuth client ID |
| `client_secret` | `string` | Yes | -- | OAuth client secret |
| `scope` | `string?` | No | `null` | OAuth scope |
| `refresh_buffer_secs` | `u64` | No | `30` | Seconds before expiry to proactively refresh |

## Host Matching Rules

| Pattern | Matches | Example |
|---------|---------|---------|
| `api.github.com` | Exact match only | `api.github.com` |
| `*.github.com` | Suffix match and bare domain | `api.github.com`, `raw.github.com`, `github.com` |

- Matching is **case-insensitive**
- Wildcard patterns must start with `*` (e.g., `*.example.com`)

## Method Matching

- Methods are normalized to uppercase
- Empty methods list means all methods match
- Methods specified via CLI use semicolons: `methods=GET;POST`
- Methods specified via JSON use arrays: `["GET", "POST"]`

## Precedence

User-provided headers in the `fetch()` call take precedence over injected headers. If the user already sets a header that a rule would inject, the user's value is preserved.

## OAuth Token Caching

OAuth tokens are cached in memory and reused until they expire (minus `refresh_buffer_secs`). Token expiry is determined from:

1. The `expires_in` field in the token response (if present)
2. The `exp` claim in the JWT access token (fallback if `expires_in` is absent)

When a cached token has a `refresh_token`, the client attempts a refresh grant before falling back to a new client credentials grant. Concurrent token requests are coalesced into a single in-flight request.

## Validation Rules

All required string values are trimmed and rejected if blank:

- `host` cannot be empty
- `header` cannot be empty
- `value` cannot be empty (static rules)
- `token_url`, `client_id`, `client_secret` cannot be empty (OAuth rules)
- Static `headers` map cannot be empty
- Case-variant duplicate header names in the same static rule are rejected (e.g., `Authorization` and `authorization`)
