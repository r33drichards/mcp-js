# Validation models

The current worker protocol is covered by [atomic admission validation](ADMISSION.md).
The ownership model below retains the historical unfenced worker abstraction for
regression/comparison. Its stale-admission and duplicate-delivery counterexamples
are not behavior of the new scheduled-only worker endpoint. The new admission model
instead requires stale-admission rejection and retains only the deliberately allowed
already-admitted late-launch counterexample.

# Historical ownership model and deterministic replay

This validates the experimental Apalis runner in PR284. It does not change the
comparison branch, replace Raft, prove exactly-once execution, or fence the worker.

`RunnerOwnership.tla` separates proposing a claim, durable consumption, receiving
its ACK, submitting an application POST, delivering it, worker admission, worker
completion, and committing a completion receipt. Lost claim ACKs can leave a
consumed occurrence without any dispatch. A consumed dispatch ticket and an
in-flight POST can survive leadership change; network messages and admitted work
survive runner crashes. Restart never recreates a dispatch ticket. Recovery changes
Running to Unknown, and Unknown is never automatically replayed.

Safety assertions are at most one automatic application submission for this
occurrence attempt, no submission without a committed consumption, and no
successful stale/replaced claim ACK. Deliberately checked unsupported assertions
produce counterexamples: a delayed POST can start after takeover, and duplicate
network delivery can admit the same POST twice. The current worker API has neither
an atomic epoch admission fence nor request deduplication. Application submission,
worker start, and external effects are different events. No exactly-once or
no-stale-worker-start guarantee follows from these checks.

## Assumptions and limits

The model assumes a linearizable durable quorum/CAS service and honest, fixed
voters; it is **not a complete Raft specification or proof**. Two possible runner
owners use an implicit third voting witness. Quorum is a Boolean partition
abstraction, not a matrix of directed links. One occurrence, one attempt, two
terms, at most two queued messages/admitted jobs, and one logical due-time advance
are modeled. Fast exploration bounds are 12 transitions and 2 fault events; deep
bounds are 16 and 3. Crash, lost claim ACK, partition, drop, and duplicate consume
the fault budget. Election and completion-ACK loss are bounded by transitions.
There is no log compaction, Byzantine behavior, disk corruption, torn-write
model, arbitrary clock skew, dynamic membership, or multi-occurrence overlap
proof here. Existing core acceptance tests separately cover missed-run, overlap,
and retry policy combinations.

No liveness property is asserted. With arbitrary loss/partition/crashes, progress
cannot be guaranteed. Progress would require eventual stable leadership/quorum,
fair service and delivery, a functioning durable store, and eventual worker
termination/observable results. Even then Unknown deliberately requires operator
resolution and may hold overlap capacity indefinitely. The bounded Spec permits
stuttering and does not encode these fairness assumptions as a temporal proof.

## Model-to-code mapping

| Model transition | Rust adapter / actual production path |
| --- | --- |
| Acquire | `Replicated::acquire`, current-term barrier, checkpoint recovery and fenced `scheduler_cas` |
| Propose / Commit / Ack | Poll actual `Replicated::claim`; explicitly deliver real `handle_append_entries` and `accept_append_response`; run production commit/apply; poll caller ACK |
| LoseAck | Drop pending claim future and owner; durable log/receipt remains |
| Send | Consume real `Invocation`; call an injected `Executor` that records submission and remains pending |
| DeliverPost / Finish | Explicit simulated worker/network queues, independently delayed, reordered by index, dropped, or duplicated |
| DeliverDone | Call actual `Replicated::finish`, including stale owners; assert rejection for stale completion |
| DeliverDoneLostAck | Poll actual completion to pending, commit through handlers, then cancel before caller observes ACK; verify owner invalidation |
| Elect | Production vote handlers on three nodes, then test-only installation of elected leader; election timers are abstracted |
| Crash / Restart | Discard volatile owners/futures/tickets; construct a new node over the same sled database; preserve queued network/worker events |
| Partition / Heal | Disable/enable delivery to quorum; third witness stays available |
| Tick | Explicit due time and Tokio paused clock; no wall sleeps |

