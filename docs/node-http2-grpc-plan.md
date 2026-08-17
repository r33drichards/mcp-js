# Plan: `node:http2` compat for stock gRPC clients

*Status: **complete** — stock `@grpc/grpc-js@1.12.6` runs unmodified in the
sandbox over the structured `node:http2` ops, gated by the official gRPC
interop cases in `server/tests/grpc_interop.rs`. See "What landed" below for
the shipped surface and "Known gaps" for what a follow-up would add.*
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

## What landed

- **`server/src/engine/http2.rs`** — structured session/stream ops over the
  `h2` crate, `http2` policy category, per-stream `apply_header_rules`.
- **`node_compat/http2.js`** — client-side `node:http2` (connect,
  ClientHttp2Session/Stream, constants incl. the `NGHTTP2_FLAG_*` table).
- **Supporting shims** — `node:net` (IP helpers + inert Socket), `node:tls`
  (option plumbing; TLS terminates host-side), `node:dns` (pass-through
  resolver + `promises.Resolver`), `node:fs` / `node:http` (import-compatible
  stubs), `node:stream` (Readable/Writable/Duplex/Transform/PassThrough),
  `node:zlib` (gzip/deflate over CompressionStream). `setImmediate` /
  `clearImmediate` / `queueMicrotask` added to the timers layer.
- **Gate** — `server/tests/grpc_interop.rs`: an in-process `h2::server`
  implementing `grpc.testing.TestService` (hand-encoded protobuf) and the
  official case list driven by stock grpc-js from esm.sh. Network-dependent,
  so `#[ignore]`d per repo convention: `cargo test --test grpc_interop --
  --ignored`. A hermetic `node_compat_shims_smoke` test runs in normal CI.

### Bugs the interop gate caught

Each of these passed the hand-written h2 tests and failed only against the
real client, which is exactly why the official suite was the right gate:

1. `session.socket` had to be an EventEmitter (grpc-js attaches `once`), and
   needed `destroyed`; missing it caused an infinite transparent-retry loop
   that OOM'd the isolate.
2. `session.state` (window sizes) and `session.encrypted` are read on every
   `createCall`.
3. A `write()` after the peer concluded the stream must be discarded, not
   raised — otherwise a trailers-only reply became a client-side INTERNAL.
4. The `response` event's second argument must carry
   `NGHTTP2_FLAG_END_STREAM`; without it grpc-js never reads the gRPC status
   off a trailers-only response (`unimplemented_method` failed).
5. **Timers had no `unref`.** Every run wedged for almost exactly 1800
   seconds — the signature of grpc-js's 30-minute channel idle timer. Node
   `unref()`s such timers so they never hold the process open; our
   `setTimeout` returned a bare id with no handle to unref, so the pending
   sleep op kept the isolate's event loop alive for the full half hour.
   `setTimeout`/`setInterval` now return a Node-style `Timeout` handle whose
   `ref()`/`unref()` drive `Deno.core.refOpPromise`/`unrefOpPromise` (the
   handle coerces to its numeric id, so browser-style `clearTimeout(id)` and
   arithmetic are unaffected). This was never gRPC-specific: *any* npm
   package holding a background timer would have pinned an execution open
   until its timeout.
6. Cancelling a stream (`cancel_after_begin`, `timeout_on_sleeping_server`)
   removed the registry entry without waking a pending `op_h2_read`, leaving
   an op alive forever. Fixed with a per-stream `CancellationToken` awaited
   by both the response and read ops.

## Known gaps

- No `node:http2` server APIs, no push streams, no `options.createConnection`
  (host owns the transport — that is the security model).
- `node:stream` is a behavioral subset: no highWaterMark backpressure
  accounting, no `readable` pull mode beyond `read()`.
- `node:zlib` is one-shot only (no streaming classes), so gRPC per-message
  compression beyond `identity` is untested.
- The remaining official interop cases that need a full TestService
  (`cancel_after_first_response`, `special_status_message`,
  `unimplemented_service`, and the auth/oauth family) are not implemented.
