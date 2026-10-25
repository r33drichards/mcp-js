# Apalis runner and fenced scheduled worker

PR284 adds an experimental no-V8 runner using the existing Raft implementation.
Apalis supplies timer pulses; the application implements persistence, recovery,
missed-run/overlap/retry policy and worker admission. PR285 remains unchanged.

The approved contract is **reject stale admissions after committed takeover,
prevent duplicate engine submissions per invocation attempt, and allow already
admitted work to launch or finish afterward**. Admission is not an atomic
transaction with external effects. This is not exactly-once execution, lossless
replay, automatic cancellation on takeover, or a production-readiness claim.

## Protocol and durable boundary

1. The leader commits a current-term barrier, then acquires a checkpoint with a
   monotonically increasing ownership epoch. Reacquisition in the same Raft term
   also advances this epoch. Old Running occurrences become Unknown.
2. Claiming consumes an occurrence and stores its exact versioned Dispatch:
   namespace, schedule/occurrence/attempt, owner/epoch, worker identity, payload
   digest and execution-authority binding. The runner sends one POST to
   `/api/scheduled/v1/exec`. It never falls back to `/api/exec`.
3. The worker authenticates the request and durably reserves the invocation in
   its local store before making any admission RPC. Duplicate delivery, aborted
   requests and restart never replay that local reservation.
4. The worker submits the bound request plus its process incarnation to
   `/scheduler/v1/admit`. The current runner validates Running/attempt, owner,
   epoch, exact claim, expiry and absence of prior admission inside **one
   checkpoint CAS**, committing the permission before returning it. Ownership and
   admission cannot race through separate keys.
5. Deduplication identity excludes ownership epoch. A fresh store or another
   worker incarnation cannot obtain a second quorum permit for the invocation.
   The permit binds worker incarnation, payload and authority. The worker consumes
   its local launch permission with a durable CAS before obtaining a non-cloneable
   launch capability. That capability calls the actual Engine once.
6. HTTP 202 is engine acceptance, not completion. The runner polls
   `/api/scheduled/v1/executions/{id}`. Lost ACKs, ambiguous status or observation
   timeout remain Unknown and retain overlap/capacity reservations.

Takeover is the committed ownership transition, not merely a leader-election
notification. An admission ordered before takeover remains valid afterward. Grant
expiry is checked during reservation/admission validation; the admission CAS may
finish later. Already admitted work may launch after expiry or takeover. This is
intentional. External resources need their own fencing/idempotency to constrain
later effects; confirmed-failure retries may repeat partial effects.

Quorum loss, timeout, malformed/mismatched ACK or cancellation while obtaining
admission yields no new launch permission. Previously admitted work may still launch. The admission itself may nevertheless have committed. A crash
between reservation, admission, local consumption and Engine submission can lose
work. Unknown is never automatically replayed. Worker receipts and the Raft log
are retained without compaction; current checkpoint metadata is bounded as described below. Preserve stores; do not clone an active worker
incarnation or use restored stale consensus snapshots as a new authoritative cluster.

## Compatibility and configuration

This changes the experimental runner checkpoint schema to **version 2** and the
worker wire protocol to **version 1**. Existing pre-admission checkpoints fail
closed; there is no automatic migration. Use fresh fixed-voter consensus stores,
a fresh dedicated worker admission store, and a unique namespace for a new cluster.
Do not reset an existing store to bypass a version or identity rejection.

The worker is a dedicated mode of the actual server binary. Set
`MCP_SCHEDULED_WORKER_CONFIG` to an operator-supplied JSON file and use `--http-port`.
In this mode the server mounts only the three scheduled routes: no `/api/exec`,
`/mcp`, SSE or stdio execution path. It uses the configured bearer reference for
these routes, with existing JWKS enforcement also applied if configured. Existing deployments without this mode keep their existing APIs;
legacy workers cannot service the new runner. The session field remains a logging
tag, not a principal identity. This operator-owned prototype does not implement
multi-tenant principal-to-grant binding or MCP management.

Example worker config (no token values are stored):

