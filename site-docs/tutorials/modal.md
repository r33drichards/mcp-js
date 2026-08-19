# Call Modal (serverless GPUs and sandboxes)

In this tutorial you'll drive [Modal](https://modal.com) — serverless
containers, GPUs, and sandboxes — from JavaScript running inside an `mcp-v8`
isolate. By the end, code in the sandbox will call a deployed Modal Function
and get a result back, using the **stock `modal` npm package, unmodified**.

The interesting part is *how* it works. Modal's SDK talks to `api.modal.com`
over gRPC, which rides HTTP/2 — a protocol the sandbox has no raw sockets for.
It works anyway because `mcp-v8` ships a policy-gated
[`node:http2`](../how-to/http2.md) transport, and the SDK's credentials are
injected server-side so they never enter the isolate. You'll assemble those
pieces one at a time and see why each is needed.

## Prerequisites

- `mcp-v8` installed (see [Install](../install/overview.md)).
- `curl` and `jq`.
- A Modal account and an API token pair (a token id `ak-…` and secret `as-…`),
  created in your Modal workspace settings. Modal is a cloud platform — there is
  no offline mode; the calls really hit `api.modal.com`.
- **A deployed Modal Function to call.** The JS SDK invokes Functions that are
  *defined in Python* and already deployed — it does not define them. If you
  don't have one, deploy this minimal example first (with the Python `modal`
  CLI, `pip install modal && modal setup`):

  ```python
  # echo.py
  import modal

  app = modal.App("my-app")

  @app.function()
  def my_fn(name: str) -> str:
      return f"hello {name}"
  ```

  ```bash
  modal deploy echo.py
  ```

  That publishes Function `my-fn` in app `my-app` — the names Step 3 looks up.

## Why this needs five settings

The SDK needs four capabilities plus Node compatibility globals that `mcp-v8`
keeps **off by default**. Turning them on one at a time makes the failure modes
legible:

1. **External module imports** — to `import` the `modal` package from esm.sh.
   Without it, the import throws immediately.
2. **Node compatibility globals** — the SDK and its dependencies expect
   `Buffer` and `process` during module evaluation. `--node-globals` installs
   the sandboxed compatibility values without exposing host process state.
3. **An `http2` policy allowing `*.modal.com`** — every gRPC connection goes
   through the gated HTTP/2 transport. With no policy, the connect is refused.
4. **Header injection of your Modal token** — gRPC metadata is just HTTP/2
   headers, so `mcp-v8` can attach the token at the transport layer. Injection
   overwrites the same-named header the SDK sets, so the sandbox authenticates
   without ever holding the real secret (it passes a placeholder).
5. **WebAssembly** — the SDK's dependency tree needs a live `WebAssembly`
   global. WebAssembly is present in the normal runtime, but a V8
   `SnapshotCreator` isolate disables it — so **heap persistence must be off**
   (`--heap-store none`, the default). If you need per-session state, use
   [filesystem persistence](../how-to/fs-snapshots.md) instead, which doesn't
   disable WebAssembly.

## Step 1 — Write the policy

The HTTP/2 transport asks a policy before dialing anywhere. Scope it to Modal so
the sandbox can reach Modal and nothing else. Match any `*.modal.com` host, not
just `api.modal.com`: the SDK's control plane can hand back a separate
input-plane host (also under `modal.com`) for some function calls, and a policy
pinned to `api.modal.com` would deny that second connection. Save this as
`http2.rego`:

```rego
package mcp.http2

default allow = false

# Which authorities may be dialed (api.modal.com plus any input-plane host).
allow if {
    input.operation == "connect"
    endswith(input.url_parsed.host, ".modal.com")
}
allow if {
    input.operation == "connect"
    input.url_parsed.host == "modal.com"
}

# Which streams (per-RPC) may open on an allowed session.
allow if {
    input.operation == "request"
    endswith(input.authority, ".modal.com")
}
allow if {
    input.operation == "request"
    input.authority == "modal.com"
}
```

## Step 2 — Start the server with the required settings

```bash
mcp-v8 \
  --http-port 8080 \
  --allow-external-modules \
  --node-globals \
  --heap-store none \
  --policies-json '{"http2":{"policies":[{"url":"file:///path/to/http2.rego"}]}}' \
  --fetch-header "host=*.modal.com,header=x-modal-token-id,value=ak-..." \
  --fetch-header "host=*.modal.com,header=x-modal-token-secret,value=as-..."
```

