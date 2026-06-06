# Authentication (JWT/JWKS)

These recipes cover enabling JWT/JWKS verification on mcp-v8 and presenting tokens from various clients. See the [reference page](../reference/authentication.md) for the complete flag and header specification.

## Enable JWKS verification

JWKS verification is optional. It activates only when `--jwks-url` is supplied (or the `JWKS_URL` environment variable is set). The flag accepts any URL that returns a JSON Web Key Set.

**CLI flag:**

```bash
mcp-v8 --http-port=3000 \
       --jwks-url=https://idp.example.com/.well-known/jwks.json
```

**Environment variable (useful in containers):**

```bash
export JWKS_URL=https://idp.example.com/.well-known/jwks.json
mcp-v8 --http-port=3000
```

The server fetches keys from the JWKS endpoint at startup. If a token arrives with an unknown `kid`, the keys are refreshed automatically from the same URL.

## Pass a token via Authorization: Bearer

Send the JWT as the value of the standard `Authorization` header on the `initialize` request. The `Bearer ` prefix (including the trailing space) is required; the server strips it before verification.

```bash
TOKEN="<your-jwt>"

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
      "clientInfo": {"name": "my-agent", "version": "1.0"}
    }
  }'
```

## Pass a token via agent-session header

As an alternative to `Authorization: Bearer`, supply the raw JWT in the `agent-session` header. No prefix is needed.

```bash
curl -s -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -H "agent-session: ${TOKEN}" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {"name": "my-agent", "version": "1.0"}
    }
  }'
```

If both `Authorization: Bearer` and `agent-session` are present, `Authorization: Bearer` takes precedence.

## Use Keycloak as the JWKS issuer

The repository ships a pre-built Keycloak realm at `keycloak/mcp-realm.json`. The realm is named `mcp` and contains a confidential client `mcp-client`.

**Start the stack:**

```bash
docker compose -f docker-compose.secure-sessions.yml up --build -d
```

**Configure mcp-v8 to trust Keycloak's keys:**

```bash
JWKS_URL=http://keycloak:8080/realms/mcp/protocol/openid-connect/certs
```

The `docker-compose.secure-sessions.yml` file sets this automatically via an environment variable inside the container.

**Acquire a client-credentials token:**

```bash
TOKEN=$(curl -s \
  -X POST http://localhost:8080/realms/mcp/protocol/openid-connect/token \
  -d grant_type=client_credentials \
  -d client_id=mcp-client \
  -d client_secret=mcp-client-secret \
  | jq -r .access_token)
```

**Use the token with an MCP client (Deno/TypeScript):**

```ts
import { Client } from "npm:@modelcontextprotocol/sdk@1.12.1/client/index.js";
import { StreamableHTTPClientTransport } from "npm:@modelcontextprotocol/sdk@1.12.1/client/streamableHttp.js";
import { ClientCredentialsProvider } from "./scripts/oauth-client-credentials-provider.ts";

const provider = new ClientCredentialsProvider({
  tokenUrl: "http://localhost:8080/realms/mcp/protocol/openid-connect/token",
  clientId: "mcp-client",
  clientSecret: "mcp-client-secret",
});

const transport = new StreamableHTTPClientTransport(
  new URL("http://localhost:3000/mcp"),
  {
    authProvider: provider,
    requestInit: {
      headers: { "X-MCP-Session-Id": "my-session" },
    },
  },
);

const client = new Client({ name: "my-agent", version: "1.0.0" });
await client.connect(transport);
```

The `ClientCredentialsProvider` fetches and caches tokens, injecting them as `Authorization: Bearer` on each request.

## Add a session ID alongside the token

The JWT authenticates the connection; it is separate from the session identifier. Send `X-MCP-Session-Id` as an additional header on the same `initialize` request. Both headers are read independently:

```bash
curl -s -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "X-MCP-Session-Id: my-agent-session" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {"name": "my-agent", "version": "1.0"}
    }
  }'
```

## See also

- [Quick-start: Authentication](../tutorials/authentication.md)
- [Concepts: Authentication](../concepts/authentication.md)
- [Reference: Authentication](../reference/authentication.md)
- [Stateful sessions & heap snapshots](../how-to/sessions-and-heaps.md)
- [Network access with fetch](../how-to/fetch.md)
- [Reference: CLI flags](../reference/cli-flags.md)