```json
{
  "namespace": "operator-scheduler-1",
  "worker_id": "worker-1",
  "data_dir": "/tmp/scheduled-worker-admissions",
  "bearer_env": "MCP_SCHEDULER_WORKER_TOKEN",
  "admission_bearer_env": "MCP_SCHEDULER_ADMISSION_TOKEN",
  "admission_urls": ["http://127.0.0.1:4501", "http://127.0.0.1:4502", "http://127.0.0.1:4503"]
}
```

Supply existing credential values through those environment references. Never put
values in config, snapshots or command arguments. Runner replicas share the
admission credential; the worker shares it for callback authentication. The
scheduled-worker credential is used only at the dedicated worker port. This work
creates no credentials and changes no deployed security settings.

```sh
cargo build --locked -p server
MCP_SCHEDULED_WORKER_CONFIG=/path/to/worker.json target/debug/server --http-port 8080
cargo build --manifest-path scheduler-runner/Cargo.toml --locked
```

Adapt [example.json](example.json). For three replicas use node IDs `runner-1..3`,
cluster ports `4401..4403`, admission addresses `127.0.0.1:4501..4503`, unique data
directories and the other two fixed voters as `id@host:port` peers. Every replica
must have the same remote config, limits and initial definitions. Match its remote
namespace/worker ID to the worker config and set `remote.url` to the dedicated
worker origin (`http://127.0.0.1:8080` locally). Run each in its own terminal:

```sh
scheduler-runner/target/debug/mcp-js-scheduler-runner /path/to/runner-1.json
scheduler-runner/target/debug/mcp-js-scheduler-runner /path/to/runner-2.json
scheduler-runner/target/debug/mcp-js-scheduler-runner /path/to/runner-3.json
```

The runner admission listener serves HTTP; use an existing authenticated TLS
termination arrangement for remote access. Both worker and callback clients require
HTTPS except loopback HTTP. Redirects/retries are disabled. Callback endpoints are
tried in configured order only after a definitive 409 (no acquired owner). Any
network ambiguity or other error stops the attempt. An offline first endpoint can
therefore cause lost work even if another leader is available; this is conservative
failure behavior, not robust leader discovery. Fixed-voter scheduler networking now requires a shared cluster credential and
binds the configured advertised address. Set `cluster_bearer_env` in each runner
config to an environment reference (see `example.json`); provide the same secret
on every voter. It is never persisted in the checkpoint, membership or log. All
cluster HTTP routes, including status/data reads, require that bearer. Pure
in-process model nodes can omit it, but cannot start a network listener/elections.
Legacy dynamic cluster mode retains its existing network behavior.

The shared bearer authenticates callers; it does not encrypt HTTP or make Raft
Byzantine-tolerant. Keep cluster ports firewalled on a private network and use
operator-supplied encrypted transport for remote links. The admission listener
also speaks HTTP and requires existing TLS termination for remote use. This
change creates no deployed credential and changes no deployed security setting.

## Local management

Use stdin `status` on any runner to find the acquired leader, then send commands
to that leader's terminal. EOF leaves scheduling active. Ctrl-C drains bounded
observation tasks; it does not cancel accepted remote effects.

```json
{"action":"status"}
{"action":"create","definition":{"name":"example","code":"console.log('scheduled')","cron":"* * * * *","timezone":"UTC","policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}}}
{"action":"list"}
{"action":"disable","id":1}
{"action":"cancel","occurrence_id":2}
{"action":"delete","id":1}
{"action":"acknowledge_stopped","occurrence_id":2,"dispatcher_and_work_quiesced":true}
```

List is a stale local view. Mutations require quorum. Acknowledge-stopped requires
operator evidence that dispatch and accepted work are quiesced; it releases an
Unknown or Cancelling capacity reservation and does not cancel a previously admitted permit.
Unadmitted terminal/abandoned occurrences cannot obtain a permit. Creation has no
request deduplication, so inspect state after an uncertain management result.

## Explicit execution cancellation

The dedicated worker requires `--heap-store none` (the default). Cancelling a
V8 SnapshotCreator with pending async references can abort the process, so heap
snapshot mode is rejected at startup rather than exposed through this endpoint.
Filesystem persistence is a separate axis and is not disabled by this restriction.

`cancel` targets an occurrence independently of its schedule. Disabling a schedule
prevents future occurrences and does not cancel accepted work. Operators can issue
both commands explicitly; cancellation still works for a disabled schedule.

