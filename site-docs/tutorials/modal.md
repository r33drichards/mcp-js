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

## Why this needs four things turned on

Everything the SDK touches is a capability that `mcp-v8` keeps **off by
default**. Turning them on one at a time makes the failure modes legible:

1. **External module imports** — to `import` the `modal` package from esm.sh.
   Without it, the import throws immediately.
2. **An `http2` policy allowing `api.modal.com`** — every gRPC connection goes
   through the gated HTTP/2 transport. With no policy, the connect is refused.
3. **Header injection of your Modal token** — gRPC metadata is just HTTP/2
   headers, so `mcp-v8` can attach the token at the transport layer. The sandbox
   authenticates without ever holding the secret.
4. **WebAssembly** — the SDK depends on `uuid`, which needs `node:crypto` and a
   live `WebAssembly` global. WebAssembly is present in the normal runtime, but
   a V8 `SnapshotCreator` isolate disables it — so **heap persistence must be
   off** (`--heap-store none`, the default). If you need per-session state, use
   [filesystem persistence](../how-to/fs-snapshots.md) instead, which doesn't
   disable WebAssembly.

## Step 1 — Write the policy

The HTTP/2 transport asks a policy before dialing anywhere. Scope it to Modal's
API host so the sandbox can reach Modal and nothing else. Save this as
`http2.rego`:

```rego
package mcp.http2

default allow = false

# Which authorities may be dialed.
allow if {
    input.operation == "connect"
    input.url_parsed.host == "api.modal.com"
}

# Which streams (per-RPC) may open on an allowed session.
allow if {
    input.operation == "request"
    input.authority == "api.modal.com"
}
```

## Step 2 — Start the server with the four capabilities

```bash
mcp-v8 \
  --allow-external-modules \
  --heap-store none \
  --policies-json '{"http2":{"policies":[{"url":"file:///path/to/http2.rego"}]}}' \
  --fetch-header "host=api.modal.com,header=x-modal-token-id,value=ak-..." \
  --fetch-header "host=api.modal.com,header=x-modal-token-secret,value=as-..."
```

The two `--fetch-header` rules are the trick that keeps the secret out of the
isolate: they're **host-scoped**, so the token only ever travels to
`api.modal.com`, and there is no request-header read-back API — sandboxed code
can authenticate but can never read the injected values.

For a container or Kubernetes deployment, the same settings are environment
variables:

```
MCP_V8_ALLOW_EXTERNAL_MODULES=true
MCP_V8_HEAP_STORE=none
MCP_V8_POLICIES_JSON={"http2":{"policies":[{"url":"file:///path/to/http2.rego"}]}}
MCP_V8_FETCH_HEADER_CONFIG=[
  {"host":"api.modal.com","headers":{
    "x-modal-token-id":"ak-...",
    "x-modal-token-secret":"as-..."}}
]
```

## Step 3 — Call Modal from the sandbox

Here's the script. Note it constructs the client with a **placeholder secret** —
the real one is injected server-side, and header injection replaces the
same-named header the SDK sets, so the placeholder never reaches Modal.

```js
import { ModalClient } from 'npm:modal?target=node';
import { Buffer } from 'node:buffer';
import process from 'node:process';

// Packages built for Node expect these as globals.
globalThis.Buffer = Buffer;
globalThis.process = process;

const modal = new ModalClient({
  tokenId: 'ak-...',            // your public token id
  tokenSecret: 'placeholder',   // overridden server-side by header injection
});

// Call a deployed Function and print its result:
const fn = await modal.functions.fromName('my-app', 'my-fn');
console.log(JSON.stringify(await fn.remote(['hello'])));
```

The `?target=node` suffix matters: it selects the SDK's Node build, which
imports the `node:*` builtins `mcp-v8` serves, rather than the browser build.

Run it through the sandbox:

```bash
curl -sX POST http://localhost:8080/api/exec \
  -H 'Content-Type: application/javascript' \
  --data-binary @modal-call.js | jq -r '.output'
```

You should see your Function's return value. That round trip — JS in the
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
server launches:

```sh
sh -c 'mkdir -p /policies && cat > /policies/http2.rego <<EOF
package mcp.http2
default allow = false
allow if { input.operation == "connect"; input.url_parsed.host == "api.modal.com" }
allow if { input.operation == "request"; input.authority == "api.modal.com" }
EOF
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
MCP_V8_POLICIES_JSON={"http2":{"policies":[{"url":"file:///policies/http2.rego"}]}}
MCP_V8_FETCH_HEADER_CONFIG=[{"host":"api.modal.com","headers":{"x-modal-token-id":"ak-...","x-modal-token-secret":"as-..."}}]
MCP_V8_ALLOWED_HOSTS=${{RAILWAY_PUBLIC_DOMAIN}}
```

Generate a public domain (target port `8080`) and the server is reachable at
`https://<your-domain>/mcp`. Your Modal token now lives only in a Railway
variable — never in the JavaScript an agent submits.

## Connect Claude Code, with authentication

A public `/mcp` endpoint that runs arbitrary JavaScript **must** be
authenticated. mcp-v8 verifies JWT bearer tokens against a JWKS endpoint; set
`JWKS_URL` to your identity provider's key set (e.g. a Keycloak realm's certs
URL) to require it:

```env
JWKS_URL=https://idp.example.com/realms/mcp/protocol/openid-connect/certs
```

With that set, every request to `/mcp` must carry a valid
`Authorization: Bearer <jwt>` (see [Authentication](../how-to/authentication.md)
for minting and presenting tokens). Register the deployment with Claude Code,
passing the token as a header:

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

## When something goes wrong

| Symptom | Cause |
|---|---|
| `Unknown node builtin module: 'crypto'` | Old build without `node:crypto`; update mcp-v8. |
| `WebAssembly is not an object` | Heap persistence is on. Set `--heap-store none`. |
| gRPC `connect` rejected / capability disabled | No `http2` policy, or it doesn't allow `api.modal.com`. |
| `UNAUTHENTICATED` from Modal | Header-injection rule missing/misspelled, or token invalid. The header names must be exactly `x-modal-token-id` / `x-modal-token-secret`. |
| Import fails | `--allow-external-modules` not set, or egress to esm.sh blocked by the OS sandbox or network policy. |

## See also

- [HTTP/2 sessions (node:http2)](../how-to/http2.md) — the transport, per-stream
  policy, and header injection in depth.
- [Running stock @grpc/grpc-js](../how-to/http2.md#run-a-stock-grpc-client) —
  the same mechanism for any gRPC SDK.
- [ES module imports](../how-to/module-imports.md) and
  [Security policies](../how-to/policies.md).
