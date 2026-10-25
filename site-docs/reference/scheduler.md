# Scheduler reference

Baseline: [PR #293 feature status and pending integration](scheduler-http-status.md).
The experimental runner is a separate Cargo workspace. It uses Apalis pulses,
the shared durable policy core, fixed-voter Raft, and a dedicated actual-server
worker. It provides an operator stdin interface, not scheduling MCP tools.

## Configuration

Run `mcp-js-scheduler-runner CONFIG.json`. Unknown runner fields are rejected.

| Runner field | Meaning |
|---|---|
| `node_id` | Stable unique voter ID |
| `advertise_addr`, `cluster_port` | Raft address and listening port |
| `peers` | Other fixed voters as `id@host:port` |
| `data_dir` | Dedicated durable runner store |
| `admission_addr` | Callback HTTP listener address |
| `admission_bearer_env` | Environment reference for shared callback credential |
| `limits` | Optional core limits below |
| `schedules` | Initial definitions; default empty |
| `remote` | Required worker/authority configuration below |

All replicas must share the same limits, initial definitions, and remote config.
The fixed voter mapping and contract are sealed into storage. Existing incompatible
stores fail closed; there is no automatic migration. The runner checkpoint schema
is version 2 and the worker wire protocol version 1.

`remote` fields: `url` (worker origin), `bearer_env` (worker credential reference),
`expires_at` (RFC3339 grant expiry), `session` (logging tag), `timeout_seconds`,
`heap_mb`, `namespace`, and `worker_id`. The session is not a principal identity.

The worker reads JSON from `MCP_SCHEDULED_WORKER_CONFIG`:

```json
{
  "namespace": "operator-scheduler-1",
  "worker_id": "worker-1",
  "data_dir": "/var/lib/scheduled-worker/admissions",
  "bearer_env": "MCP_SCHEDULER_WORKER_TOKEN",
  "admission_bearer_env": "MCP_SCHEDULER_ADMISSION_TOKEN",
  "admission_urls": ["http://127.0.0.1:4501"]
}
```

Namespace and worker ID must match the runner's remote configuration. Origins
require HTTPS except loopback HTTP; embedded credentials, paths, queries, and
fragments are rejected. Redirects and automatic retries are disabled. At this
baseline callback fallback occurs only on HTTP 409. Tokens are resolved from the
named environment variables, not stored as config values.

The actual server must use HTTP mode and `--heap-store none` (the default).
SnapshotCreator cancellation can abort with pending async references, so heap
snapshot mode is rejected. Filesystem persistence is independent of heap storage.

## Worker and admission HTTP routes

| Route | Purpose |
|---|---|
| `POST /api/scheduled/v1/exec` | Bound dispatch; reserve, obtain quorum permit, consume, submit; 202 means accepted |
| `GET /api/scheduled/v1/executions/{id}` | Observe execution with worker credential |
| `POST /api/scheduled/v1/cancel` | Original bound dispatch; fence launch and reconcile stop |
| `POST /scheduler/v1/admit` | Runner callback; atomic checkpoint admission |

The dedicated worker mounts only the first three routes, with its configured
bearer credential and any existing JWKS enforcement. There is no fallback to
`/api/exec`, MCP, SSE, or HTTP functions. The runner callback uses the admission
credential. Dispatch binds version, namespace, schedule/occurrence/attempt,
owner/epoch, worker, payload digest and execution authority. It is an internal
runner protocol, not a replacement for operator management. At this baseline
scheduled/admission routers allow 16 MiB bodies; upcoming #284 reduces this.

## Definition and calendar policy

```json
{"name":"example","code":"console.log('scheduled')","cron":"* * * * *","timezone":"UTC","policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}}
```

| Field | Contract |
|---|---|
| `name` | Optional, at most 128 bytes |
| `code` | At most 64 KiB |
| `cron` | Numeric five-field expression, at most 256 bytes |
| `timezone` | Explicit IANA timezone, at most 128 bytes |
| `policy.missed` | `"Skip"`, `"RunOnce"`, or `{"CatchUp":{"cap":N}}` |
| `policy.overlap` | `"Skip"`, `"Queue"`, `"Concurrent"` |
| `policy.retry` | Additional attempts: `max_retries` 0–100, fixed `delay_seconds` 0–86400 |

Cron accepts stars, lists, ranges, and steps. Sunday is 0 or 7; restricted day-of-
month and day-of-week use OR. Named fields, aliases, seconds, years, and `L/W/#/?`
extensions are rejected. Croner's fixed-time jobs move spring-gap times to the first valid instant after
the gap and run only the first fall-fold instance. Wildcard/interval jobs skip
the gap and can run in both fold instances as distinct UTC due times. Creation
schedules strictly after creation time.

`Skip` selects the latest due slot only within the grace window. `RunOnce`
coalesces unobserved slots into the newest. `CatchUp` selects at most the newest
cap slots in chronological order and discards older slots for that observation.
The cadence stays on original calendar slots. Duplicate/backward observations do
not rearm consumed slots.

Overlap follows missed-run selection. `Skip` drops a slot when the schedule has
active, unknown, queued, or retry-waiting work. `Queue` uses FIFO with one active
occurrence per schedule. `Concurrent` permits different occurrences subject to
the global running limit. Retries retain occurrence identity and increment attempt;
only confirmed `Failed` outcomes retry. Unknown never retries automatically.

## Limits

| Core field | Default | Validation |
|---|---|---|
| `schedules` | 64 | 1–1024 |
| `records` | 4096 | 1–65536 |
| `outstanding` | 128 | Positive, no greater than records |
| `running` | 16 | Positive, no greater than outstanding |
| `grace_seconds` | 5 | 0–59, inclusive grace window |

These are policy limits, not a bound on the retained Raft log and claim/admission
envelope at this baseline. No general compaction or history erasure is provided.

## Operator commands and states

One JSON command per stdin line, sent to the acquired leader:

```json
{"action":"status"}
{"action":"list"}
{"action":"create","definition":{"name":"example","code":"console.log('scheduled')","cron":"* * * * *","timezone":"UTC","policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}}}
{"action":"disable","id":1}
{"action":"cancel","occurrence_id":2}
{"action":"delete","id":1}
{"action":"acknowledge_stopped","occurrence_id":2,"dispatcher_and_work_quiesced":true}
```

`status` identifies acquired ownership; `list` is explicitly a stale local view.
Mutations require quorum. Creation lacks request deduplication: inspect state
after an uncertain result. EOF leaves scheduling active. Ctrl-C drains bounded
observation tasks; it does not cancel accepted external effects.

States are `Pending`, `Running`, `Cancelling`, `Cancelled`, `Succeeded`, `Failed`,
`Unknown`, `Abandoned`, each carrying attempt information. `Unknown` and
`Cancelling` retain running/overlap reservations. `acknowledge_stopped` accepts
only `Unknown` here; newer #284 extends this to unresolved `Cancelling`.
Older binaries cannot read cancellation enum variants. Do not roll back a live
store with unresolved cancellation to such a binary.

See [operational recovery](../how-to/stop-scheduled-work.md) and
[admission safety](../concepts/scheduler-admission.md).
