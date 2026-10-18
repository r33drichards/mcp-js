# Atomic admission validation

`RunnerAdmission.tla` models the approved v1 scheduled worker protocol: a committed
ownership fence rejects new old-epoch admissions; previously admitted work may
launch afterward. Invocation identity excludes epoch. No exactly-once external
effects or cancellation guarantee is asserted.

The model separates claim, send/delivery choice, local reservation, quorum admission,
permit ACK, local consume and engine launch. Lost admission ACK records consumption
without releasing a permit. A quorum-uncertain attempt releases none. Worker crashes
lose volatile permits/launch capabilities but preserve reservation/consumption.
Owner takeover preserves quorum admission records and advances ownership epoch.

Safety checks: no stale admission, at most one admission and launch per invocation,
and launch requires admission. `NoLateLaunchDiagnostic` intentionally fails: admit,
then takeover, then launch is permitted. `SkipEpoch` and `SkipDedup` mutations must
fail their respective safety invariants; unmodified production rejects those traces.
These are different from the historical unfenced worker counterexamples in
`RunnerOwnership.tla`, which remain legacy regression evidence and do not describe
the new worker route.

## Bounds and mapping

Fast: 10 transitions; deep: 14. One invocation/attempt, two owner epochs, two
worker stores with the same worker ID and fresh incarnations on restart, at most two queued requests.
Partitions, message duplication/drop and crash/restart are bounded by transition
count. Quorum is Boolean. The underlying durable linearizable CAS service is an
assumption; this is not a full Raft proof. No liveness/fairness property is checked.
Progress requires stable quorum, reachable callback endpoints and worker service;
consume-before-launch deliberately permits lost work even then.

Each run exports 25 selected shortest witnesses/counterexamples, covering 17 action
types and takeover, consumed-before-crash, reservation/restart and lost-ACK scenarios.
These are not every explored state or edge. Rust replays actual
`Replicated::claim_for_worker/admit/acquire`, real Raft vote/append/commit handlers,
and the same `Worker::reserve/consume` and non-cloneable `Launch` used by the server.
A counting Runtime observes engine submissions; a separate server test exercises
the real Engine adapter. A paused Tokio clock and explicit RPC deliveries replace
background election/replication timers. Election installation is a test seam after
real vote handlers grant a quorum, as in the ownership harness.

Replay compares committed epoch/admission count, real returned permits and launch
capabilities, runtime submission count and reservation/consumption success markers.
It does not compare every Raft/model variable. Send/duplicate/drop model steps choose
which bound Dispatch to deliver; the adapter performs no network I/O for those
steps. Separate real HTTP tests cover concurrent duplicate delivery, authentication,
absence of legacy/MCP routes and a committed-but-lost admission ACK. Worker restart
in generated replay reopens sled; subprocess runner tests independently cover
abrupt process exit. Flush-failure injection is supplemental, not a physical disk
fault or TLC-generated storage model.

Expiry is checked during reservation and admission validation, not atomically with
wall-clock passage at commit. Already admitted work may launch after expiry. The
model abstracts expiry away; it models takeover ordering, not time-limited leases.
Payload/authority/incarnation mismatches and terminal occurrence state have direct
production tests outside the bounded model. External side effects are not modeled.

## Reproduce

Use the same SHA-256-pinned official TLC jar as `check.py` (see README):

```sh
python3 specs/runner/check_admission.py --jar /tmp/tla2tools.jar --out /tmp/admission
SCHEDULER_ADMISSION_TRACES=/tmp/admission cargo test --locked --manifest-path scheduler-runner/Cargo.toml --lib admission_replay -- --nocapture
python3 specs/runner/check_admission.py --jar /tmp/tla2tools.jar --out /tmp/admission-deep --deep
SCHEDULER_ADMISSION_TRACES=/tmp/admission-deep cargo test --locked --manifest-path scheduler-runner/Cargo.toml --lib admission_replay -- --nocapture
python3 specs/runner/admission_mutations.py
```

Seed 1, fingerprint polynomial 0, one TLC worker. Normalized JSON has format/version,
model, seed, bounds, mutation/expected violation and ordered states with
`event {op,worker}`. Worker 0 denotes no worker; 1/2 identify independent stores.
`summary.json` records model/tool hashes and exploration counts. Raw TLC JSON/logs
are CI artifacts. Committed fixtures support ordinary Cargo tests without Java.
CI regenerates/replays both models; manual dispatch selects deep bounds.

Source-copy negative controls remove the ownership/eligibility fence, remove
quorum deduplication, permit repeated local consumption, or skip the consumption
flush. Each must fail its named production regression; compilation failures are
not detections. Source copies and tests do not modify the working tree.

Recorded checks: fast safety generated 53,698 states / 15,748 distinct; deeper
safety generated 260,389 / 58,319 distinct. See committed summaries for the exact
model and tool hashes. The earlier ownership model's counts remain separate.
