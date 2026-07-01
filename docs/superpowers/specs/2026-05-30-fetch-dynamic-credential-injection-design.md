# Fetch Dynamic Credential Injection Design

Date: 2026-05-30

## Summary

Add dynamic OAuth2 client-credentials-backed header injection for the fetch tool, alongside the existing static header injection rules. A matching fetch rule should be able to mint an access token from `client_id`, `client_secret`, and `token_url`, cache that token in memory, inject it into the outbound request, and refresh or reacquire it when it expires.

The design keeps the current fetch header injection entry point and precedence model:

- Rules are matched by `host` and optional `methods`
- User-supplied headers still win over injected headers
- Static header injection remains supported without behavior changes
- Dynamic credentials are supported from both CLI flags and JSON config

## Goals

- Support dynamic credential injection from both CLI and JSON config
- Reuse cached access tokens until they are near expiry
- Refresh expired access tokens when a refresh token exists
- Fall back to reacquiring a token with the client credentials grant when refresh is unavailable or fails
- Keep existing static header injection behavior backward-compatible
- Provide test coverage at the parser, unit, integration, and end-to-end levels

## Non-Goals

- Persist tokens across server restarts
- Introduce a generic credential plugin framework
- Change the current fetch authorization precedence rules
- Add interactive OAuth flows

## Current State

Today, fetch header injection is modeled as a static `HeaderRule` with:

- `host`
- `methods`
- `headers: HashMap<String, String>`

Rules are loaded in `server/src/main.rs`, stored in `FetchConfig`, and applied synchronously in `server/src/engine/fetch.rs` before the outbound request is sent. Tests currently cover static header injection behavior in `server/tests/fetch_header_injection.rs`.

The repo already includes a TypeScript helper, `scripts/oauth-client-credentials-provider.ts`, which implements a similar in-memory token cache for Keycloak-backed client credentials. That script provides a useful behavioral reference for expiry handling and Keycloak end-to-end testing.

## Proposed Architecture

### Rule Model

Replace the current static-only fetch rule model with an auth-aware rule representation that supports two injection modes:

1. Static header injection
2. OAuth2 client credentials dynamic injection

The match surface remains unchanged:

- `host`
- `methods`

The injection surface becomes a tagged variant:

- Static: inject one or more fixed headers
- Dynamic: compute a header value from a reusable token source

Recommended internal shape:

```rust
pub struct FetchRule {
    pub host: String,
    pub methods: Vec<String>,
    pub injection: HeaderInjection,
}

pub enum HeaderInjection {
    Static {
        headers: HashMap<String, String>,
    },
    OAuthClientCredentials {
        header: String,
        token_url: String,
        client_id: String,
        client_secret: String,
        scope: Option<String>,
        refresh_buffer_secs: u64,
    },
}
```

The exact type names can vary, but the design requires an explicit internal distinction between static and dynamic injection.

### Token Source

Each dynamic rule gets a dedicated in-memory token source owned by the server process. The token source should mirror the behavior of Go's `ReuseTokenSource`:

- Return the cached token if it is still valid
- Refresh or reacquire only when the token is missing or expired
- Avoid repeated token endpoint calls while a valid token is cached

The token source should expose a single operation used by fetch injection:

```rust
async fn authorization_header_value(&self) -> Result<String>
```

Internally it should:

1. Check the cached token state
2. Return the cached bearer token when it is still valid
3. If expired and a refresh token exists, attempt refresh once
4. If refresh is unavailable or fails, request a new token using the client credentials grant
5. Update the cache and return the new bearer token

### Cache And Concurrency

Dynamic rules need refresh coordination so multiple requests do not stampede the token endpoint when one token expires. The token source should therefore keep:

- Cached token payload
- Acquisition timestamp
- Derived expiry time
- Async synchronization around refresh/reacquire

A practical Rust shape is:

- `Arc<TokenSource>`
- interior mutability via `tokio::sync::Mutex` or `RwLock`

