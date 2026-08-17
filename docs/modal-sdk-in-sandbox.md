# Running the Modal SDK inside the sandbox

The `modal` npm SDK ([docs](https://modal.com/docs/guide/sdk-javascript-go))
uses nice-grpc, which opens HTTP/2 sockets through `node:tls` — APIs this
sandbox does not have. Every bundler workaround (esm.sh `?bundle`,
`?no-external`, aliasing to nice-grpc-web) dead-ends on that transport, and
gRPC-Web is not an option either: Modal's envoy has no grpc-web filter.

None of that is needed. **Native gRPC unary calls are just HTTP/2 POSTs**
with 5-byte-prefixed protobuf frames and a `te: trailers` header — all
expressible with `fetch`. Verified against `api.modal.com`: requests round-trip
into the real service (fake credentials come back as
`grpc-status 16: Token not found`, authenticated calls succeed).

Requirement: the runtime's fetch must speak HTTP/2 — the reqwest `http2`
feature, enabled in `server/Cargo.toml`. Deployments built before that change
negotiate HTTP/1.1 and gRPC calls fail.

## The recipe

1. Parse Modal's `api.proto` **at runtime** with protobufjs (the npm package
   ships only a bundled dist; its codecs are module-scoped and unreachable).
   The proto is fetchable from jsDelivr, and protobufjs bundles the google
   well-known types it imports.
2. Implement unary gRPC over `fetch`: frame → POST → check the `grpc-status`
   response header (error responses are trailers-only, so the status lands in
   the headers where fetch can see it) → decode response frames.
3. Wrap that as a nice-grpc-shaped client (camelCase methods, plain objects,
   async generators for server-streaming) with a `Proxy`.
4. Inject it into the real SDK: `new ModalClient({ tokenId, tokenSecret,
   cpClient })`. The SDK's own `createClient` — the only code path touching
   TLS — never runs, and every high-level API (`client.apps`,
   `client.sandboxes`, `client.functions`, …) works through the injected
   client.

The full working snippet lives in `server/tests/modal_grpc.rs` (an ignored
network test): `cargo test --test modal_grpc -- --ignored --nocapture`.
Set `MODAL_TOKEN_ID` / `MODAL_TOKEN_SECRET` for an authenticated run.

## Caveats

- **Server-streaming** works by buffering the whole response (frames parsed
  after completion). Live incremental consumption needs `Response.body`
  streaming from the fetch op; long-poll-style RPCs (function outputs) are
  fine buffered. Client-streaming RPCs are not supported over fetch.
- **Typed SDK errors**: the SDK's `err instanceof ClientError` checks cannot
  match errors thrown by an injected client (the bundle's `ClientError` is a
  private copy), so e.g. `NotFoundError` mapping degrades to the raw gRPC
  error. Message and `code` are preserved.
- **int64 fields** decode as JS numbers (`longs: Number`); values above
  2^53 would lose precision. Modal's id/timestamp fields are safe in
  practice.
- The `task_command_router` service (sandbox exec I/O tunnels) targets
  per-task hosts and is untested here.
