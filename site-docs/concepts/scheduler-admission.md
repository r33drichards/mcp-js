# Why scheduling separates admission, completion, and cancellation

A timer can say that work is due without proving which process may execute it.
The scheduler core defines calendar and policy decisions; Apalis supplies pulses;
the replicated runner owns durable decisions; a dedicated worker submits to the
actual Engine. These boundaries explain the conservative failure behavior.
See [feature status](../reference/scheduler-http-status.md) for the integrated runtime boundary.

## Ownership is a committed transition

A newly elected leader first commits a current-term barrier and acquires a
checkpoint with a new ownership epoch. Election notification alone does not
fence the old owner. The checkpoint contains both ownership and admission, so
an admission cannot race takeover through separate keys.

The claim binds namespace, schedule, occurrence, attempt, owner/epoch, worker,
payload digest, and execution authority. Before a worker requests permission,
it durably reserves the invocation locally. The runner grants permission in one
checkpoint compare-and-set, validating the exact running attempt and binding.
Deduplication excludes the ownership epoch: a new leader or worker incarnation
cannot obtain a second permit for the same invocation attempt.

The worker durably consumes its local launch permission, then uses a non-cloneable
capability to submit to the Engine. This prevents duplicate submissions for an
attempt, but crashes between these steps can lose work. Records are retained so
restart cannot turn a consumed reservation into permission to replay.

## Permission is not an external-effect transaction

An admission ordered before takeover remains valid. Already admitted work may
launch or finish after takeover or grant expiry. External resources require their
own fencing or idempotency if later effects must be constrained. An HTTP 202
acknowledges Engine acceptance; it does not prove completion.

A lost acknowledgement may hide a committed permit or submitted execution.
Recording `Unknown` and retaining overlap/capacity reservations avoids assuming
that the work did not happen. Unknown outcomes never retry automatically. Even a
retry after a confirmed failure can repeat partial effects.

A definite not-launched response requires a bound, quorum-backed terminal fence
that proves no permit exists and prevents later admission. Existing permits, lost
acknowledgements, and quorum uncertainty cannot be converted into that response.
During graceful shutdown the runner keeps admission available while draining
observation and cancellation futures. Engine lifetime tracking includes detached
blocking execution: a driver panic or abort alone does not prove work stopped.

## Disable and cancel answer different questions

Disabling changes future calendar selection. Cancellation targets one occurrence.
The runner commits `Cancelling` before contacting the worker; that state survives
recovery. The worker fences future launches and serializes cancellation with
consume, submit, and receipt persistence. `Cancelled` requires confirmation that
the Engine background task has stopped, rather than a cancellation request alone.
Completion can win the race and preserve its actual result.

Unreachable workers and old-incarnation receipts can remain uncertain. Operator attestation supports
`Unknown` and unresolved `Cancelling`. Neither attestation cancels a permit or proves external
effects were undone. The [recovery guide](../how-to/stop-scheduled-work.md) explains
the evidence required.

## Safety depends on deployment boundaries

Fixed-voter Raft HTTP routes require the shared cluster bearer, including status
and data reads. This authenticates callers but does not encrypt transport or
protect against malicious voters. Keep cluster ports private and use encrypted
transport for remote links; the admission HTTP listener also requires existing
authenticated TLS termination for remote use. This operator-owned prototype provides no tenant principal
mapping, schema migration, log compaction, or exactly-once effects.

For lookup details, see [calendar policies and limits](../reference/scheduler.md).

## Management adapters preserve the durable boundary

Stdin, operator HTTP, and operator MCP commands enter the same serialized runner
loop. The adapters do not create a new scheduler, retry policy, ownership epoch,
or cancellation state. A successful cancellation reply still precedes confirmed
stoppage, and disabling a schedule remains independent of cancellation.

Transport authentication separates routine operator access from human-attested
recovery. The worker, admission, and cluster credentials cannot issue management commands.
A separately authorized recovery caller must supply the existing explicit
quiescence attestation; possession of a credential cannot prove that statement.
The MCP tool description instructs agents not to infer or auto-approve it.

The extra listener is loopback-only. Fixed-voter Raft authenticates separately
and still requires private network boundaries and encrypted remote transport.
Neither the integrated hardening nor management adapters make the stack production-ready. See the [authorization matrix](../reference/scheduler.md#authorization-matrix).
