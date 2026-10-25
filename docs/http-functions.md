# Dynamic JavaScript HTTP functions

Set `MCP_V8_HTTP_FUNCTIONS_DIR` to enable registration and invocation in ordinary HTTP or SSE server mode. In single-node mode this directory holds the durable registry. Cluster mode uses the existing replicated store and accepts requests on the leader; followers and nodes without quorum return 503. Scheduled-only workers never mount these routes.

The routes inherit the transport's authentication: configuring `--jwks-url` requires a verified token for management and invocation. Without JWKS, they have the same access as `/api/exec`. Definitions introduce no additional roles, owners, or authority profiles. Handlers run through the ordinary Engine with its configured policies, capabilities, imports, and deployment ceilings.

## Register and invoke

Start a local server:

```sh
MCP_V8_HTTP_FUNCTIONS_DIR=./http-functions ./target/debug/server \
  --heap-store none --http-port 8080 --bind-host 127.0.0.1
```

Create a definition. Add `Authorization: Bearer …` to each request when JWKS is configured.

```sh
curl -i -X PUT http://127.0.0.1:8080/api/http-functions/v1/definitions/echo \
  -H 'If-None-Match: *' -H 'Content-Type: application/json' \
  --data-binary '{"version":1,"methods":["POST"],"export":"handler","source":"async function handler(request) { return {status:200, headers:{\"content-type\":\"application/json\"}, body:JSON.stringify(request)}; }","limits":{"timeout_seconds":30,"heap_mb":64}}'
curl -X POST 'http://127.0.0.1:8080/functions/v1/echo?a=1&a=2' --data-binary 'hello'
curl http://127.0.0.1:8080/api/http-functions/v1/definitions
```

Creation requires `If-None-Match: *`. Replacement and deletion require `If-Match` with the quoted ETag returned by registration or `GET /api/http-functions/v1/definitions/echo`. Missing preconditions return 428; stale revisions return 412. Delete with:

```sh
curl -X DELETE http://127.0.0.1:8080/api/http-functions/v1/definitions/echo \
  -H 'If-Match: "REPLACE_WITH_REVISION"'
```

Metadata responses omit source. Deletion commits a tombstone. Recreating a deleted name produces a fresh revision, so old ETags remain stale across restarts.

## Handler contract

Source is parsed at registration and executes only during invocation. Declare the callable named by `export`; JavaScript and TypeScript module imports use the existing Engine loader. The name `__mcp_http_result` is reserved for the result adapter. Each invocation starts with a fresh heap and pins one definition revision.

The handler receives `method`, `path`, `query`, `headers`, and `body_base64`. `path` is the full dispatcher path. `query` is an ordered array of key/value pairs preserving duplicates. Headers include only `content-type`, `accept`, and `user-agent`; credentials and cookies are omitted. The body contains raw bytes encoded as standard base64. The dispatcher mounts one exact name with no suffix wildcard.

Return an object with integer `status` (200–599), optional string-valued `headers`, and exactly one of `body` (UTF-8) or `body_base64` (standard base64). Console output remains execution logging. The server validates response headers, rejects hop-by-hop and caller-provided content-length headers, and calculates content length. HEAD suppresses the body; 204, 205, and 304 suppress bodies and content length. Streaming and upgrades are unsupported.

## Bounds and failures

Names use 1–64 ASCII letters, digits, underscores, or hyphens; infrastructure names are reserved. Methods are GET, POST, PUT, PATCH, DELETE, HEAD, and OPTIONS. Definitions allow at most 64 KiB of source, 1–30 seconds, and 8–64 MiB of heap, clamped to deployment ceilings. The registry retains at most 32 names including tombstones; it does not compact tombstones automatically. Each server admits at most 16 concurrent requests. Input and decoded output are limited to 1 MiB, URIs and response headers to 8 KiB, and response headers to 32 entries.

Responses include an `x-request-id` header linking redacted diagnostics to execution IDs and pinned revisions. Errors use empty generic status responses: 400 invalid definition/request, 404 missing function, 405 wrong method (with Allow), 409 capacity, 412 stale revision, 413 oversized input, 428 missing precondition, 500 handler/response failure, 503 registry/submission unavailable, and 504 timeout. Transport authentication returns its existing 401 response.

A committed compare-and-set fences cluster reads before admission. Quorum loss cannot publish speculative definitions or admit from a stale follower cache. Already admitted requests may finish their pinned revision after replacement or deletion. HTTP retries create new executions. Submission is never replayed automatically, including after ambiguous failures or disconnects. A disconnected request may continue until its Engine timeout; capacity remains held while the execution completes. External effects have no rollback or exactly-once guarantee.

## Validation

The implementation passed 277 server library tests, 31 server integration tests, 33 scheduler-core tests, and 34 scheduler-runner tests. The scheduler suites each mark one subprocess helper ignored during discovery; parent tests invoke those helpers explicitly.

```sh
cargo test --locked -p server --lib -- --test-threads=1
cargo test --locked -p server --test jwks_auth_e2e --test base64_and_formdata \
  --test stdio_e2e --test sandbox_print_bypass -- --test-threads=1
cargo test --locked --manifest-path scheduler-core/Cargo.toml
cargo test --locked --manifest-path scheduler-runner/Cargo.toml
cargo build --locked -p server --bin server
rustfmt --edition 2024 --check server/src/http_functions.rs server/tests/jwks_auth_e2e.rs
cargo fmt --manifest-path scheduler-core/Cargo.toml --check
cargo fmt --manifest-path scheduler-runner/Cargo.toml --check
git diff --check
```
