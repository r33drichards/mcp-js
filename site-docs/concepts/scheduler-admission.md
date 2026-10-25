# Why scheduling separates admission, completion, and cancellation

A timer can say that work is due without proving which process may execute it.
The scheduler core defines calendar and policy decisions; Apalis supplies pulses;
the replicated runner owns durable decisions; a dedicated worker submits to the
actual Engine. These boundaries explain the conservative failure behavior.
See [feature status](../reference/scheduler-http-status.md) for the baseline and
pending upstream fixes.

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

## Disable and cancel answer different questions

Disabling changes future calendar selection. Cancellation targets one occurrence.
The runner commits `Cancelling` before contacting the worker; that state survives
recovery. The worker fences future launches and serializes cancellation with
consume, submit, and receipt persistence. `Cancelled` requires confirmation that
the Engine background task has stopped, rather than a cancellation request alone.
Completion can win the race and preserve its actual result.

Unreachable workers and old-incarnation receipts can remain uncertain. The #293
baseline supports operator attestation for `Unknown` only; newer #284 adds it for
unresolved `Cancelling`. Neither attestation cancels a permit or proves external
effects were undone. The [recovery guide](../how-to/stop-scheduled-work.md) explains
the evidence required.

## Safety depends on deployment boundaries

At this baseline Raft RPCs are unauthenticated. Fixed voters do not authenticate
network callers. The scheduler therefore requires an isolated trusted network;
its admission HTTP listener also requires authenticated TLS termination for
remote use. Upcoming #284 authentication is an integration prerequisite, not a
property of this base. This operator-owned prototype provides no tenant principal
mapping, schema migration, log compaction, or exactly-once effects.

For lookup details, see [calendar policies and limits](../reference/scheduler.md).
