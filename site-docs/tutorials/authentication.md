# Authentication (JWT/JWKS)

In this tutorial we'll bring up the secure-sessions example stack, obtain a JWT from Keycloak, and connect an MCP client that presents the token on every initialize call.

## Prerequisites

- Docker and Docker Compose installed and running.
- Ports 3000 (mcp-v8) and 8080 (Keycloak) free on localhost.
- `curl` and `jq` available in your shell.

## 1. Start the secure-sessions stack

The repository ships a ready-made Compose file that starts Keycloak, OPA, and mcp-v8 wired together.

```bash
docker compose -f docker-compose.secure-sessions.yml up --build -d
```

The stack sets `JWKS_URL=http://keycloak:8080/realms/mcp/protocol/openid-connect/certs` inside the `mcp-js` container, which tells mcp-v8 to verify JWTs against Keycloak's public keys.

## 2. Wait for Keycloak to become healthy

Keycloak takes 20–40 seconds on first start. Poll until it responds:

```bash
until curl -sf http://localhost:8080/realms/mcp > /dev/null; do
  echo "waiting for Keycloak…"; sleep 3
done
echo "Keycloak ready"
```

## 3. Acquire a token from Keycloak

The `mcp-realm.json` imports a confidential client `mcp-client` with the client-credentials grant enabled. Fetch an access token:

```bash
TOKEN=$(curl -s \
  -X POST http://localhost:8080/realms/mcp/protocol/openid-connect/token \
  -d grant_type=client_credentials \
  -d client_id=mcp-client \
  -d client_secret=mcp-client-secret \
  | jq -r .access_token)

echo "${TOKEN:0:60}…"   # first 60 chars — should look like a JWT
```

Expected output: three base64url segments separated by dots.

## 4. Send an MCP initialize request with the token

mcp-v8 listens on `POST /mcp` for Streamable HTTP. The token is checked during the `initialize` call:

```bash
curl -s -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {"name": "tutorial-client", "version": "1.0"}
    }
  }' | jq .
```

The server returns an `InitializeResult`. Check the server log for the verification outcome:

```bash
docker compose -f docker-compose.secure-sessions.yml logs mcp-js | grep -E "JWT|verified"
```

You should see a line like `JWT verified`.

## 5. Try a forged token

Corrupt the last five characters of the token and repeat the request:

```bash
FORGED="${TOKEN::-5}XXXXX"

curl -s -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${FORGED}" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {"name": "forged-client", "version": "1.0"}
    }
  }' | jq .
```

The response is still an `InitializeResult` — in the current implementation verification outcome is logged, not enforced as a gate — but the log will show:

```
WARN JWT present but failed verification
```

## 6. Connect a full MCP client with Deno

For a complete end-to-end test using the bundled Deno script:

```bash
deno run -A scripts/test-keycloak-sessions.ts
```

This script acquires a real token, connects two sessions with different `X-MCP-Session-Id` headers, verifies heap isolation between them, and tests forged-token logging.

## 7. Tear down

```bash
docker compose -f docker-compose.secure-sessions.yml down
```

## See also

- [How-to: Authentication](../how-to/authentication.md)
- [Concepts: Authentication](../concepts/authentication.md)
- [Reference: Authentication](../reference/authentication.md)
- [Stateful sessions & heap snapshots](../tutorials/sessions-and-heaps.md)
- [Transports: stdio, HTTP, SSE](../tutorials/transports.md)
