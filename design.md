# Design: MCP Events support for mcp-js

**Status: proposal; design-only PR.** No implementation, deployment, subscription,
credential, or verified host integration is introduced by this change.

## Goal and selected scope

Allow an authorized host to discover application events, explicitly subscribe a
chat, and receive matching occurrences even when no run_js request is open.
The first implementation is **server-side, webhook-only MCP Events**, aligned
with the published ChatGPT webhook profile. It is disabled by default.

This is not a scheduler, an SSE console-output feature, a downstream notification
listener, or permission to start autonomous agent work. Receipt of an event is
not a grant to execute instructions embedded in its payload.

Reviewed against mcp-js main e1f1d6552c5a2c2d3e7d39b069c1d07707dbc2f3.
All APIs and defaults described below are proposed unless stated otherwise.

## 1. Specification: distinguish three contracts

| Contract | Consequence |
|---|---|
| Core MCP 2026-07-28 | server/discover and per-request metadata; subscriptions/listen carries opted-in tool, prompt and resource changes. It does not define application Events methods. |
| Experimental MCP Events design sketch | events/list, events/subscribe, events/unsubscribe, plus optional poll/push; not an adopted core specification. |
| ChatGPT MCP Events integration | Webhook and callback-verification subset requiring MCP 2026-07-28; no poll, stream, gap or terminated controls. |

Pin the Events proposal to upstream revision
[d5316be214be2e76e51462069c1d67e00a5a7619][events-draft]. Its repository
[explicitly says experimental][events-readme]. Pin core MCP references to
54b91cdb8f21825ce586c452704f319d49866ef8. The [OpenAI guide][openai-events]
was read on 2026-10-26 and is a mutable host contract, not a finalized MCP standard.

Use an explicit compatibility-profile module. In particular, the pinned sketch
uses CallbackEndpointError -32027 while OpenAI documents -32015. Select -32015
for the first ChatGPT profile, with a bounded data.reason category; do not blend
profiles or guess from the recipient hostname. A later upstream profile needs
its own negotiation/configuration decision and fixtures.

Advertise only webhook delivery and capabilities.events: {}. Do not claim
listChanged, polling, streaming, replay, or broad host compatibility. Work chats
on web, desktop Work with Cloud, and dots are the documented ChatGPT surfaces,
subject to workspace controls. Ordinary MCP support in Responses API, Codex or
Claude does not prove this Events integration is supported.

Sources:

- [Pinned experimental Events sketch][events-draft] and [WG status][events-readme].
- [OpenAI MCP Events guide][openai-events].
- [Core change log][core-changes], [subscriptions][core-subscriptions] and
  [Streamable HTTP][core-http].
- [Standard Webhooks signing specification][standard-webhooks].
- Existing [scheduler/Events proposal](docs/schedule-tool-mcp-events-design.md)
  and [scheduler status](site-docs/reference/scheduler-http-status.md).
- [weechat-mcp Events adapter reference][weechat-events]: concrete prior art,
  not code directly reusable in Rust/rmcp or proof of client compatibility.

## 2. Repository fit and implementation blockers

- [server/src/mcp.rs](server/src/mcp.rs) implements McpService and
  StatelessMcpService with tool/resource discovery but no Events methods.
  Both must share one native Events service, or reject Events when unsupported.
- [server/Cargo.toml](server/Cargo.toml) selects rmcp 3.5.1. Inspection of that
  crate shows ServerHandler::on_custom_request, a candidate dispatch hook, but
  typed ServerCapabilities has **no top-level events field**. A reviewed SDK
  extension/upgrade or equivalent supported serialization change is a gate:
  prove the field survives server/discover. Do not substitute experimental.events
  or extensions.events for the host's required wire shape.
- [server/src/engine/mcp_client.rs](server/src/engine/mcp_client.rs) implements
  downstream mcp.callTool/listTools/servers. It is not Events infrastructure;
  do not automatically relay upstream notifications or repurpose callTool.
- [server/src/engine/execution.rs](server/src/engine/execution.rs) keeps execution
  records in memory and console data in sled. This is not a durable event log.
  Its cancel path requests isolate termination and marks cancelled; that alone
  does not establish that every native operation stopped.
- [SSE_IMPLEMENTATION.md](SSE_IMPLEMENTATION.md) concerns output streaming,
  not signed application-event callbacks. Existing scheduler core/runner and
  management layers remain separate from Events delivery.

Before implementation, prove exact discovery serialization, custom-method
routing, 2026-07-28 result/metadata handling, and trusted principal propagation.
Keep existing authentication, Origin checks and policy middleware. Do not add
an alternate raw JSON-RPC route that bypasses those controls. Legacy tools and
older-protocol behavior must not regress; unsupported Events combinations fail
explicitly rather than partially advertising support.

