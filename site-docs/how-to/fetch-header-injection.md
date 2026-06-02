# How to Configure Automatic Header Injection

Inject headers into outbound `fetch()` requests based on host and method rules.

## Inject a static header via CLI

```bash
mcp-v8 --fetch-header 'host=api.github.com,header=Authorization,value=Bearer ghp_xxx' \
       --policies-json '{"fetch": {"policies": [{"url": "file:///path/to/fetch.rego"}]}}'
```

Format: `host=<host>,header=<name>,value=<val>[,methods=GET;POST]`

Restrict to specific methods:

```bash
mcp-v8 --fetch-header 'host=api.example.com,header=X-Api-Key,value=secret123,methods=GET;POST'
```

## Inject multiple headers

Repeat the flag:

```bash
mcp-v8 --fetch-header 'host=api.github.com,header=Authorization,value=Bearer ghp_xxx' \
       --fetch-header 'host=api.example.com,header=X-Api-Key,value=key123'
```

## Use a JSON config file

Create a JSON file with header injection rules:

```json
[
  {
    "host": "api.github.com",
    "methods": ["GET", "POST"],
    "headers": {
      "Authorization": "Bearer ghp_xxx"
    }
  },
  {
    "host": "api.example.com",
    "headers": {
      "X-Api-Key": "key123"
    }
  }
]
```

Load it with:

```bash
mcp-v8 --fetch-header-config /etc/mcp-v8/fetch-headers.json
```

## OAuth client credentials

The config file also supports OAuth client-credentials flow for automatic bearer token acquisition:

```json
[
  {
    "host": "api.example.com",
    "headers": {},
    "oauth": {
      "header": "Authorization",
      "token_url": "https://auth.example.com/oauth/token",
      "client_id": "my-client-id",
      "client_secret": "my-client-secret",
      "scope": "api:read"
    }
  }
]
```

The token is acquired automatically and refreshed before expiry.

## Precedence

Headers set directly in JavaScript `fetch()` calls take precedence over injected headers. Injection only fills in headers not already set by the caller.
