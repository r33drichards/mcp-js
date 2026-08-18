# Running the Modal SDK (gRPC over node:http2)

[Modal](https://modal.com) is a serverless cloud platform (containers, GPUs,
sandboxes). Its JavaScript SDK talks to `api.modal.com` over gRPC, which rides
HTTP/2. Because mcp-v8 ships a policy-gated [`node:http2`](http2.md)
transport, the **stock `modal` npm package runs unmodified inside the
sandbox** — no proxy, no hand-rolled channel — and its credentials are injected
server-side so they never enter the JavaScript isolate.

This guide is the end-to-end recipe: the server flags, the policy, credential
injection, and the JS to drive it.

## What the SDK needs from the server

Four capabilities, all off by default, must be enabled:

1. **External module imports** (`--allow-external-modules`) — to `import` the
   `modal` package (and its deps) from esm.sh. See [ES module
   imports](module-imports.md).
2. **An `http2` policy** allowing `api.modal.com` — gRPC connects go through the
   HTTP/2 transport, which is gated. See [HTTP/2 sessions](http2.md).
3. **Header injection** of the Modal token as gRPC metadata — so the sandbox
   authenticates without ever holding the secret.
4. **WebAssembly** — the SDK's `uuid` dependency and other pieces need
   `node:crypto` and a live `WebAssembly` global. WebAssembly is present in the
   normal runtime but **disabled under heap persistence** (a V8
   `SnapshotCreator` isolate turns it off), so run with `--heap-store none`
   (the default). Use [filesystem persistence](fs-snapshots.md) if you need
   per-session state.

## Server configuration

`http2.rego` — allow only Modal's API host:

```rego
package mcp.http2

default allow = false

allow if {
    input.operation == "connect"
    input.url_parsed.host == "api.modal.com"
}

allow if {
    input.operation == "request"
    input.authority == "api.modal.com"
}
```

Wire it up, inject the token as gRPC metadata, and allow module imports:

```bash
mcp-v8 \
  --allow-external-modules \
  --policies-json '{"http2":{"policies":[{"url":"file:///path/to/http2.rego"}]}}' \
  --fetch-header "host=api.modal.com,header=x-modal-token-id,value=ak-..." \
  --fetch-header "host=api.modal.com,header=x-modal-token-secret,value=as-..."
```

Environment-variable equivalents (e.g. for a container deployment):

```
MCP_V8_ALLOW_EXTERNAL_MODULES=true
MCP_V8_POLICIES_JSON={"http2":{"policies":[{"url":"file:///path/to/http2.rego"}]}}
MCP_V8_FETCH_HEADER_CONFIG=[
  {"host":"api.modal.com","headers":{
    "x-modal-token-id":"ak-...",
    "x-modal-token-secret":"as-..."}}
]
MCP_V8_HEAP_STORE=none
```

Header-injection rules are host-scoped, so the token only ever travels to
`api.modal.com`, and sandboxed code cannot read the injected values — there is
no request-header read-back API. Create a Modal token pair in your
[workspace settings](https://modal.com/docs/guide).

## Use it from JavaScript

```js
import { ModalClient } from 'npm:modal?target=node';
import { Buffer } from 'node:buffer';
import process from 'node:process';

// Packages built for Node expect these as globals.
globalThis.Buffer = Buffer;
globalThis.process = process;

// Real credentials are injected server-side for api.modal.com, so the client
// only needs a placeholder secret — the injected value overrides it at the
// gRPC metadata layer (header injection replaces same-named headers).
const modal = new ModalClient({
  tokenId: 'ak-...',              // public token id
  tokenSecret: 'placeholder',     // overridden server-side
});

// Call a deployed Function:
const fn = await modal.functions.fromName('my-app', 'my-fn');
console.log(JSON.stringify(await fn.remote(['hello'])));

// …or create a Sandbox:
const app = await modal.apps.fromName('sandbox-app', { createIfMissing: true });
const image = modal.images.fromRegistry('alpine:3.21');
const sb = await modal.sandboxes.create(app, image, { command: ['cat'] });
await sb.stdin.writeText('hi'); await sb.stdin.close();
console.log(await sb.stdout.readText());
await sb.terminate();
```

The `?target=node` suffix selects the Node build (which imports the `node:*`
builtins mcp-v8 serves) rather than the browser build.

The JS SDK's scope is creating/driving **Sandboxes** and calling **deployed
Functions/Classes** (Functions are defined in Python elsewhere). See the
[Modal JS examples](https://github.com/modal-labs/modal-client/tree/main/js/examples).

## Troubleshooting

| Symptom | Cause |
|---|---|
| `Unknown node builtin module: 'crypto'` | Old build without `node:crypto`; update mcp-v8. |
| `WebAssembly is not an object` | Heap persistence is on. Set `--heap-store none`. |
| gRPC `connect` rejected / capability disabled | No `http2` policy, or it doesn't allow `api.modal.com`. |
| `UNAUTHENTICATED` from Modal | Header-injection rule missing/misspelled, or token invalid. Injected header names must be exactly `x-modal-token-id` / `x-modal-token-secret`. |
| Import fails | `--allow-external-modules` not set, or egress to esm.sh blocked by the OS sandbox / network policy. |

## See also

- [HTTP/2 sessions (node:http2)](http2.md) — the transport, per-stream policy, and header injection in depth.
- [Running stock @grpc/grpc-js](http2.md#run-a-stock-grpc-client) — the same mechanism for any gRPC SDK.
- [ES module imports](module-imports.md) and [Security policies](policies.md).
