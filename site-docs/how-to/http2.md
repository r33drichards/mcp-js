# HTTP/2 sessions (node:http2)

The `node:http2` module gives sandboxed code a client-side HTTP/2 API — the
transport gRPC clients are built on — without raw sockets. Sessions and
streams are backed by host-side ops, so per-stream policy and server-side
header injection keep working even though the protocol is not HTTP/1-shaped.

## Enable node:http2 with a policy

Like fetch and WebSocket, the capability is off by default. Add an `http2`
section to `--policies-json`:

```rego
package mcp.http2

default allow = false

# Sessions: gate which authorities can be dialed.
allow if {
    input.operation == "connect"
    input.url_parsed.host == "api.modal.com"
}

# Streams: gate individual calls — for gRPC, input.path is the RPC method.
allow if {
    input.operation == "request"
    input.authority == "api.modal.com"
    startswith(input.path, "/modal.client.ModalClient/")
}
```

```json
{ "http2": { "policies": [ { "url": "file:///path/to/http2.rego" } ] } }
```

The `connect` input is `{operation, url, url_parsed}`; the per-stream
`request` input is `{operation, scheme, authority, method, path, headers}` —
evaluated after header injection, so policies can also assert on injected
metadata.

## Use it from JS

```js
import http2 from 'node:http2';
import { Buffer } from 'node:buffer';

const session = http2.connect('https://api.example.com');
const stream = session.request({
  ':method': 'POST',
  ':path': '/pkg.Service/Method',
  'content-type': 'application/grpc',
  'te': 'trailers',
});
stream.on('response', (headers) => console.log(headers[':status']));
stream.on('data', (chunk) => { /* Buffer */ });
stream.on('trailers', (trailers) => console.log(trailers['grpc-status']));
stream.write(buffer);
stream.end();
```

`https` authorities negotiate TLS with ALPN `h2`; plaintext `http`
authorities use h2c prior knowledge (the standard insecure-gRPC
arrangement). Only the client subset is implemented — `createServer` throws
— and `options.createConnection`/custom sockets are deliberately ignored:
the host owns the transport; that is the security model.

## Inject gRPC credentials server-side

gRPC metadata is plain HTTP/2 headers, so `--fetch-header` /
`--fetch-header-config` rules apply per stream for matching hosts:

```bash
--fetch-header "host=api.modal.com,header=x-modal-token-id,value=ak-..." \
--fetch-header "host=api.modal.com,header=x-modal-token-secret,value=as-..."
```

Sandboxed code can make authenticated calls but can never read the injected
values — there is no request-header read-back API, and the rules are
host-scoped so the credentials only ever travel to the configured authority.

## See also

- [How-to: Network access with fetch](fetch.md) — same policy and
  header-injection model, for HTTP/1 request/response.
- [How-to: WebSocket connections](websocket.md)
- `docs/node-http2-grpc-plan.md` in the repository — the roadmap to running
  stock `@grpc/grpc-js` unmodified on this transport.
