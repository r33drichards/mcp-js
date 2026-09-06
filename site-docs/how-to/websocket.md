# WebSocket connections

Recipes for enabling the `WebSocket` global, restricting which endpoints are
reachable, and injecting authentication headers into the handshake
server-side.

## Enable WebSocket with a host allowlist

The WHATWG `WebSocket` class is available in JS only when the server is
started with a `--policies-json` configuration that includes a `websocket`
section. Without it, the global does not exist.

Create a policy that allows only specific hosts (`websocket.rego`):

```rego
package mcp.websocket

default allow = false

allow if {
    input.url_parsed.host == "stream.example.com"
}
```

Create the policies configuration (`policies.json`):

```json
{
  "websocket": {
    "policies": [
      { "url": "file:///path/to/websocket.rego" }
    ]
  }
}
```

Start the server:

```bash
mcp-v8 --http-port=8080 --policies-json=/path/to/policies.json
```

The policy is evaluated once per connection, at handshake time, with input:

```json
{
  "operation": "connect",
  "url": "wss://stream.example.com/feed",
  "protocols": ["graphql-ws"],
  "headers": { "x-api-key": "..." },
  "url_parsed": { "scheme": "wss", "host": "stream.example.com", "port": null, "path": "/feed", "query": "" }
}
```

A denied connect surfaces in JS the way a network failure does: an `error`
event followed by a `close` event with `wasClean: false` and code 1006.

An empty `policies` list allows every connection:

```json
{ "websocket": { "policies": [] } }
```

## Use it from JS

The class follows the browser API:

```js
const ws = new WebSocket("wss://stream.example.com/feed", ["graphql-ws"]);
ws.binaryType = "arraybuffer";
ws.onmessage = (event) => console.log(event.data);
ws.onopen = () => ws.send("subscribe");
ws.onclose = (event) => console.log("closed", event.code, event.wasClean);
```

Two runtime-specific notes:

- **Close your sockets.** An execution's event loop stays alive while a
  socket is open (like a Node or Deno process). A `run_js` call that leaves a
  socket open runs until the execution timeout.
- **Connections do not survive turns.** In stateful sessions the V8 heap is
  snapshotted between calls, but a live TCP connection cannot be. A socket
  from a previous call behaves as closed.

## Inject handshake credentials server-side

Fetch header-injection rules (`--fetch-header` /
`--fetch-header-config`) also apply to WebSocket handshakes for matching
hosts:

```bash
mcp-v8 --http-port=8080 \
  --policies-json=/path/to/policies.json \
  --fetch-header "host=stream.example.com,header=Authorization,value=Bearer my-token"
```

The browser WebSocket API has no way to set *or read* handshake headers, so
the injected credential is structurally invisible to JS: sandboxed code can
use the authenticated connection but can never observe the token. Rules are
host-scoped, so the credential is only ever attached to handshakes with the
configured host. (Method matchers treat the handshake as `GET`.)

## Conformance

The implementation is gated on a vendored subset of the
[Web Platform Tests](https://github.com/web-platform-tests/wpt) `websockets/`
suite — see `server/tests/wpt/` — covering constructor validation, close-code
semantics, messaging, and `binaryType` behavior.

## See also

- [How-to: Network access with fetch](fetch.md) — the same policy and
  header-injection model, for HTTP.
- [Concepts: Security policies](../concepts/policies.md)
