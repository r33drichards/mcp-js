# HTTP revisions and execution lifetime

Dynamic HTTP functions add named request handlers to the ordinary HTTP/SSE
server. The registry stores definitions; each invocation uses the same Engine,
capability policies, import loader, and deployment ceilings as ordinary execution.
Enabling a registry does not create a new authority profile. See
[feature status](../reference/scheduler-http-status.md) for the baseline.

## Revisions protect management races

An ETag identifies one definition revision. Conditional creation, replacement,
and deletion ensure that a caller changes the state it inspected. Tombstones
preserve history across delete/recreate, so an old revision never becomes current
again. The 32-name bound includes those retained tombstones.

Single-node storage is durable sled state. In a cluster, the existing replicated
store is authoritative. Reads cross a committed compare-and-set barrier before
admission. A separate current-term barrier commits surviving prior-term registry
proposals before loading a candidate; otherwise an ambiguous write could prevent
future comparisons from progressing after restart. Followers and nodes without
quorum return 503 rather than executing from stale local definitions.

## A call pins one revision

Replacement changes future admissions. An already admitted call keeps its pinned
source and limits even if the definition is replaced or deleted. Every call uses
a fresh heap, so globals do not carry across requests. Persistent host effects
still depend on the configured capabilities and external systems.

The adapter serializes request data into the module invocation and reads a
structured return value through a reserved export. Console output stays in logs.
Filtering caller headers keeps bearer tokens and cookies out of the handler input;
it does not make the handler a separate security principal. Shared transport
access still controls who can manage or invoke definitions.

## Uncertainty outlives the HTTP connection

The server never automatically replays a submission. A disconnected client may
leave work running until its timeout, and the concurrency slot remains reserved
while it completes. Client retries create distinct executions. Quorum loss blocks
new registry admission but does not revoke already admitted work. Applications
that mutate remote state need idempotency or resource-side fencing.

See the [handler contract](../reference/http-functions.md),
[update guide](../how-to/manage-http-functions.md), and
[NixOS verification guide](../how-to/scheduler-http-nixos.md).

## Two protocols share one registry

HTTP and MCP are adapters over the same management and invocation handlers. The
registry and its invocation slots belong to the server, so opening another MCP
session does not create a fresh registry or more capacity. A revision read over
one protocol can condition a write over the other.

Changing protocols does not change authority. Network MCP calls inherit the
ordinary transport's authentication; stdio uses local process authority. The
registry has no per-function owner or read-only role, and the scheduler's recovery
credential grants no authority here.

An invocation slot remains reserved until the Engine confirms its background
execution stopped. Timeout and cancellation requests alone do not establish that
fact. If submission or observation fails ambiguously, the slot is quarantined
for that process rather than reused while work might still run. This conservative
behavior can reduce available capacity and requires investigation; it provides
no proof that external effects stopped or were undone.
