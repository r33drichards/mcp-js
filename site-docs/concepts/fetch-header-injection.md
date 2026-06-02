# Fetch Header Injection

Header injection automatically attaches headers (typically authentication credentials) to outgoing `fetch()` requests based on host and method matching rules. This allows operators to configure server-side credentials that JavaScript code uses implicitly, without the code needing to know the credential values.

## Why Header Injection Exists

AI agents often need to call authenticated APIs. Embedding credentials in the JavaScript code sent to the agent is problematic: credentials leak into logs, session histories, and potentially into the model's context window. Header injection solves this by keeping credentials in server configuration and transparently injecting them into matching requests.

## Static vs. Dynamic Injection

### Static Headers

Static injection adds fixed header values to matching requests:

```json
{
  "host": "api.github.com",
  "methods": ["GET", "POST"],
  "headers": {
    "Authorization": "Bearer ghp_xxxxxxxxxxxx",
    "X-Custom-Header": "custom-value"
  }
}
```

Multiple headers can be injected per rule. Static headers are appropriate when the credential does not expire or when the operator manages rotation externally.

### Dynamic Headers (OAuth2 Client Credentials)

Dynamic injection uses the OAuth2 client credentials grant to obtain and cache access tokens:

```json
{
  "host": "api.example.com",
  "auth": {
    "type": "oauth_client_credentials",
    "header": "Authorization",
    "token_url": "https://auth.example.com/oauth/token",
    "client_id": "my-client-id",
    "client_secret": "my-client-secret",
    "scope": "read:data write:data",
    "refresh_buffer_secs": 30
  }
}
```

Dynamic injection is appropriate when the API uses OAuth2 and tokens have a limited lifetime.

## Host Matching

Each injection rule specifies a host pattern. Two matching modes are supported:

### Exact match

```json
{ "host": "api.github.com" }
```

Matches only requests to `api.github.com`. Case-insensitive.

### Wildcard match

```json
{ "host": "*.github.com" }
```

The leading `*` matches any subdomain. This matches `api.github.com`, `raw.github.com`, and even `github.com` itself (the wildcard matches zero or more subdomain levels).

## Method Filtering

Rules can optionally restrict to specific HTTP methods:

```json
{ "host": "api.example.com", "methods": ["GET", "POST"] }
```

When `methods` is empty or omitted, the rule matches all HTTP methods. Method matching is case-insensitive.

## Header Precedence

User-provided headers always take precedence over injected headers. If JavaScript code includes an `Authorization` header in the `fetch()` call and an injection rule would also add `Authorization`, the user-provided value wins. The injection rule is skipped for that header.

This design ensures that code can override injected credentials when necessary, and that injection never silently replaces headers that the code explicitly set.

## Token Caching, Refresh, and Reacquire Lifecycle

Dynamic OAuth2 injection follows a multi-stage token lifecycle:

### 1. Initial acquisition

On the first matching request, the token source sends a `client_credentials` grant to the token URL. The response provides an access token, token type, expiration time, and optionally a refresh token.

### 2. Caching

The token is cached in memory. Subsequent matching requests reuse the cached token without contacting the token endpoint, as long as the token is still valid.

### 3. Proactive refresh

The `refresh_buffer_secs` parameter (default: 30 seconds) defines how far before expiration the token should be refreshed. If a request arrives within this buffer period, the token source proactively acquires a new token. This avoids using a token that is about to expire, reducing the chance of a request failing due to an expired token.

### 4. Refresh token flow

If the cached token has expired (or is within the refresh buffer) and a refresh token is available, the token source first attempts a `refresh_token` grant. This is typically faster and more efficient than a full client credentials grant.

### 5. Reacquire on failure

If the refresh fails (e.g., the refresh token has been revoked), the token source falls back to a fresh `client_credentials` grant. If this also fails, the injection rule produces an error and the `fetch()` call fails with a sanitized error message that does not leak credentials.

### 6. Error sanitization

Error messages from token acquisition are sanitized to prevent credential leakage. The error includes the header name and the host, but never the client secret, token URL response body, or token values. This is a deliberate security measure: even in error conditions, credentials should not appear in logs or agent context.

## Configuration Methods

Header injection rules can be configured in two ways:

### CLI flags

```
--fetch-header "host=api.github.com,header=Authorization,value=Bearer ghp_xxx"
--fetch-header "host=api.example.com,header=Authorization,token_url=https://auth.example.com/token,client_id=abc,client_secret=xyz"
```

### JSON config file

```
--fetch-header-config /path/to/fetch-rules.json
```

Both methods can be combined. CLI rules and file rules are merged into a single list. Validation ensures that static and dynamic injection are not mixed in a single rule, required fields are non-empty, and header names do not have case-variant duplicates.
