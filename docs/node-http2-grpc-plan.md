# Plan: `node:http2` compat for stock gRPC clients

*Status: phases 1–2 landed — `node:buffer`/`node:events` were already served
by the module loader, and the structured ops (`server/src/engine/http2.rs`) +
`node:http2` client shim (`node_compat/http2.js`) are in, gated by
`tests/http2_e2e.rs` (gRPC-framed unary with trailers, trailers-only,
RST_STREAM, per-stream policy, header injection, secret non-leak). Remaining:
the supporting shims grpc-js imports (`node:net`/`node:tls` stubs,
`node:dns`, `node:stream`, `node:zlib`), an esm.sh target that emits `node:*`
externals for `npm:@grpc/grpc-js`, and the official interop-suite gate.*
*Date: 2026-08-17*

## Goal

Run stock gRPC SDKs (e.g. the Modal JS SDK via `@grpc/grpc-js`) inside the
sandbox **without workarounds**, while preserving the two properties that make
the fetch/WebSocket capability model safe:

1. **Per-request policy.** Every outbound request is OPA-evaluated with a
   structured input (host, path, headers), not just a one-time
   `connect(host, port)` check.
2. **Server-side header injection.** Credentials are attached outside the
   isolate (`--fetch-header` rules) and are structurally unobservable from JS.

## Why not raw sockets

The obvious route — `node:net`/`node:tls` TCP ops with the Node compat shims
stacked on top — breaks both properties. Once the isolate can push opaque
bytes down a socket, policy can only gate the connect, and the host can no
longer see (or inject into) headers inside a byte stream it does not parse.
Raw TCP is explicitly **out of scope**; if it is ever added it gets its own
policy category, off even when everything else is on.

## Design: structured HTTP/2 session ops

`@grpc/grpc-js` sits on Node's `http2.connect()` API, which is
frame-structured, not byte-structured. That is the interception point. New
extension `server/src/engine/http2.rs` (pattern: `websocket.rs`):

- `op_h2_connect(authority) -> session_rid`
  - policy input `{operation: "connect", authority, url_parsed}` against a new
    `http2` chain in `--policies-json` (`data.mcp.http2.allow`).
  - TLS via rustls (same stack as fetch/WebSocket), backed by the `h2` crate's
    client (the layer under hyper — already battle-tested and h2spec-covered
    upstream).
- `op_h2_request(session_rid, headers_json, end_stream) -> stream_rid`
  - **`apply_header_rules` runs here**, per stream, exactly like fetch — this
    is what keeps `x-modal-token-*`-style injection working for gRPC.
  - per-stream policy input `{operation: "request", authority, headers,
    path}`.
- `op_h2_send_data(stream_rid, data_b64, end_stream)`
- `op_h2_recv(stream_rid) -> {kind: headers|data|trailers|reset|goaway, ...}`
- `op_h2_close_stream` / `op_h2_close_session` / `op_h2_ping`

On top, a `node:http2` JS shim (registered in the module loader alongside the
existing `node:os`/`node:process`/... shims) implementing the subset grpc-js
uses: `connect()`, `ClientHttp2Session` (`request`, `ping`, `close`, events
`connect`/`error`/`goaway`), `ClientHttp2Stream` (writable side, events
`response`/`data`/`trailers`/`end`/`close`), and the `constants` table.

Supporting shims grpc-js also needs, in likely order of effort:
`node:buffer` (Buffer over Uint8Array — biggest one), `node:events`
(EventEmitter), `node:stream` (Duplex subset), `node:dns` (resolve via a host
op or stub to the authority literal).

## Test plan (gates, same philosophy as the WPT gate for WebSocket)

1. **gRPC interop suite** — the official fixed case list (`empty_unary`,
   `large_unary`, `client_streaming`, `server_streaming`, `ping_pong`,
   `custom_metadata`, `status_code_and_message`, ...) driven by stock
   `@grpc/grpc-js` inside the sandbox against a `tonic` interop server spun up
   in-test. `custom_metadata` doubles as the per-stream header-injection test.
2. **Curated Node core tests** — vendor the `test-http2-*` files covering the
   API surface grpc-js touches, run via the same expectations-manifest pattern
   as `tests/wpt/`.
3. **In-repo integration tests** — tonic server edge cases: trailers-only
   error responses, GOAWAY mid-stream, RST_STREAM, flow-control backpressure,
   plus the security tests (policy deny per stream, header injection only for
   allowlisted authorities, no leak to others).
4. **Fuzz targets** — mirror `fuzz_fetch_operations` for the h2 op parameter
   surface.

## Sequencing

1. `node:buffer` + `node:events` shims (needed by everything).
2. `http2.rs` ops + minimal `node:http2` shim; gate with tonic unary test.
3. Streaming + trailers; gate with the interop suite.
4. Wire the Modal SDK end-to-end as the acceptance demo.