The lock should cover the refresh decision and update path, not the entire fetch request lifecycle.

### Expiry Detection

Expiry should be derived in this order:

1. `expires_in` from the token endpoint response
2. JWT `exp` claim from the access token when `expires_in` is absent
3. If neither is available, treat the token as immediately expiring and reacquire on next use

The token source should apply a configurable refresh buffer, defaulting to 30 seconds, to avoid injecting nearly expired tokens.

## Configuration Design

### CLI

Keep the current `--fetch-header` flag and extend its accepted rule syntax.

Static rules remain unchanged:

```text
host=<host>,header=<name>,value=<val>[,methods=GET;POST]
```

Dynamic rules add OAuth-specific fields:

```text
host=<host>,header=Authorization,token_url=<url>,client_id=<id>,client_secret=<secret>[,scope=<scope>][,methods=GET;POST][,refresh_buffer_secs=30]
```

CLI parsing rules:

- Static mode requires `host`, `header`, `value`
- Dynamic mode requires `host`, `header`, `token_url`, `client_id`, `client_secret`
- `methods`, `scope`, and `refresh_buffer_secs` are optional in dynamic mode
- Mixing static keys (`value`) with dynamic keys (`token_url`, `client_id`, `client_secret`) in the same rule is invalid
- Unknown keys remain invalid

The implementation should continue supporting repeated `--fetch-header` flags.

### JSON Config

JSON config should support both backward-compatible static rules and explicit dynamic rules.

Static rules remain supported in the current shape:

```json
[
  {
    "host": "api.github.com",
    "methods": ["GET"],
    "headers": {
      "Authorization": "Bearer ghp_xxxx"
    }
  }
]
```

Dynamic rules use an explicit `auth` block:

```json
[
  {
    "host": "api.example.com",
    "methods": ["GET", "POST"],
    "auth": {
      "type": "oauth_client_credentials",
      "header": "Authorization",
      "token_url": "https://issuer.example.com/oauth/token",
      "client_id": "my-client",
      "client_secret": "my-secret",
      "scope": "read:all",
      "refresh_buffer_secs": 30
    }
  }
]
```

JSON validation rules:

- Static rules require `headers`
- Dynamic rules require `auth.type == "oauth_client_credentials"`
- Dynamic `auth` requires `header`, `token_url`, `client_id`, and `client_secret`
- A rule cannot define both `headers` and `auth`
- Unknown auth `type` values are rejected at startup

## Fetch Runtime Flow

Fetch request processing remains structurally the same:

1. Normalize user-supplied request headers
2. Match configured fetch rules by host and method
3. Apply injected headers for each matched rule
4. Run policy evaluation
5. Dispatch the outbound HTTP request

Dynamic rule handling changes step 3:

- If the target header is already present in the normalized user headers, do nothing
- Otherwise, obtain the current header value from the dynamic token source
- Inject `Authorization: Bearer <access_token>` or the configured header name/value format

The token endpoint call itself should use the server's own HTTP client path and should not recursively pass through fetch policy/header injection.

## Error Handling

### Startup Validation Errors

Invalid rule configurations should fail server startup with actionable messages. Examples:

- Missing `client_secret` in a dynamic CLI rule
- Mixed `value` plus `token_url` in a single CLI rule
- JSON rule containing both `headers` and `auth`
- Unsupported JSON auth type

### Runtime Token Errors

If token acquisition fails for a matched dynamic rule:

- The fetch request should fail before the outbound request is sent
- The error should identify the rule host and token acquisition stage
- The error must not include `client_secret`, refresh tokens, or full access tokens

Refresh-specific behavior:

- If an expired token has a refresh token, attempt refresh first
- If refresh fails, attempt one full client-credentials reacquire
- If reacquire fails, fail the fetch request

## Security Considerations

