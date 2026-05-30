# OAuth Client-Credentials Fetch Injection

This tutorial shows how to use mcp-js fetch header injection with OAuth2 client credentials so the server can acquire, cache, refresh, and reuse bearer tokens for outbound `fetch()` calls. The JavaScript code running inside `run_js` never receives the client secret directly, but any upstream you allow can still observe the injected bearer token and reflect it back.

The examples below use the local Keycloak setup already included in this repo.

## What You Will Build

- A local Keycloak realm that issues tokens for the `mcp-client` service account.
- An `mcp-js` instance configured with a dynamic `--fetch-header` rule.
- A protected/mock upstream that receives the injected bearer token.

## Prerequisites

- Docker and Docker Compose
- Deno for running the end-to-end helper script

## Dynamic CLI Syntax

Dynamic OAuth injection uses the same `--fetch-header` flag as static headers, but replaces `value=` with token-source settings:

```bash
mcp-v8 --stateless \
  --policies-json '{"fetch":{"policies":[{"url":"http://localhost:8181"}]}}' \
  --fetch-header "host=api.example.com,header=Authorization,token_url=https://issuer.example.com/oauth2/token,client_id=my-client,client_secret=${CLIENT_SECRET},scope=read:all,refresh_buffer_secs=45"
```

Field summary:

- `host`: exact or wildcard host match for the outgoing fetch target.
- `header`: header to inject, usually `Authorization`.
- `token_url`: OAuth2 token endpoint.
- `client_id` / `client_secret`: client-credentials pair used by the server.
- `scope`: optional scope for the client-credentials grant.
- `refresh_buffer_secs`: optional early-refresh window, default `30`.

## JSON Config Syntax

For larger setups, put the rule in a JSON config file and point `--fetch-header-config` at it:

```json
[
  {
    "host": "api.example.com",
    "methods": ["GET", "POST"],
    "auth": {
      "type": "oauth_client_credentials",
      "header": "Authorization",
      "token_url": "https://issuer.example.com/oauth2/token",
      "client_id": "my-client",
      "client_secret": "my-secret",
      "scope": "read:all",
      "refresh_buffer_secs": 45
    }
  }
]
```

JSON config files are read literally. They do not expand shell-style placeholders such as `${CLIENT_SECRET}`.

Each rule must choose one style:

- Static `headers`
- Dynamic `auth`

Do not put both on the same rule.

## Local Keycloak Example

The repo already includes a ready-made secure-session compose stack in [docker-compose.secure-sessions.yml](../docker-compose.secure-sessions.yml). It now configures `mcp-js` with:

- `JWKS_URL=http://keycloak:8080/realms/mcp/protocol/openid-connect/certs`
- the same OPA-backed fetch and filesystem policy chain used by the secure-session setup
- a dynamic fetch rule targeting `host.docker.internal`
- host gateway mapping so the container can call a mock upstream running on your machine

Start the environment:

```bash
docker compose -f docker-compose.secure-sessions.yml up --build -d
```

The helper waits for the imported Keycloak realm, OPA, and `mcp-js` before submitting `/api/exec`:

```bash
deno run -A scripts/test-keycloak-fetch-injection.ts
```

The script:

1. Starts a local protected/mock upstream on port `4010`.
2. Waits for OPA and `mcp-js` readiness.
3. Submits JavaScript to `mcp-js` through `/api/exec`.
4. Lets the secure-session fetch path pass through the repo's OPA allowlist for the exact local e2e target `http://host.docker.internal:4010/protected`.
5. Polls the async execution to completion, then reads the observable result back from `/api/executions/{id}/output`.
6. Parses the protected upstream payload from console output and verifies it received an `Authorization: Bearer ...` header.
7. Decodes the JWT and checks that it was issued by the local Keycloak realm.
8. Repeats the fetch and confirms the same token is reused before expiry.

Tear the stack down when finished:

```bash
docker compose -f docker-compose.secure-sessions.yml down -v
```

## Token Reuse, Refresh, And Reacquire

Dynamic OAuth rules are cached per fetch rule:

- Matching requests reuse the same token until it approaches expiry.
- If the token endpoint returns a `refresh_token`, mcp-js tries a refresh first after expiry.
- If refresh is unavailable or fails, mcp-js falls back to a fresh client-credentials token acquisition.

This means your JavaScript code keeps calling `fetch()` normally while the server handles token lifecycle in the background.

## Header Precedence

Injected headers never overwrite headers provided by user code. If your JavaScript explicitly sets `Authorization`, that value wins and the dynamic token source is skipped for that request.

```javascript
await fetch("https://api.example.com/data", {
  headers: {
    Authorization: "Bearer user-supplied-token",
  },
});
```

## Secret Handling Notes

- Prefer environment variables, mounted config files, or container secrets instead of hard-coding client secrets.
- Avoid committing real `client_secret` values to the repo.
- Be aware that inline CLI arguments can show up in shell history and process listings.
- The sandboxed JavaScript code does not get the token automatically, but the upstream service still receives it and can reflect it back, so only target services you trust.