The replication adapter sends full logs in reverse peer order, loses the first
reply, and delivers duplicate AppendEntries. It exercises production handlers,
not every possible Raft packet interleaving. The storage test injects a
flush-failure result and verifies no claim ticket escapes and the owner stays unusable;
this is an additional deterministic test, not a TLC-generated disk-failure path.
Restart here reuses the sled handle: separate subprocess/binary tests cover abrupt
process exit and reopening the on-disk database. The injected Executor is not
HTTP; existing remote and binary tests exercise the actual HTTP adapter.

After each generated event, replay compares committed receipt, dispatch tickets,
submission/admission counts, network/work queues, and logical time. It does not
compare every internal model variable with every Raft field. Replay covers selected
shortest witnesses (all 18 action types plus recovery scenarios), two diagnostic
counterexamples, and two mutation counterexamples, **not all explored states or
edges**. This is bounded trace conformance evidence, not exhaustive refinement
checking. Witnesses of `WitnessAbsent` are intentional counterexamples used to
reach an action/state; they are not safety failures.

## Reproduce and inspect

Requires Java 21, Python 3, and the repository Rust toolchain. Download TLC from
its official release; `check.py` verifies its SHA-256 before execution. The pinned
release asset currently identifies itself as TLC 2026.10.15.145915 (932d35e).
A changed upstream asset fails closed; review a new pin rather than bypass it.

```bash
curl --fail --location https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar -o /tmp/tla2tools.jar
python3 specs/runner/check.py --jar /tmp/tla2tools.jar --out /tmp/runner-model
SCHEDULER_MODEL_TRACES=/tmp/runner-model cargo test --locked --manifest-path scheduler-runner/Cargo.toml --lib model_replay -- --nocapture
python3 specs/runner/mutations.py
# Deeper bounded exploration, same deterministic replay:
python3 specs/runner/check.py --jar /tmp/tla2tools.jar --out /tmp/runner-model-deep --deep
SCHEDULER_MODEL_TRACES=/tmp/runner-model-deep cargo test --locked --manifest-path scheduler-runner/Cargo.toml --lib model_replay -- --nocapture
```

TLC uses one worker, seed 1, fingerprint polynomial 0. `summary.json` records the
bounds, model/tool hashes and generated/distinct state counts. Each normalized
trace has `format: 1`, model/name/seed/bounds, expected invariant violation,
optional mutation name, and an ordered `states` array. Each state contains an
`event {op,node,arg}`; node 0 means no owner, nodes 1/2 identify runners, and queue
indices are one-based. Raw `*.tlc.json` and logs retain TLC's original
counterexample output (`-dumpTrace json`, documented in the
[official TLC tools documentation](https://github.com/tlaplus/tlaplus/blob/master/general/docs/current-tools.md)).
Normalized fast fixtures are checked in so ordinary Cargo tests need no Java or
network. CI regenerates traces and replays them; manual workflow dispatch selects
the deeper bounds and publishes logs/traces as artifacts. Changing model semantics
requires regenerating fixtures and reviewing their summary.

Negative controls are distinct:

* TLA `ReplayUnknown` and `IndexOnlyAck` mutations must produce their expected
  safety counterexamples; unmodified production must reject the resulting traces.
* `mutations.py` changes disposable source copies to restore index-only claim ACK,
  truncate a mismatched committed prefix, or remove the expected-term fence.
  Each must fail its named **handwritten** production Raft regression. Compilation
  errors or missing tests do not count as detection. The working tree is untouched.

These checks expose missing worker admission fencing; resolving that requires a
worker protocol/architecture decision, not another runner-side pre-POST check.

Recorded validation for the current model: fast safety exploration generated
12,286 states / 4,214 distinct; deep generated 141,464 / 25,511 distinct. Both
26-trace replay sets passed, covering all 18 action types. All three source-copy
negative controls were detected. These counts describe the bounded model, not
production state-space coverage; see the checked-in summaries for exact hashes.