The runner commits `Cancelling` before contacting the worker. That state survives
restart/takeover and holds overlap/running capacity. Each owner reconciles it using
`POST /api/scheduled/v1/cancel` with the original bound dispatch, even after grant
expiry. Cancellation is idempotent; it never automatically retries the execution.
Pending, unclaimed occurrences can become `Cancelled` immediately.

The worker durably fences the invocation against future launches and serializes
cancellation with consume/submit/receipt persistence. For submitted work it requests
Engine cancellation, then confirms `Cancelled` only after the background task has
joined V8. An acknowledgement alone does not establish stopped execution. Completion
may win the race and retain its actual completed/failed outcome instead. Confirmed
cancellation releases capacity and leaves future scheduled runs enabled.

Unreachable workers, consumed launches without a durable execution receipt, or
receipts from a previous worker incarnation remain uncertain and retain capacity.
These are not false cancellation successes. Cancellation does not roll back
external effects or establish that remote operations/subprocess effects stopped.
Worker cancellation tombstones/receipts remain uncompacted. Older binaries cannot
read the new `Cancelling`/`Cancelled` enum variants; do not roll back a live store
with unresolved cancellation to such a binary.

## Validation and remaining limits

```sh
cargo test --locked --manifest-path scheduler-runner/Cargo.toml
cargo clippy --locked --manifest-path scheduler-runner/Cargo.toml --all-targets -- -D warnings
cargo test --locked -p server --lib scheduled_runtime_tests
cargo test --locked --manifest-path server/cluster-tests/Cargo.toml
cargo test --locked --manifest-path scheduler-core/Cargo.toml
```

[Admission model and replay](../specs/runner/ADMISSION.md) records bounds, generated
traces, negative controls and mapping limits. Real HTTP tests compose the production
worker gate, actual single-voter Raft admission and a counting mock runtime. The
separate real Engine adapter test executes JavaScript. Binary tests exercise three
actual runner processes and a mock worker at a real minute boundary; those tests
measure submission/failover, not actual Engine admission. Process exit and injected
flush errors are not power-loss tests.

Independent admission review covers both takeover orderings, duplicate requests,
old-incarnation and payload binding, lost ACK, restart, terminal state and flush
failures. The planner loaded from a checkpoint is purely in memory; Raft supplies its
durability. Claims keep metadata/digests rather than duplicate schedule code, and
claim/admission records retire when their occurrence/attempt is no longer retained
by Core. Eligibility still checks the exact live occurrence and non-reused ID, so
pruning cannot re-admit retired work. Checkpoint JSON is capped at 4 MiB; transitions
that would exceed that budget fail closed. This is an admission/storage budget,
not a throughput guarantee or a migration mechanism.

Scheduled execution/admission bodies are capped at 512 KiB (including worst-case
JSON escaping of 64 KiB code); cluster bodies are capped at 16 MiB. Outgoing
AppendEntries carries at most 32 entries / 12 MiB, and newly persisted entries
must fit that replication budget. Followers backtrack logarithmically on prefix
mismatch rather than one entry per heartbeat. Invalid extreme terms are rejected
before persistence; elections use checked addition and fail closed at exhaustion.
The absolute term bound avoids rejecting legitimately lagging peers merely for
missing many elections.

A definite no-launch response is emitted only after a quorum-backed terminal
fence proves no permit exists and prevents any future admission for that attempt.
Duplicate admissions with an existing permit, lost ACKs and quorum uncertainty
remain Unknown and retain capacity. Graceful shutdown continues serving admitted
callback work, drains both observation and cancellation futures, and does not wait
for stdin EOF to let the Tokio runtime exit.

The Engine's driver and blocking execution share a lifetime guard: a driver panic
or abort does not confirm stopped while detached blocking execution remains alive.
Handle-publishing polls are bounded. Named regression tests cover both immediate
cancellation and cancellation after startup separately from ordinary execution.

The Raft log and worker receipts still grow without automatic compaction, and
deleting a schedule does not erase historical replicated code. No
Events delivery, schema migration, multi-tenant authorization, creation deduplication,
Raft compaction, throughput benchmark or automatic Unknown resolution is provided.
