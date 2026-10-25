# HTTP functions reference

Baseline: [PR #293 feature status](scheduler-http-status.md). Routes are opt-in
through `MCP_V8_HTTP_FUNCTIONS_DIR` in ordinary HTTP or SSE mode. Single-node mode
uses that directory for registry storage; cluster mode uses replicated storage.
Dedicated scheduled workers never mount these routes. Existing transport JWKS
authentication applies equally to management and invocation.

## Routes and conditions

| Method and path | Contract |
|---|---|
| `GET /api/http-functions/v1/definitions` | List metadata, omitting source |
| `GET /api/http-functions/v1/definitions/{name}` | Metadata and quoted ETag |
| `PUT /api/http-functions/v1/definitions/{name}` | Create with `If-None-Match: *`; replace with current `If-Match` |
| `DELETE /api/http-functions/v1/definitions/{name}` | Tombstone with current `If-Match` |
| Configured method on `/functions/v1/{name}` | Invoke exact name; no suffix wildcard |

Names contain 1–64 ASCII letters, digits, underscores, or hyphens. Reserved names:
`api`, `mcp`, `health`, `scheduler`, `discovery`.

## Definition

```json
{
  "version": 1,
  "methods": ["POST"],
  "export": "handler",
  "source": "async function handler(request) { return {status: 200, body: request.method}; }",
  "limits": {"timeout_seconds": 30, "heap_mb": 64}
}
```

`version` must be 1. Methods are uppercase `GET`, `POST`, `PUT`, `PATCH`, `DELETE`,
`HEAD`, `OPTIONS`. `export` names a callable in the source; it starts with an ASCII
letter or underscore and continues with those characters or digits. The reserved
name `__mcp_http_result` belongs to the adapter. Source is parsed at registration;
callability and returned data are checked during execution. Module imports use
the ordinary Engine loader and deployment policy.

## Handler input and output

| Input field | Shape |
|---|---|
| `method` | Method string |
| `path` | Full dispatcher path |
| `query` | Ordered array of key/value pairs, preserving duplicates |
| `headers` | Only `content-type`, `accept`, `user-agent`; credentials/cookies omitted |
| `body_base64` | Raw request bytes encoded as standard base64 |

Return an object with integer `status` from 200 through 599, optional string-valued
`headers`, and exactly one of UTF-8 `body` or standard-base64 `body_base64`.
Unknown response fields are rejected. Header names/values must be valid HTTP.
The server rejects `connection`, `transfer-encoding`, `content-length`,
`keep-alive`, `trailer`, `upgrade`, `te`, `proxy-authenticate`, and
`proxy-authorization`, and calculates content length itself. HEAD suppresses the
body. Statuses 204, 205, and 304 suppress body and content length. Streaming and
upgrades are unsupported. Each execution starts with a fresh heap.

## Limits

| Resource | Bound |
|---|---|
| Source | Nonempty, at most 64 KiB |
| Requested timeout | 1–30 seconds, default 30; clamped to deployment ceiling |
| Requested heap | 8–64 MiB, default 64; clamped to deployment ceiling |
| Registry names | 32 including tombstones; no automatic tombstone compaction |
| Concurrent invocations | 16 per server |
| Request body / decoded response body | 1 MiB each |
| URI / response header names and values combined | 8 KiB each |
| Response headers | 32 entries |

The serialized response adapter also bounds its JSON result to 2 MiB. JSON escaping
can therefore exceed that bound before the decoded body reaches 1 MiB.

## Error statuses

Errors use generic empty responses with `x-request-id` for diagnostics, except
existing transport authentication responses.

| Status | Meaning |
|---|---|
| 400 | Invalid definition or request |
| 401 | Existing transport authentication rejection |
| 404 | Missing function |
| 405 | Unconfigured method; `Allow` supplied |
| 409 | Capacity exhausted |
| 412 | Stale revision or failed creation condition |
| 413 | Oversized input |
| 428 | Missing precondition |
| 500 | Handler or response validation failure |
| 503 | Registry/submission unavailable; follower or no quorum |
| 504 | Execution timeout |

An error, disconnect, or deletion does not roll back external effects. See
[execution lifetime](../concepts/http-functions.md) and
[safe management](../how-to/manage-http-functions.md).
