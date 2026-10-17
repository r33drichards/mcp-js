# Schedule JavaScript and deliver MCP Events

Status: design proposal, stacked on [PR #281](https://github.com/r33drichards/mcp-js/pull/281).
Reviewed against repository code and official documentation on 2026-10-15.
This PR adds documentation only. It does not implement a scheduler, create
credentials or subscriptions, or change a deployment.

## Selected scheduling policy

[TaskScheduleCron](../specs/TaskScheduleCron.md) is the selected model:

- Fixed cadence: periodic slots stay anchored to the original start time.
- Missed periodic slots are dropped; no catch-up backlog.
- Each new `(schedule_id, due_at)` occurrence gets at most one dispatch attempt.
- Overlap is allowed. A running, failed, or unknown earlier occurrence does not
  block later occurrences.
- No automatic execution retry, including after an uncertain outcome.
- Disabling stops future admission; already-admitted work may continue.

These are scheduling guarantees, not exactly-once external effects or a promise
that every slot executes. The model abstracts persistence, timer jitter, and the
handoff to the executor. Overdue one-time behavior remains OPEN. Everything below
is a proposal unless explicitly identified as this selected policy.

## Comparison: what other schedulers schedule

The useful common shape is **timing + payload + delivery**, with a stored job ID
and lifecycle operations. A prompt-based scheduler starts an agent to interpret
instructions at fire time. MCPJS can instead run stored JavaScript directly,
using its existing sandbox and policy gates. An event may then wake a host agent;
that agent turn is a separate consumer of the result.

### OpenClaw

- Interface: the `automations` agent tool uses actions, while the CLI supports
  add/create, list/get/show, edit, enable/disable, run, runs, and remove. List
  responses are compact; get returns the full definition.
- Timing: one-shot `at`, fixed `every`, and five/six-field `cron` with timezone
  handling. It also has event-driven schedule types outside this proposal.
- Payload: main-session `systemEvent` text, an `agentTurn` prompt, or operator
  command/script execution. Agent jobs can choose isolated or existing execution
  context; a prompt is the instruction for a later agent run.
- Delivery: `announce`, `webhook`, or `none`; execution context and destination
  are distinct. Command/script output uses the same delivery layer.
- Runtime: the Gateway owns the scheduler. Job definitions, runtime state, and
  run history persist in its shared SQLite database.

Sources: [management](https://docs.openclaw.ai/automation/cron-jobs/managing-jobs),
[timing](https://docs.openclaw.ai/automation/cron-jobs/schedules),
[payloads](https://docs.openclaw.ai/automation/cron-jobs/payloads),
[delivery](https://docs.openclaw.ai/automation/cron-jobs/delivery),
[runtime](https://docs.openclaw.ai/automation/cron-jobs/how-it-works).

### Hermes

- Interface: `cronjob_manage` is the unified agent tool. The guide's `cronjob(...)`
  examples use create/list/update/pause/resume/run/remove; CLI editing uses edit.
- Timing: one-shot relative delays or timestamps, intervals, natural-language
  day/time schedules, and cron expressions.
- Payload: normally `prompt` plus `schedule`, with optional name, skills,
  working directory, and other execution settings. It schedules prompts:
  each run starts a fresh agent session and executes those instructions.
- Alternative: `no_agent` runs a configured script and delivers its stdout
  without an LLM turn.
- Delivery: `deliver` selects origin, local files, or configured platform
  destinations. Execution and delivery failures are tracked separately.
- Runtime: the Gateway ticks every 60 seconds; definitions are loaded from its
  cron job store. A fresh run does not automatically inherit the creating chat.

Source: [Hermes scheduled tasks](https://hermes-agent.nousresearch.com/docs/user-guide/features/cron).

### Implications for MCPJS

Keep the same conceptual separation:

| Concern | MCPJS proposal |
|---|---|
| Timing | Explicit one-time instant or anchored fixed interval |
| Payload | Stored JavaScript, not a natural-language prompt |
| Execution | Existing `Engine` submission path and operation policies |
| Identity | Stable schedule ID; distinct occurrence and execution IDs |
| Inspection | Owner-scoped definition, next slot, and occurrence outcome |
| Delivery | Explicit MCP Events subscription owned by a host/chat |
| Host action | Host decides what to do with an event under its own authority |

This comparison informs interface shape only. It does not import either
product's catch-up, retry, overlap, session, or delivery policies. One-shot
timestamps, fixed-duration intervals, agent orchestration, dependencies between
jobs, and automatic workflow recovery are outside the minimal first version.

## Proposed tool surface

The initial discussion is **JavaScript + cron + optional name**, returning an ID
that can be inspected, disabled, or deleted. Exact names and schemas remain OPEN.
A concrete candidate for `schedule` is:

```json
{
  "code": "console.log('scheduled check')",
  "cron": "0 9 * * *",
  "timezone": "UTC",
  "name": "scheduled-check"
}
```

The initial surface uses one five-field cron expression for recurring wall-clock
slots. It does not add separate `at`, `when`, or fixed-duration `every`
variants. If one-shot scheduling is required, design it separately rather than
embedding a second timing language in this tool.

Proposed constraints:

- Require one standard five-field cron expression and an explicit IANA timezone;
  reject unsupported extensions at creation. Do not silently use the server's
  local timezone.
- Start with inline code captured at creation. File references, heap/filesystem
  snapshots, runtime-limit overrides, and the exact cron parser/dialect require
  separate decisions.
- A name is a label, not an ownership key or unique identifier.
- No callback URL, signing secret, owner ID, bearer token, or host chat ID in
  model-facing schedule arguments.
- Return `schedule_id` on accepted creation. Returning normalized `cron` and
  `timezone`, `next_run_at`, `enabled`, and `revision` is useful but still
  proposed. Creation
  means the definition was recorded, not that code ran or delivery was arranged.

Proposed lifecycle capabilities, whether separate tools or actions on one tool:

- Inspect/list: owner-scoped, paginated; report definition, next eligible slot,
  and bounded occurrence history without secrets or unrequested full output.
- Disable: atomically prevents future admissions, leaving admitted work alone.
- Delete: stops future admissions and removes the definition from normal lists.
  It does not cancel work or retract delivered events. Retention/tombstones and
  the treatment of pending notifications are OPEN and must be explicit.
- Edit/resume/manual-run: OPEN, not required for the first surface. If added,
  edits affect future slots and use revision checks. They never rearm a consumed
  `(schedule_id, due_at)`. A manual execution gets a distinct identity.

Creation-request deduplication is also OPEN: losing the tool response must not
silently create two schedules when a client repeats its request. A principal-
scoped request key with payload-conflict detection is one option; it is separate
from occurrence deduplication and does not authorize execution retries.

## Minimal architecture and existing integration points

```text
schedule tool -> owner/authority validation -> durable definition + next slot
                                                    |
                                      one scheduler observes due slots
                                                    |
                              consume occurrence + durable dispatch receipt
                                                    |
                                      one Engine run_js submission
                                                    |
                            actual execution observation + durable outcome
                                                    |
                              optional Events record -> webhook adapter
```

Proposed first deployment: one scheduler owner per persistent store. Do not add
worker leases, takeover, distributed retries, or a workflow engine to establish
this interface. Multi-process ownership is a separate design gate.

Use [`mcp.rs`](../server/src/mcp.rs) for tool schemas and transport integration,
[`mcp_dispatch.rs`](../server/src/mcp_dispatch.rs) for shared dispatch, and the
[`Engine`](../server/src/engine/mod.rs) for execution. Preserve operation hooks,
policy checks, timeouts, and resource bounds; do not introduce a privileged
JavaScript execution path.

Current code needs deliberate adaptation:

- Stateful `run_js` returns `execution_id` before execution finishes. Stateless
  `run_js` polls for up to five minutes; a polling-timeout error does not prove
  execution stopped. An MCP Task wrapping a tool call is not a durable schedule
  or proof of the underlying JavaScript's final state. Observe the execution.
- [`ExecutionRegistry`](../server/src/engine/execution.rs) keeps execution
  records in a `DashMap`; console/artifact storage does not make those records
  recoverable. Durable schedule and occurrence receipts are new requirements.
- The engine's global semaphore can delay execution after submission. Overlap
  permission does not imply unlimited capacity. Choose bounded admission and
  saturation behavior before implementation; do not accumulate an unbounded
  queue and call it missed-slot dropping.

### Admission and failure boundary

Proposed storage transaction: verify enabled state/current authority, consume the
unique occurrence, record its immutable payload revision, and advance the next
slot on the original cadence. Only then attempt submission once. Store the
returned execution ID when known.

This deliberately permits loss: a crash after consumption but before submission
can leave a slot unexecuted. A crash after submission but before recording the
ID can leave it unknown. Recovery must not resubmit either case. Later new slots
continue normally. Uncertain outcomes remain inspectable; timeout/cancellation
must not claim that previous external side effects were undone.

Persist an observed terminal result and its event record together where possible.
If the result was never durably observed, do not synthesize success or rerun code
to reconstruct it. A notification delivery attempt cannot create another
execution. Existing execution status, schedule status, and delivery status stay
separate.

## Authority and lifetime

A schedule persists authority to run later. Bind it to a verified principal and
a bounded, revocable execution grant. Revalidate that grant at each admission
and preserve normal per-operation policy enforcement. A client-selected session
header is not proof of ownership. The current
[`SessionVerifier`](../server/src/session.rs) returns a verification boolean;
a trusted principal/authority binding must be designed rather than inferred
from that boolean or copied request headers.

Do not persist raw caller bearer tokens or replay the creation request's entire
`mcp_headers` as future credentials. Use an existing operator-managed authority
reference if available. Grant source, expiry, revocation, and unattended-auth
failure behavior are OPEN implementation prerequisites. Keep scheduling disabled
when the deployment cannot establish ownership; explicit single-owner deployment
configuration is a possible first scope.

Each occurrence should start from its stored payload and explicitly selected
state, not mutate a shared implicit continuation. Heap/filesystem/session choices
remain OPEN because overlapping runs can race on shared state. Permission
revocation prevents new dispatch; it cannot undo an already-sent external call.

## MCP Events: separate delivery contract

Core MCP and the draft Events surface are different contracts. The exact host
compatibility profile and sources are recorded below; support must be negotiated
and tested rather than inferred from an MCP connection.

Proposed event: `schedule.occurrence.finished`, filtered by `schedule_id`.
Its payload would include stable `eventId`, `schedule_id`, `occurrence_id`,
`due_at`, known `execution_id`, terminal status, and bounded result references.
Event naming, failure/unknown events, output inclusion, and schema are OPEN.
Never call an unobserved outcome completed; a later unknown-state notice needs
a distinct, explicit status contract.

Creating a schedule does not subscribe a chat. The host explicitly discovers and
subscribes using `events/list`, `events/subscribe`, and `events/unsubscribe`; it
provides delivery coordinates and a signing secret outside the tool arguments.
A subscription authorizes notifications for schedules its principal may read.
Separate chats have separate grants; a new chat inherits none. Without a
subscription the job can still run and be inspected. Conversely, losing or
expiring delivery must not silently rerun, disable, or delete the schedule.

**Delivery retry policy is OPEN.** Execution's no-retry policy does not decide
whether the same recorded notification may be retried. If delivery is at least
once, retain stable event identity and payload across attempts, bound retries
and retention, and require receiver deduplication. An HTTP acknowledgment is
acceptance by the receiver, not proof of a host turn or its follow-on effects.

### Compatibility snapshot

| Surface | Verified contract and consequence |
|---|---|
| [Core MCP 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/schema) | `subscriptions/listen` carries opted-in tool/prompt/resource changes. Core does not define the Events methods. |
| [Upstream Events proposal](https://github.com/modelcontextprotocol/experimental-ext-triggers-events) | Explicitly experimental, not an official specification. Its [design sketch](https://github.com/modelcontextprotocol/experimental-ext-triggers-events/blob/main/docs/design-sketch-proposal.md) advertises `capabilities.events` and describes poll, stream, and webhook delivery. |
| [ChatGPT MCP Events](https://developers.openai.com/plugins/build/mcp-events) | Documented webhook subset for Work web, desktop Work with Cloud, and dots, subject to controls. Requires MCP 2026-07-28. Polling/streaming and gap/terminated notifications are excluded. |
| [OpenAI Responses MCP](https://developers.openai.com/api/docs/guides/tools-connectors-mcp) | Documents tool discovery/calls. Incoming Events triggering a new Response remains unverified; API response webhooks are a different direction. |
| [Claude API MCP connector](https://platform.claude.com/docs/en/agents-and-tools/mcp-connector) | Explicitly supports tool calls only. [Consumer connectors](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp) do not establish draft Events support. |
| [Claude Code Channels](https://code.claude.com/docs/en/channels-reference) | Separate preview: `experimental["claude/channel"]` and `notifications/claude/channel` over stdio, with explicit opt-in and an open session. Not a drop-in Events client. |

For ChatGPT, the documented webhook has one event per POST, a 256 KiB limit, and
asynchronous processing after acknowledgment. Its retry rules require stable
IDs; 410/413 are not retryable. Select this profile explicitly if targeting that
host. Pin and test the adapter contract: the current upstream sketch uses
callback error `-32027`, whereas the OpenAI guide uses `-32015`.

The [Claude Code runtime documentation](https://code.claude.com/docs/en/mcp)
also says its v2 runtime does not register channel servers negotiating
2026-07-28. A Claude bridge would need its own tested protocol/runtime path.
This proposal does not promise standalone Codex CLI/IDE Events compatibility.

### Existing adapter reference and security requirements

[`weechat-mcp`'s Events guide](https://github.com/r33drichards/weechat-mcp/blob/24d3cbb6b0230a4d1d64a79e740c7601d1818991/MCP_EVENTS.md),
[protocol adapter](https://github.com/r33drichards/weechat-mcp/blob/24d3cbb6b0230a4d1d64a79e740c7601d1818991/event_protocol.py),
and [webhook transport](https://github.com/r33drichards/weechat-mcp/blob/24d3cbb6b0230a4d1d64a79e740c7601d1818991/event_webhooks.py)
are concrete references for authenticated discovery, explicit chat grants,
callback verification, signing, durable subscriptions, and bounded delivery.
They are Python/FastMCP integration examples, not code that can be dropped into
Rust/rmcp or evidence that every client supports the extension. Their retry
counts and TTLs are not selected MCPJS defaults.

Requirements for any implemented webhook adapter:

- Verify callback control with a fresh signed challenge before activation.
  Accept public HTTPS only; reject embedded credentials and special-use/private
  addresses, revalidate DNS at connection time, pin the validated address, and
  verify TLS for the original hostname. No redirects or implicit proxy bypass.
- Sign exact body bytes with Standard Webhooks headers, bound timestamp
  tolerance, and support controlled key rotation. Cap request/response sizes,
  connect/read deadlines, concurrent sends, queues, and subscription counts.
- Keep callback URLs, secrets, and payloads in private persistent storage with
  protected backups. Mask secrets before generic validation/logging middleware;
  never return them in tools, event discovery, errors, or execution output.
- Scope grants to authenticated principal, callback, event, and canonical filter.
  Enforce schedule ownership on subscribe, refresh, replay, and delivery. Expiry,
  revocation, and unsubscribe stop new deliveries; disclose an in-flight send
  cannot be recalled. Define whether unsubscribe drains or cancels pending work.
- Preserve event IDs across retries/replay and define deduplication retention,
  cursor validity, deletion, overflow, and restart behavior. Never silently
  claim lossless delivery after retention expires. No live grants are created
  by server startup, discovery, or schedule creation.

## Decisions required before implementation

1. Final tool name, cron dialect/field count, timezone and daylight-saving
   behavior, return fields, and separate tools versus action-based lifecycle;
   create-response deduplication.
2. Scheduler tick precision/grace, clock jumps, and bounded capacity handling.
   Preserve cron cadence and drop missed-slot backlog.
3. Durable store, single-owner enforcement, trusted execution authority, and
   schedule/result retention. Choose payload/state binding without implicit
   continuation or privilege growth.
4. Events names/filter/output contract and host profile. Decide delivery retry,
   TTL/replay/overflow, unsubscribe, and schedule-deletion behavior separately
   from execution policy.

Implementation gates should cover duplicate timer observations, disable/delete
races, overlapping different occurrences, backward/forward clock jumps, restart
at each admission boundary, lost executor acknowledgments, and failure to store
an outcome. Add principal-isolation/revocation tests and webhook challenge,
SSRF/rebinding, secret-redaction, duplicate/expired-event, and unsubscribe tests.
Finally exercise a real supported host: explicitly subscribe a test chat,
observe one scheduled result, and unsubscribe. A mock webhook proves neither
host compatibility nor a completed follow-on agent action.