`--http-port 8080` is required: with no port flag mcp-v8 serves the stdio
transport, and Step 3's `curl http://localhost:8080/...` would get connection
refused. `--node-globals` installs the sandboxed `Buffer` and `process`
compatibility values before the Modal module graph is evaluated; it does not
expose host environment variables or grant additional capabilities. The two
`--fetch-header` rules are the trick that keeps the secret out
of the isolate: they're **host-scoped** to `*.modal.com` (matching the policy
above), so the token only ever travels to Modal, and there is no request-header
read-back API — sandboxed code can authenticate but can never read the injected
values. Injection **overwrites** the same-named header the SDK sets, so the
placeholder the script passes (Step 3) is replaced by the real token before the
request leaves the host.

For a container or Kubernetes deployment, the same settings are environment
variables (the JSON must be a single line):

```
MCP_V8_HTTP_PORT=8080
MCP_V8_ALLOW_EXTERNAL_MODULES=true
MCP_V8_NODE_GLOBALS=true
MCP_V8_HEAP_STORE=none
MCP_V8_POLICIES_JSON={"http2":{"policies":[{"url":"file:///path/to/http2.rego"}]}}
MCP_V8_FETCH_HEADER_CONFIG=[{"host":"*.modal.com","headers":{"x-modal-token-id":"ak-...","x-modal-token-secret":"as-..."}}]
```

## Step 3 — Call Modal from the sandbox

Here's the script. Note it constructs the client with a **placeholder secret** —
the real one is injected server-side, and header injection replaces the
same-named header the SDK sets, so the placeholder never reaches Modal.

```js
import { ModalClient } from 'npm:modal?target=node&bundle';

const modal = new ModalClient({
  tokenId: 'ak-...',            // your public token id
  tokenSecret: 'placeholder',   // overridden server-side by header injection
});

// Call the deployed Function and print its result:
const fn = await modal.functions.fromName('my-app', 'my-fn');
console.log(JSON.stringify(await fn.remote(['world'])));
```

The `target=node` query selects the SDK's Node build, which imports the
`node:*` builtins `mcp-v8` serves. The `bundle` query keeps optional native
detection dependencies out of the fetched module graph; without it, the current
SDK graph imports the unsupported `node:child_process` builtin before the
client is created.

Run it through the sandbox. `/api/exec` is **asynchronous**: it returns `202`
with an `execution_id`, and you read the result from the execution's output
endpoint — there is no synchronous `.output` field.

```bash
# Save the script above as modal-call.js, then submit it.
EXEC_ID=$(curl -sX POST http://localhost:8080/api/exec \
  -H 'Content-Type: application/javascript' \
  --data-binary @modal-call.js | jq -r '.execution_id')

# Poll until the execution reaches a terminal state.
while :; do
  STATUS=$(curl -s "http://localhost:8080/api/executions/$EXEC_ID" | jq -r '.status')
  case "$STATUS" in
    Completed) break ;;
    Failed|TimedOut|Cancelled) echo "execution $STATUS"; break ;;
    *) sleep 1 ;;
  esac
done

# Read the console output (your Function's return value is here).
curl -s "http://localhost:8080/api/executions/$EXEC_ID/output" | jq -r '.data'
```

You should see your Function's return value (`"hello world"`). That round trip — JS in the
isolate → `node:http2` → gRPC → `api.modal.com` → back — is the whole point:
an unmodified cloud SDK, talking to its backend over a protocol the sandbox
implements through host-side ops, authenticated by a credential the isolate
never saw.

## Going further: Sandboxes

The same setup drives Modal Sandboxes — spin up a container, stream to its
stdin, read its stdout:

```js
const app = await modal.apps.fromName('sandbox-app', { createIfMissing: true });
const image = modal.images.fromRegistry('alpine:3.21');
const sb = await modal.sandboxes.create(app, image, { command: ['cat'] });
await sb.stdin.writeText('hi there'); await sb.stdin.close();
console.log(await sb.stdout.readText());
await sb.terminate();
```

