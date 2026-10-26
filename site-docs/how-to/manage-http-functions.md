# Update and retire an HTTP handler safely

Assume HTTP functions are enabled, and you have access to the server's existing
HTTP authentication. Add `Authorization: Bearer …` to each request when JWKS is
configured. Definitions grant no separate owner or role; control access to both
management and invocation with the transport's existing deployment controls.
For a first local registration, use the [tutorial](../tutorials/first-http-function.md).

## Replace the revision you inspected

Fetch the current metadata and its quoted ETag:

```sh
curl --fail-with-body -i   http://127.0.0.1:8080/api/http-functions/v1/definitions/greeting
```

Prepare a complete new definition in `greeting.json`, following the
[definition schema](../reference/http-functions.md). Replace `REVISION` with the
ETag's value:

```sh
curl --fail-with-body -i -X PUT   http://127.0.0.1:8080/api/http-functions/v1/definitions/greeting   -H 'Content-Type: application/json' -H 'If-Match: "REVISION"'   --data-binary @greeting.json
```

On 412, fetch metadata again and reconcile the intervening update. Do not silently
replace it using an old revision. On 428, supply the appropriate precondition.
New names require `If-None-Match: *`. Metadata omits source, so retain your reviewed
source outside the registry response.

## Retire a handler

Delete using the current ETag:

```sh
curl --fail-with-body -i -X DELETE   http://127.0.0.1:8080/api/http-functions/v1/definitions/greeting   -H 'If-Match: "REVISION"'
```

New invocations return 404. Already admitted invocations can finish their pinned
revision. Deletion leaves a tombstone; recreating the name receives a fresh
revision, and stale ETags remain invalid. Tombstones count toward the 32-name
registry limit, so rotating names is not a retention strategy.

## Handle failures without duplicating effects

Use `x-request-id` to correlate diagnostics with execution ID and revision. On
503 in a cluster, verify leader and quorum before retrying management. After an
uncertain write, inspect current metadata first. On 409 invocation capacity,
reduce concurrency. On 500 or 504, investigate logs and external effects before
retrying: each invocation retry creates a new execution. Disconnecting a client
does not establish that the handler stopped; it can continue until Engine timeout.

Use application idempotency for external mutations. The server does not replay
ambiguous submissions or roll back effects. See the
[revision model](../concepts/http-functions.md) and [limits](../reference/http-functions.md).
