# Stop scheduled work and recover uncertain capacity

Use the runner's operator stdin interface on its acquired leader. This guide
assumes you have a configured runner and dedicated scheduled worker. Consult
[feature status](../reference/scheduler-http-status.md) before applying it to a
newer scheduler revision.

## Prevent future runs

Find the acquired leader with `status`, then inspect `list` on that leader:

```json
{"action":"status"}
{"action":"list"}
{"action":"disable","id":1}
```

Replace 1 with the schedule ID. Disabling stops future occurrences. It does not
cancel admitted work. Deletion tombstones the schedule; it does not erase history
or roll back effects. A list response is explicitly a stale local view; quorum
is required for mutations.

## Cancel an existing occurrence

Find the occurrence ID in `list`. To stop that occurrence while keeping its
schedule enabled:

```json
{"action":"cancel","occurrence_id":2}
{"action":"list"}
```

To stop both current and future work, issue `disable` and `cancel` separately.
Cancellation works even after disabling the schedule. A successful command reply
means cancellation was requested. Wait for the durable occurrence state:

| State | Meaning and action |
|---|---|
| `Cancelling` | Intent committed; capacity remains reserved. Continue observation. |
| `Cancelled` | Worker confirmed Engine background execution stopped; capacity released. |
| `Succeeded` / `Failed` | Completion won the race; inspect the actual outcome. |
| `Unknown` or persistent `Cancelling` | Outcome remains uncertain; investigate dispatcher, worker, and effects. |

Cancellation survives runner takeover and is idempotent. It does not reverse
network requests or establish that remote services or subprocess effects stopped.
Future scheduled occurrences remain enabled unless you disable the schedule.

## Resolve an uncertain occurrence

First establish that the old dispatcher cannot submit and all in-flight work is
quiescent. Reconcile external effects independently. Then attest explicitly:

```json
{"action":"acknowledge_stopped","occurrence_id":2,"dispatcher_and_work_quiesced":true}
```

This accepts `Unknown` or unresolved `Cancelling`, marking it `Abandoned` and
releasing capacity without replay. It does not invalidate a previously admitted
permit; the attestation requires evidence about both dispatch and work.

A worker restart can leave a previous-incarnation receipt unresolved in
`Cancelling`. Keep capacity reserved until that evidence is established. Do not
clear stores or claim cancellation success merely to unblock scheduling.

## Investigate unavailable admission

Check matching namespace/worker identity, configured credential references,
expiry, acquired leadership, quorum, and worker reachability. Callback endpoints
advance after a definitive 409 or definite connection-establishment failure;
timeouts and ambiguous sends remain uncertain. Never replay an uncertain dispatch
manually as if it were a fresh execution.

See [scheduler reference](../reference/scheduler.md) for fields and
[admission explanation](../concepts/scheduler-admission.md) for the durable boundary.
