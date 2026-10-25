# Stop scheduled work and recover uncertain capacity

Use the runner's operator stdin interface or configured HTTP/MCP management
listener on its acquired leader. This guide
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

## Resolve an Unknown occurrence

First establish that the old dispatcher cannot submit and all in-flight work is
quiescent. Reconcile external effects independently. Then attest explicitly:

```json
{"action":"acknowledge_stopped","occurrence_id":2,"dispatcher_and_work_quiesced":true}
```

At the #293 baseline this accepts only `Unknown`, marking it `Abandoned` and
releasing capacity without replay. It does not invalidate a previously admitted
permit; the attestation requires evidence about both dispatch and work.

**Baseline limitation:** a worker restart can leave a previous-incarnation
receipt unresolved in `Cancelling`, and this command rejects that state. Preserve
the records and keep capacity reserved. Do not clear stores or claim cancellation
success to unblock it. PR #284 at `ffe1195` adds attested cleanup for unresolved
`Cancelling`; it requires integration through the parent stack before use here.

## Investigate unavailable admission

Check matching namespace/worker identity, configured credential references,
expiry, acquired leadership, quorum, and worker reachability. At this baseline,
callback endpoints advance only after a definitive 409. A refused connection to
the first endpoint stops admission even if another endpoint has a leader. The
upstream fix permits failover on definite connection-establishment failure;
timeouts and ambiguous sends remain uncertain. Never replay an uncertain dispatch
manually as if it were a fresh execution.

See [scheduler reference](../reference/scheduler.md) for fields and
[admission explanation](../concepts/scheduler-admission.md) for the durable boundary.

## Use the authenticated management listener

Enable the runner's optional [management configuration](../reference/scheduler.md#operator-http-and-mcp-transport)
with separate existing operator and recovery credential references. Keep the
listener on loopback. Query its status using the operator credential:

```sh
curl --fail-with-body http://127.0.0.1:4601/scheduler/v1/status \
  -H "Authorization: Bearer $MCP_SCHEDULER_OPERATOR_TOKEN"
curl --fail-with-body http://127.0.0.1:4601/scheduler/v1/schedules \
  -H "Authorization: Bearer $MCP_SCHEDULER_OPERATOR_TOKEN"
```

Send the same command object used by stdin to the command endpoint. For example,
to cancel occurrence 2 independently of its schedule's enabled state:

```sh
curl --fail-with-body http://127.0.0.1:4601/scheduler/v1/commands \
  -H "Authorization: Bearer $MCP_SCHEDULER_OPERATOR_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary '{"action":"cancel","occurrence_id":2}'
```

For MCP, connect a sessionless `2026-07-28` client to this runner's `/mcp`, supplying
the operator bearer on every request. Use `scheduler_status`, `scheduler_list`,
and `scheduler_cancel` with `{"occurrence_id":2}`. The ordinary server's `/mcp`
and the dedicated worker do not forward scheduler management requests.

Recovery requires the separately configured recovery credential over either
transport. Complete the human investigation above before sending
`acknowledge_stopped`, or its MCP equivalent `scheduler_acknowledge_stopped`.
Never ask an agent to manufacture the required attestation. Omitting the recovery
credential configuration keeps this operation local to operator stdin.

After an uncertain result, inspect state and operator diagnostics. Neither a lost
response nor a 503 establishes that a mutation did not commit. The management
layer does not retry commands, cancel them on client disconnect, or automatically
release uncertain capacity.