## 3. Proposed operator, publisher and MCP interfaces

An operator opt-in (proposed --events-config) defines a static catalog: event
name, description, subscription inputSchema, payloadSchema, allowed filters,
publish permissions and consumer authorization. No guest catalog registration
or mutation in v1. Invalid enabled config fails startup. Disabled means no
Events capability and method-not-found for Events RPCs.

An explicitly granted guest API can publish application events:

    const receipt = await events.emit("app.job.finished", {
      job_id: "job_123", status: "completed"
    });
    // Proposed receipt: { eventId, accepted: true }

The native op derives owner and execution identity from trusted admission,
checks publish policy and payloadSchema, and durably records the event before
returning acceptance. No owner/tenant override, callback URL, signing secret,
recipient selection or subscription administration is exposed to guest JS.
Follow the existing native op/hook/policy patterns; disabled or missing grant
fails closed. This is a separate publisher surface, not mcp.callTool.

Acceptance means recorded, not delivered or processed. Emit is irreversible;
a subsequent exception does not retract it. Never rerun a producer on an
unknown emit outcome. IDs are native and persisted. If producer dedup keys are
added, scope them to owner/event type, reject conflicting data, and define
bounded retention rather than claiming unlimited exactly-once effects.

Future native adapters may publish execution/scheduler outcomes. An event such
as mcpjs.execution.finished must follow an actual completion observation after
native work exits. Admission, polling timeout, cancellation request, lost worker
or unknown outcome must not become a success event. Subscribe does not run a
job; creating a schedule does not subscribe a chat. Do not emit on Events RPCs
or automatically subscribe all executions, creating feedback loops.

| MCP method | Contract |
|---|---|
| server/discover | Advertise top-level capabilities.events only when auth, storage and the selected profile are operational. |
| events/list | Return authorized event descriptors, delivery: [webhook], inputSchema and payloadSchema; bounded pagination, owner/revision-bound cursors. |
| events/subscribe | Validate/authorize, verify callback control, then atomically create or refresh a single durable identity. |
| events/unsubscribe | Owner-authorized, idempotent removal by name + arguments + delivery URL; return the profile's empty method result. |

These are protocol methods, not tools named events/subscribe. No events/poll,
events/stream or custom events on subscriptions/listen in the first release.

## 4. Identity, lifecycle and isolation

Webhook subscriptions require a stable authenticated principal, derived from
validated issuer/subject and tenant/security domain, plus the installation/chat
grant scope required to prevent cross-chat inheritance. A session ID, event ID,
unsigned header or model-supplied owner field is not authentication. Explicit
single-operator deployments need a stable operator binding; anonymous callbacks
are forbidden. Accepted auth without owner propagation is a release blocker.

Identity is (principal, exact validated callback URL, event name, canonical JSON
arguments). Specify canonicalization (proposed RFC 8785), reject duplicate keys
and non-finite numbers, and preserve callback path/query meaning. Derive a stable
domain-separated HMAC subscription ID with a private persisted server key.
The ID is routing data, not authorization. Key rotation needs migration.

State: pending-verification -> active -> suspended/expired/revoked/unsubscribed.

- Check schema, filter ownership and quotas before network work. Reserve bounded
  pending capacity with expiring revision tokens; do not lock storage across HTTP.
- Activate only after challenge success and renewed authorization. Concurrent
  refresh/verification uses compare-and-swap revisions; stale work cannot
  resurrect an unsubscribe or overwrite a newer secret.
- Refresh the same identity without duplicating listeners or deliveries.
  Changed URL/filter means a new identity; secrets and TTL are mutable fields.
- Proposed TTL: one-hour default, cap 24h; honor positive hints at no more than
  requested duration. Reject zero/negative/invalid hints. ttlMs:null requests
  unlimited life but v1 grants finite life; no refreshBefore:null in v1.
- Expiry, unsubscribe and revoked authorization cancel pending retries and stop
  new delivery admission. Recheck owner permission, subscription revision and
  expiry immediately before send. Already transmitted work cannot be recalled.
- Persist state for the full granted TTL. Keep only bounded tombstones needed
  to fence late work; successful refresh may reactivate suspended delivery only
  under current authorization and callback-verification requirements.

Subscribe returns id, refreshBefore, cursor, truncated and optionally sanitized
deliveryStatus. Neither secret nor raw callback response is returned. Each chat
requires an explicit host subscription/grant; a new chat inherits none.

## 5. Callback and payload security

