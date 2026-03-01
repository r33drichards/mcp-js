# Testing OAuth Authentication with Keycloak

This guide explains how to test the MCP server's OAuth authentication locally using Keycloak as the identity provider.

## Architecture

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  MCP Client  │────>│   MCP-JS     │────>│  Keycloak    │
│ (Claude Code │     │  :3000       │     │  :8080       │
│  or curl)    │     │              │     │              │
│              │     │ validates    │     │ issues       │
│              │────>│ bearer token │     │ tokens       │
└──────────────┘     └──────────────┘     └──────────────┘
        │                                        ▲
        └────── OAuth flow (authorize/token) ────┘
```

- **Keycloak** is the OAuth authorization server (issues tokens)
- **MCP-JS** is the resource server (validates tokens via Keycloak introspection)
- **MCP clients** discover OAuth endpoints via `/.well-known/oauth-authorization-server` on the MCP server, then authenticate directly with Keycloak

## Quick Start

### 1. Start the stack

```bash
docker compose -f docker-compose.oauth.yml up --build
```

Wait for both services to be healthy. Keycloak takes ~30 seconds to start.

### 2. Verify Keycloak is running

Open http://localhost:8080 and log in with `admin` / `admin`.
The `mcp` realm is pre-configured with:

| Resource          | Value                |
|-------------------|----------------------|
| Realm             | `mcp`                |
| Public client     | `mcp-client`         |
| Server client     | `mcp-server`         |
| Server secret     | `mcp-server-secret`  |
| Test user         | `testuser`           |
| Test password     | `testpassword`       |

### 3. Verify MCP server OAuth discovery

```bash
curl -s http://localhost:3000/.well-known/oauth-authorization-server | jq .
```

Expected output shows Keycloak's endpoints:
```json
{
  "issuer": "http://keycloak:8080/realms/mcp",
  "authorization_endpoint": "http://keycloak:8080/realms/mcp/protocol/openid-connect/auth",
  "token_endpoint": "http://keycloak:8080/realms/mcp/protocol/openid-connect/token",
  ...
}
```

### 4. Verify unauthenticated requests are rejected

```bash
curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/mcp
# Expected: 401
```

## Testing with curl

### Get a token using the Resource Owner Password Grant

For testing purposes, you can use Keycloak's direct access grant (password flow):

```bash
# Get an access token
TOKEN=$(curl -s -X POST http://localhost:8080/realms/mcp/protocol/openid-connect/token \
  -d "grant_type=password" \
  -d "client_id=mcp-client" \
  -d "username=testuser" \
  -d "password=testpassword" \
  -d "scope=openid" | jq -r '.access_token')

echo "Token: $TOKEN"
```

### Use the token with the MCP server

```bash
# This should succeed (200)
curl -s -H "Authorization: Bearer $TOKEN" \
  http://localhost:3000/api/executions | jq .

# Execute JavaScript via the HTTP API
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"code": "1 + 1"}' \
  http://localhost:3000/api/exec | jq .
```

### Verify token introspection works

```bash
# Introspect the token directly with Keycloak (this is what MCP-JS does internally)
curl -s -X POST http://localhost:8080/realms/mcp/protocol/openid-connect/token/introspect \
  -u "mcp-server:mcp-server-secret" \
  -d "token=$TOKEN" | jq .
```

## Testing with Claude Code

### Option A: Use the MCP server's built-in OAuth (no Keycloak)

For simpler testing, you can run the MCP server with its built-in OAuth provider:

```bash
# Run locally (no Docker needed for the server)
cd server && cargo run -- --http-port 3000 --stateless --oauth --oauth-issuer http://localhost:3000
```

Then configure Claude Code's MCP settings (`.cursor/mcp.json` or equivalent):

```json
{
  "mcpServers": {
    "mcp-js": {
      "url": "http://localhost:3000/mcp"
    }
  }
}
```

Claude Code will discover the OAuth endpoints and handle the authorization flow automatically.

### Option B: Use Keycloak (full stack)

1. Start the Docker stack:
   ```bash
   docker compose -f docker-compose.oauth.yml up --build
   ```

2. The MCP server at `http://localhost:3000` will advertise Keycloak's OAuth endpoints.
   Configure Claude Code to connect to `http://localhost:3000/mcp`.

3. When Claude Code discovers the OAuth requirement, it will redirect you to
   Keycloak's login page. Log in with `testuser` / `testpassword`.

## Pre-configured Keycloak Clients

### mcp-client (public)

Used by MCP client applications (Claude Code, MCP Inspector, curl, etc.).

- **Client ID**: `mcp-client`
- **Type**: Public (no secret required)
- **Flows**: Authorization Code, Direct Access Grant (password)
- **Redirect URIs**: `http://localhost:*`, `http://127.0.0.1:*`

### mcp-server (confidential)

Used by the MCP server internally to introspect tokens.

- **Client ID**: `mcp-server`
- **Client Secret**: `mcp-server-secret`
- **Type**: Confidential (service account)
- **Purpose**: Token introspection only

## Troubleshooting

### "401 Unauthorized" when using a token

1. Check that the token hasn't expired (default: 1 hour)
2. Verify Keycloak is reachable from the MCP server container:
   ```bash
   docker compose -f docker-compose.oauth.yml exec mcp-js \
     curl -s http://keycloak:8080/realms/mcp/.well-known/openid-configuration | head -5
   ```
3. Check MCP server logs for introspection errors:
   ```bash
   docker compose -f docker-compose.oauth.yml logs mcp-js
   ```

### Keycloak not starting

Keycloak needs ~30 seconds to initialize. Check its health:
```bash
docker compose -f docker-compose.oauth.yml ps
docker compose -f docker-compose.oauth.yml logs keycloak
```

### Realm not imported

If the `mcp` realm doesn't appear in Keycloak, verify the volume mount:
```bash
docker compose -f docker-compose.oauth.yml exec keycloak ls -la /opt/keycloak/data/import/
```
