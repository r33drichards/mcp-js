# Authentication (JWT/JWKS)

Complete reference for JWT/JWKS authentication in mcp-v8. Covers the CLI flag, environment variable, accepted headers, verification logic, and log output.

## CLI flag and environment variable

| Item | Value |
|---|---|
| Flag | `--jwks-url=<URL>` |
| Environment variable | `JWKS_URL` |
| Type | `String` (URL) |
| Required | No — omitting the flag disables all JWT verification |
| Conflicts | None |

When both `JWKS_URL` and `--jwks-url` are provided, `--jwks-url` takes precedence (standard clap/env precedence).

Example:

```bash
mcp-v8 --http-port=3000 --jwks-url=https://idp.example.com/.well-known/jwks.json
```

## Applicable transports

| Transport | Verification runs |
|---|---|
| Streamable HTTP (`--http-port`) | Yes — on every `initialize` request |
| SSE (`--sse-port`) | Yes — on every `initialize` request |
| stdio (default) | No — no HTTP context is available |

## Accepted token headers

Verification looks for a token in the headers of the MCP `initialize` HTTP request. Headers are tried in the order shown; the first match is used.

| Priority | Header name | Expected format |
|---|---|---|
| 1 | `Authorization` | `Bearer <jwt>` — the `Bearer ` prefix is stripped before verification |
| 2 | `agent-session` | Raw JWT string, no prefix |

If both headers are present, `Authorization: Bearer` is used and `agent-session` is ignored. If neither header is present, verification is skipped for that request.

## Key store behavior

`JwksKeyStore` maintains an in-memory map of `kid → DecodingKey`:

- Keys are fetched from `--jwks-url` at server startup. The server will fail to start if the endpoint is unreachable or returns no usable keys.
- Keys are indexed by `kid` (the `kid` field in each JWK). JWK entries without a `kid` are silently skipped.
- On a cache hit, the locally cached key is used.
- On a cache miss (unknown `kid`), the JWKS endpoint is re-fetched and the cache is replaced. If the refresh also fails or still does not contain the `kid`, verification fails for that token.

## Verification logic

`SessionVerifier::verify(token)` returns `true` only when all steps below succeed:

| Step | Details |
|---|---|
| 1. Parse JWT header | Token must be a well-formed JWT; malformed bytes return `false` immediately |
| 2. Require `kid` | Tokens with no `kid` in the JWT header return `false` |
| 3. Resolve key | Look up `kid` in cache; refresh from JWKS endpoint on miss |
| 4. Decode / verify | Decode with the algorithm declared in the JWT header (`alg`) and the resolved public key |

## Claims checked and not checked

| Claim | Checked |
|---|---|
| Signature | Yes — required for `decode()` to succeed |
| `alg` | Yes — algorithm is taken from the JWT header; only algorithms supported by `jsonwebtoken` are accepted |
| `kid` | Yes — must be present in JWT header and resolvable to a key |
| `exp` | Not required to be present; if present, expiration is validated by the `jsonwebtoken` library |
| `aud` | Not validated (`validate_aud = false`) |
| `iss` | Not required; not validated |
| `sub`, `azp`, custom claims | Not inspected |

## Enforcement behavior

In the current implementation, the verification result is **logged only**. The `initialize` handler always returns a successful `InitializeResult` regardless of whether the token is valid, invalid, or absent.

| Scenario | Log level | Response |
|---|---|---|
| Token present and verified (stateful mode) | `INFO JWT verified` | `InitializeResult` (success) |
| Token present and verified (stateless mode) | `INFO JWT verified in stateless mode` | `InitializeResult` (success) |
| Token present but fails verification | `WARN JWT present but failed verification` | `InitializeResult` (success) |
| No token header | `DEBUG No Authorization/AgentSession header in initialize request` | `InitializeResult` (success) |
| `--jwks-url` not configured | (no verification attempted) | `InitializeResult` (success) |

## Example: docker-compose.secure-sessions.yml configuration

```yaml
services:
  mcp-js:
    environment:
      - JWKS_URL=http://keycloak:8080/realms/mcp/protocol/openid-connect/certs
    command:
      - --http-port=3000
```

## Keycloak realm configuration

The bundled `keycloak/mcp-realm.json` configures:

| Field | Value |
|---|---|
| Realm name | `mcp` |
| Client ID | `mcp-client` |
| Client secret | `mcp-client-secret` |
| Grant type | `client_credentials` |
| JWKS URL (from inside Docker network) | `http://keycloak:8080/realms/mcp/protocol/openid-connect/certs` |
| Token URL (from host) | `http://localhost:8080/realms/mcp/protocol/openid-connect/token` |

## See also

- [Quick-start: Authentication](../tutorials/authentication.md)
- [How-to: Authentication](../how-to/authentication.md)
- [Concepts: Authentication](../concepts/authentication.md)
- [Reference: CLI flags](../reference/cli-flags.md)
- [Stateful sessions & heap snapshots](../reference/sessions-and-heaps.md)
- [Network access with fetch](../reference/fetch.md)