Before application delivery, send a signed control body with type:verification
and a fresh random single-use challenge. Require 2xx plus the echoed challenge,
compared in constant time within a bounded lifetime. Verification webhook IDs
are unique and separate from application event IDs. Proposed verification cache:
five minutes, bound to principal + exact URL + signing-secret revision. Reverify
on key change; cache entries must not bypass revocation or cross owners.

- Validate whsec_ secrets: base64 decoding to 24-64 bytes. Support explicit key
  rotation with a short bounded overlap (proposed five minutes); emit Standard
  Webhooks space-separated old/new signatures during that overlap.
- Serialize event body once. Sign and send exactly those bytes. Set Content-Type,
  webhook-id, webhook-timestamp, webhook-signature and X-MCP-Subscription-Id.
  webhook-id equals body eventId. Retries preserve ID/body and use fresh signing
  timestamps/signatures. Verify against independent Standard Webhooks vectors.
- Require public HTTPS; proposed v1 port policy is 443 only. Reject credentials,
  fragments, ambiguous URLs and all prohibited special-use IPv4/IPv6 addresses,
  including mapped IPv6 and metadata endpoints.
- Validate DNS on every new connection, connect to a validated address, preserve
  hostname for Host/SNI/certificate validation, and do not resolve a second time
  after checking. Test pooled connections and multi-address answers. Disable
  redirects, implicit proxy environment inheritance and ambient auth/cookies.
- Use the same checks for challenge and delivery. Reuse audited URL/IP
  primitives, not guest fetch authority or unrelated private-network exceptions.
- Proposed deadlines: 3s connect / 10s total; challenge responses <=16 KiB.
  Discard bounded delivery responses. Errors expose categories, not raw response
  bodies, headers or status lines that become an SSRF response oracle.
- Store signing keys, HMAC key, callback URLs and private event data in protected
  host storage/backups, outside guest fs/heap/artifacts. Redact before tracing
  and generic validation errors; do not rely on filtering secrets after logging.

Application envelope: eventId, name, timestamp, data, cursor. Timestamp records
occurrence time with timezone; application fields remain in data. One event per
POST, entire serialized body <=262,144 bytes including envelope overhead. Reject
oversize events; never truncate JSON. Prefer bounded summaries and authorized
read references to raw code/log/artifact output. Payloads are untrusted data.

## 6. Durability, replay and delivery guarantees

Use a native store for subscriptions/revisions, immutable events/outbox,
per-delivery receipts and bounded tombstones. Keep it separate from the
in-memory execution registry and guest-writable state. Transaction + durability
barrier precede acceptance. Corrupt schema, full disk or unavailable durable
storage fails closed, never a transient-Map fallback.

Start single-node/single-writer. Reject Events-enabled multi-writer deployments
until replicated ownership/fencing is implemented and tested. A transport
session or stateless execution mode is not a storage owner. Both allowed MCP
service modes must retain subscriptions across restart for their granted TTL.

Proposed bounds: 32 subscriptions/principal, 1,024/server; 2 pending challenges
per principal, 16 globally; 8 MiB pending outbox/principal, 64 MiB global; 1,000
pending entries/principal, 10,000 global (byte caps also apply); 1 delivery per
subscription and 8 globally. Atomic admission rejects saturation before accepted
publication; never silently evict an accepted pending event to admit another.
No unbounded producer/worker/retry queue.

Choose transactional fan-out or bounded resumable fan-out before implementation.
For the latter, commit the event plus a persistent fan-out cursor and unique
(subscription revision,eventId) delivery keys. Recovery resumes delivery, never
producer execution. Workers recheck revision/expiry/permission before send.

Delivery has bounded at-least-once attempts, not exactly-once receipt, processing
or eventual delivery. A 2xx acknowledges webhook receipt only. Crash after
receiver acceptance but before local receipt persistence may cause a duplicate
with the same event ID. Receiver deduplication and idempotent write tools matter.

Proposed retry budget: 8 total attempts, at most 24h and never beyond expiry;
exponential backoff + jitter for network ambiguity, 408, 429 and 5xx, with bounded
Retry-After. No retries on 410/413, redirects or other permanent 4xx; suspend
and record sanitized failure. Failed deliveries remain inspectable through an
authenticated owner-scoped management surface. One broken subscriber must not
starve others. Delivery retry must never rerun JavaScript or a scheduled job.

**v1 replay decision:** cursor:null, no public replay history. Reject non-null
cursors rather than pretending to honor them. Validate maxAgeMs if supplied
without claiming it creates history. truncated:false when no replay was requested.
Durable pending delivery recovery is internal, not cursor-based replay. Events
missed before subscription or while the producer is down cannot be recovered
through this interface. Bound retention and show exhausted records as failed.
A durable cursor/replay feature needs a separate design and gap/profile tests;
never advance a public watermark past undelivered records.