- Client secrets and tokens must never be written to logs
- Dynamic credentials remain invisible to sandboxed JavaScript unless the user explicitly sets the same header themselves
- Token caches are in-memory only and cleared on process exit
- Header precedence remains unchanged: user-supplied headers win
- Token requests should use TLS validation provided by `reqwest` defaults unless the repo already has a broader transport override policy

## Testing Strategy

### Parser And Validation Tests

Add unit tests around CLI and JSON parsing in `server/src/main.rs` or extracted parsing helpers:

- Static CLI rule parses successfully
- Dynamic CLI rule parses successfully
- Missing required dynamic fields fail with clear errors
- Mixed static and dynamic fields fail
- Static JSON rule still parses
- Dynamic JSON rule parses
- Conflicting `headers` plus `auth` JSON fails
- Unsupported auth type fails

### Token Source Unit Tests

Add focused tests for the reusable token source:

- Reuses cached token before expiry
- Reacquires token after expiry
- Uses refresh token when available
- Falls back to reacquire when refresh fails
- Uses JWT `exp` when `expires_in` is missing
- Treats unknown expiry as immediately stale
- Coalesces concurrent refresh attempts into a single outbound token request

These tests should use a local mock token server instead of a real IdP.

### Fetch Integration Tests

Extend `server/tests/fetch_header_injection.rs` or add a sibling integration test file for dynamic auth:

- Matching request receives injected bearer token from mock token server
- Repeated requests reuse the cached token
- Post-expiry request triggers refresh or reacquire
- User-provided `Authorization` overrides dynamic injection
- Non-matching host/method performs no token lookup

### End-To-End Keycloak Coverage

Use the existing Keycloak setup in:

- `docker-compose.secure-sessions.yml`
- `keycloak/mcp-realm.json`
- `scripts/oauth-client-credentials-provider.ts`
- `scripts/test-keycloak-sessions.ts`

Add a fetch-focused e2e path that boots Keycloak, OPA, and the server with a dynamic fetch rule pointing to Keycloak's token endpoint. The e2e test should verify:

- A fetch to a protected mock upstream receives a Keycloak-issued bearer token
- The token is reused across multiple fetches before expiry
- Expiry handling works by forcing a short-lived token setup or simulated expiry
- The dynamic rule works under the same secure-session environment already used for JWKS-backed session tests

The exact e2e harness can be shell-based, Rust-based, or Deno-based, but it should reuse the current secure-session compose assets rather than introducing a parallel local auth environment.

## Documentation Changes

Update the user-facing documentation in:

- `README.md`
- `server/README.md`

Documentation should cover:

- New CLI dynamic syntax
- New JSON config structure
- Token reuse and refresh behavior
- Header precedence
- Security notes around secret handling

Add a dedicated tutorial for OAuth client-credentials-based fetch injection, ideally alongside the existing GitHub token tutorial, with a Keycloak example for local development.

## Implementation Notes

- Prefer extracting fetch rule parsing into smaller helpers if `server/src/main.rs` becomes harder to follow
- Keep the fetch engine boundary clean: matching logic should not need to know CLI syntax details
- Store prebuilt dynamic token sources in `FetchConfig` rather than reconstructing them per request
- Avoid broad refactors unrelated to fetch auth injection

## Open Design Decisions Resolved

- Dynamic credentials are supported in both CLI and JSON config
- `token_url` is configured per rule
- The runtime uses Go-style token reuse semantics
- Refresh is attempted when a refresh token exists, with fallback to full client-credentials reacquisition
- Keycloak-backed end-to-end testing is part of the plan

## Success Criteria

The feature is complete when:

- Users can configure dynamic fetch auth via CLI and JSON config
- Matching requests inject bearer tokens acquired from `token_url`
- Tokens are reused until near expiry
- Expired tokens refresh or reacquire without user code changes
- Static header injection still behaves exactly as before
- Automated tests cover parsing, token reuse logic, fetch integration, and Keycloak-backed e2e behavior