Sandbox creation takes many more options (`secrets`, `timeoutMs`, `cpu`,
`memoryMiB`, GPUs, volumes, tunnels). The JS SDK's scope is creating and
driving Sandboxes and calling deployed Functions/Classes — Functions themselves
are defined in Python. The
[Modal JS examples](https://github.com/modal-labs/modal-client/tree/main/js/examples)
cover each of these.

## Deploy it to Railway

Running this on [Railway](https://railway.com) gives you a hosted, always-on
sandbox that an agent elsewhere can call. The fastest start is the one-click
[**Deploy on Railway** template](https://railway.com/deploy/mcp-js), which
provisions the server with a volume and the standard variables; the
[`RAILWAY.md` guide](https://github.com/r33drichards/mcp-js/blob/main/RAILWAY.md)
in the repo documents every variable. Then apply two Modal-specific changes.

First, the policy file has no place on an ephemeral container, so write it from
the **start command** (Settings → Deploy → Custom Start Command) before the
server launches. Write it to `/tmp` — the image runs as a non-root user that
can't create files under `/`, and the OS sandbox still grants read access to a
`file://` policy path:

```sh
sh -c 'printf %s "package mcp.http2
default allow = false
allow if { input.operation == \"connect\"; endswith(input.url_parsed.host, \".modal.com\") }
allow if { input.operation == \"request\"; endswith(input.authority, \".modal.com\") }
" > /tmp/http2.rego
exec mcp-v8'
```

Second, set the Modal variables alongside the standard ones — and note
`MCP_V8_HEAP_STORE=none` (WebAssembly), which replaces the `dir` value the base
guide uses; keep `MCP_V8_FS_STORE=dir` for per-session filesystem state:

```env
MCP_V8_HEAP_STORE=none
MCP_V8_FS_STORE=dir
MCP_V8_FS_DIR=/data/fs
MCP_V8_ALLOW_EXTERNAL_MODULES=true
MCP_V8_NODE_GLOBALS=true
MCP_V8_POLICIES_JSON={"http2":{"policies":[{"url":"file:///tmp/http2.rego"}]}}
MCP_V8_FETCH_HEADER_CONFIG=[{"host":"*.modal.com","headers":{"x-modal-token-id":"ak-...","x-modal-token-secret":"as-..."}}]
MCP_V8_ALLOWED_HOSTS=${{RAILWAY_PUBLIC_DOMAIN}},${{RAILWAY_PRIVATE_DOMAIN}}
```

Keep both domains in `MCP_V8_ALLOWED_HOSTS`: the public one for external agents,
the private one so other Railway services can reach it over the internal
network. **Generate the public domain first** (Settings → Networking → target
port `8080`) — until it exists, `${{RAILWAY_PUBLIC_DOMAIN}}` expands empty and
mcp-v8 falls back to loopback-only, 403-ing every request; redeploy after
generating it if you set the variable first. The server is then reachable at
`https://<your-domain>/mcp`, and your Modal token lives only in a Railway
variable — never in the JavaScript an agent submits.

## Provision an identity provider (Keycloak on Railway)

A public `/mcp` endpoint that runs arbitrary JavaScript **must** be
authenticated. mcp-v8 verifies JWT bearer tokens against a JWKS endpoint, so you
need something that issues signed tokens. This repo ships a ready-made Keycloak
realm — `keycloak/mcp-realm.json`, realm `mcp` with a confidential client
`mcp-client` — so you can stand up an issuer in one more service instead of
wiring an IdP by hand.

Add a second Railway service for Keycloak. The realm is **declarative**:
importing it on every boot recreates the client and its secret identically, so
Keycloak's dev mode (ephemeral H2 storage) is enough here — a redeploy re-imports
the same realm, and while its **signing keys rotate on each redeploy** (so tokens
must be re-minted after one), the client id and secret stay stable.

The official Keycloak image has `curl` and package managers **removed**, so you
can't fetch the realm from a start command. Instead deploy from a tiny
Dockerfile that bakes the realm in at build time (Docker's `ADD` fetches the URL
during the build, where the network is available):

```dockerfile
FROM quay.io/keycloak/keycloak:26.4
ADD --chmod=444 \
  https://raw.githubusercontent.com/r33drichards/mcp-js/main/keycloak/mcp-realm.json \
  /opt/keycloak/data/import/mcp-realm.json
CMD ["start-dev", "--import-realm", "--http-port=8080"]
```

Put that Dockerfile in a repo (or a subdirectory) and point the Railway service
at it. Set these variables so Keycloak trusts Railway's TLS-terminating proxy and
can bootstrap an admin user:

```env
KC_PROXY_HEADERS=xforwarded
KC_HTTP_ENABLED=true
KC_HOSTNAME_STRICT=false
KC_BOOTSTRAP_ADMIN_USERNAME=admin
KC_BOOTSTRAP_ADMIN_PASSWORD=<pick-a-strong-password>
```

Generate a public domain for the service (target port `8080`). Keycloak is now
serving the realm's JWKS at
`https://<keycloak-domain>/realms/mcp/protocol/openid-connect/certs`.

> **Not production-hardened as written.** Dev mode stores nothing durably and
> the client secret is public in the repo. For real use, run Keycloak in
> production mode against a Postgres database (Railway provisions one in a
> click), rotate `mcp-client`'s secret, and lengthen or shorten the access-token
> lifespan to taste. The declarative realm is the starting point, not the final
> config.

## Require auth on the sandbox and connect Claude Code

Point mcp-v8 at Keycloak's key set by adding one variable to the **mcp-js**
service (from the [Deploy it to Railway](#deploy-it-to-railway) step):

```env
JWKS_URL=https://<keycloak-domain>/realms/mcp/protocol/openid-connect/certs
```

> **Bring Keycloak up first.** mcp-v8 fetches the JWKS at startup and **exits if
> the endpoint is unreachable**, so confirm Keycloak is serving before you set
> this. A quick check: `curl -sf https://<keycloak-domain>/realms/mcp/protocol/openid-connect/certs`
> should return a JSON key set. Set `JWKS_URL` (and redeploy mcp-js) only after
> that succeeds.

With that set, mcp-v8 **enforces** auth: every request to `/mcp` *and* the HTTP
API (`/api/exec`, `/api/fs/*`) must carry a valid `Authorization: Bearer <jwt>`,
or it is rejected with `401`. (Without `JWKS_URL` the server requires no token —
so don't expose it publicly until this is set.) Mint a token with the
client-credentials grant — no browser, no user, just the client id and secret
from the realm:

```bash
TOKEN=$(curl -s \
  -X POST https://<keycloak-domain>/realms/mcp/protocol/openid-connect/token \
  -d grant_type=client_credentials \
  -d client_id=mcp-client \
  -d client_secret=mcp-client-secret \
  | jq -r .access_token)
```

Register the deployment with Claude Code, passing that token as a header (see
[Authentication](../how-to/authentication.md) for other ways to present it):

```bash
claude mcp add --transport http modal-sandbox \
  https://<your-domain>/mcp \
  --header "Authorization: Bearer ${TOKEN}"
```

Claude Code now sees `run_js` (and any [upstream MCP tools](../how-to/mcp-client.md)
you've bridged) as callable tools, and can drive Modal through the sandbox on
your behalf — GPUs, Sandboxes, and deployed Functions — with the credential
boundary intact end to end: the agent holds a short-lived JWT to reach the
sandbox, and the sandbox holds nothing; the Modal token is injected host-side
and never crosses into the isolate or back to the agent.

Access tokens are short-lived (five minutes by default), so a header captured
once will expire — re-run `claude mcp add` with a fresh `${TOKEN}` when it
lapses. Raising the lifespan in the Keycloak admin console **won't stick** in the
dev-mode setup above: the next redeploy re-imports the declarative realm and
resets it. To change it durably, set `accessTokenLifespan` in the realm JSON, or
run Keycloak in production mode against a database.

## When something goes wrong

| Symptom | Cause |
|---|---|
| `Unknown node builtin module: 'crypto'` | Old build without `node:crypto`; update mcp-v8. |
| `WebAssembly is not an object` | Heap persistence is on. Set `--heap-store none`. |
| gRPC `connect` rejected / capability disabled | No `http2` policy, or it doesn't allow the host. Scope it to `*.modal.com`, not just `api.modal.com` — some calls dial a separate input-plane host. |
| A call to `api.modal.com` works but another gRPC connect is denied | The policy (and the `--fetch-header` host) is pinned to `api.modal.com`; widen both to `*.modal.com` for the input-plane host. |
| `UNAUTHENTICATED` from Modal | Header-injection rule missing/misspelled, or token invalid. The header names must be exactly `x-modal-token-id` / `x-modal-token-secret`, and the rule host must match the request (`*.modal.com`). |
| `process is not defined` or `Buffer is not defined` | `--node-globals` / `MCP_V8_NODE_GLOBALS=true` is missing. |
| `Unknown node builtin module: 'child_process'` | Import the bundled SDK build with `npm:modal?target=node&bundle`. |
| Import fails | `--allow-external-modules` not set, or egress to esm.sh blocked by the OS sandbox or network policy. |
| `401`/`403` from `/mcp` or `/api/*` | Missing or expired bearer token, or `JWKS_URL` doesn't point at the realm's `.../protocol/openid-connect/certs`. Mint a fresh token. |
| Keycloak token request returns `invalid_client` | Wrong `client_id`/`client_secret`, or the realm didn't import — check the service logs for the `Imported realm mcp` line. |

## See also

- [HTTP/2 sessions (node:http2)](../how-to/http2.md) — the transport, per-stream
  policy, and header injection in depth.
- [Running stock @grpc/grpc-js](../how-to/http2.md#run-a-stock-grpc-client) —
  the same mechanism for any gRPC SDK.
- [ES module imports](../how-to/module-imports.md) and
  [Security policies](../how-to/policies.md).