## 7. Future implementation sequence and acceptance gates

1. SDK/discovery support + custom RPC routing, protocol envelopes and trusted
   owner context through existing authenticated middleware. Capture actual wire
   fixtures and keep legacy tool behavior unchanged.
2. Native config/catalog/store, authorized filters, quotas, TTL/rotation/revoke
   state machine and protected secret handling.
3. Callback challenge + SSRF-safe transport + signing + bounded durable outbox,
   retries and restart recovery.
4. Policy-gated events.emit op. No retained V8 callbacks after a run ends.
   Later scheduler/execution adapters use observed outcomes and existing
   authority/no-execution-retry rules, not delivery receipts as success evidence.
5. Opt-in docs and an explicitly authorized real-host canary. Merge is not
   deployment approval. Live credentials/subscriptions need separate permission.

Required tests before claiming support:

- Wire: real HTTP/stdio exchanges for discover/list/subscribe/unsubscribe,
  exact top-level events capability, version/profile errors and empty results,
  pagination, disabled behavior, malformed/oversized schema arguments.
- Isolation: issuer/tenant/owner/chat boundaries; canonical arguments; concurrent
  duplicate/refresh/unsubscribe/revoke races; stale verification resurrection;
  publish denial and no guest secret/recipient/owner override.
- Security: public-to-private DNS rebinding, multiple answers, IPv4/IPv6/mapped
  address cases, pool reconnect, TLS/SNI, redirect/proxy denial; challenge replay,
  wrong echo, expiry; secret canaries absent in logs/errors/tools/artifacts.
- Signing: independent vectors, exact Unicode/body bytes, rotation overlap,
  fresh retry timestamp, immutable event ID, body at/over 262,144-byte limit.
- Durability: crash points around activation, flush, fan-out, send and receipt;
  disk full, incompatible schema, expiry and tombstone races; duplicate delivery
  is distinguishable and no producer is rerun.
- Bounds: concurrent subscription storms, atomic quota/byte limits, slow
  receivers, retry exhaustion and fairness; no silent accepted-event loss.
- Host: authorized ChatGPT Work/Cloud discovery -> explicit chat subscription
  -> challenge -> one matching event -> no nonmatching event -> refresh/key
  rotation -> unsubscribe -> no further admitted delivery. Distinguish receiver
  2xx from actual host/agent response. Mocks do not prove host compatibility.

Keep Events disabled by default until these gates pass. Expose sanitized counts,
latency, failures and expiry metrics, not raw URLs/secrets/payloads. Rollback
stops admission/delivery without deleting durable records; re-enable honors
expiry/revocation and never re-executes producers.

## 8. Decisions to confirm before coding

- Confirm server-side webhook publishing, static operator catalog and guest emit
  API as the first product scope; downstream event consumption is separate.
- Select the reviewed SDK serialization change and authenticated principal
  plumbing. These are blockers, not existing implementation claims.
- Choose durable backend/flush semantics, single-owner enforcement and fan-out
  algorithm; confirm limits, TTL, retry, rotation and retention defaults.
- Freeze a fixture snapshot of the host guide at implementation time. Reconcile
  draft/profile drift explicitly instead of silently changing a live contract.

Out of scope: scheduler redesign, downstream Events relay/client, polling/push,
public inbound webhook receiver, cross-node delivery, public replay, autonomous
agent loops, or production/credential changes. This PR only adds design.md.

[events-readme]: https://github.com/modelcontextprotocol/experimental-ext-triggers-events/blob/d5316be214be2e76e51462069c1d67e00a5a7619/README.md
[events-draft]: https://github.com/modelcontextprotocol/experimental-ext-triggers-events/blob/d5316be214be2e76e51462069c1d67e00a5a7619/docs/design-sketch-proposal.md
[openai-events]: https://developers.openai.com/plugins/build/mcp-events
[core-changes]: https://github.com/modelcontextprotocol/modelcontextprotocol/blob/54b91cdb8f21825ce586c452704f319d49866ef8/docs/specification/2026-07-28/changelog.mdx
[core-subscriptions]: https://github.com/modelcontextprotocol/modelcontextprotocol/blob/54b91cdb8f21825ce586c452704f319d49866ef8/docs/specification/2026-07-28/basic/patterns/subscriptions.mdx
[core-http]: https://github.com/modelcontextprotocol/modelcontextprotocol/blob/54b91cdb8f21825ce586c452704f319d49866ef8/docs/specification/2026-07-28/basic/transports/streamable-http.mdx
[standard-webhooks]: https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md
[weechat-events]: https://github.com/r33drichards/weechat-mcp/blob/24d3cbb6b0230a4d1d64a79e740c7601d1818991/MCP_EVENTS.md
