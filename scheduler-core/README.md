# Scheduler core comparison

> Apalis branch update: [`../scheduler-runner/README.md`](../scheduler-runner/README.md)
> adds an optional runner-only, fixed-voter Raft adapter. The local core described
> below retains its policy contract and acceptance tests. Only this branch adds
> checkpoint staging and the distributed adapter; PR285 remains the local comparison.

This experimental crate compares Apalis and tokio-cron-scheduler as timer drivers
for the same durable scheduling policy. It exposes an executor interface; it does
not register MCP tools, execute JavaScript itself, create unattended authority,
or deliver MCP Events. See [BACKEND.md](BACKEND.md) for this branch's adapter.

Both alternatives start at design branch `docs/schedule-tool-mcp-events`, commit
`b3fe6e53fa1f5e3f0de4d8d1f6a5241a2d97ecf8` (PR #282). They are sibling changes,
not a stack on one another. The policy core, runner, examples, and acceptance
tests are identical. Only the timer adapter, dependencies, and backend notes vary.
The crate has its own Cargo workspace and lockfile so this comparison does not
upgrade the server's dependencies or enable scheduling in a deployment.

## Policy contract

| Concern | Default | Configurable alternatives |
|---|---|---|
| Missed runs | `Skip` | `RunOnce`, `CatchUp { cap }` |
| Overlap | `Skip` | `Queue`, `Concurrent` |
| Execution retries | Zero | At most 100 additional attempts, fixed delay up to 24 hours |

`observe(now)` evaluates a five-field cron schedule against explicit UTC time.
Creation starts strictly after the supplied time. The default grace is five
seconds, inclusive; it must be less than one minute. A later observation:

- `Skip`: admits the latest due slot only if it is within the grace window.
- `RunOnce`: coalesces all unobserved slots into the newest due slot, once.
- `CatchUp`: admits at most the newest `cap` due slots in chronological order.
  Older slots are discarded in that observation; later ticks do not drain them.

All modes advance to the next original calendar slot after the observation.
They do not shift the cadence to execution completion. Duplicate and backward
observations cannot rearm consumed slots. A large forward jump applies the
selected missed-run policy; it is not treated as proof of downtime.

Overlap applies after missed-run selection. `Skip` drops a selected slot if the
schedule has running, unknown, queued, or retry-waiting work. `Queue` keeps FIFO
order and allows one active occurrence per schedule. `Concurrent` allows different
occurrences together, subject to the global running limit. A retry keeps its
occurrence ID and increments its attempt number; queued later occurrences cannot
jump ahead of an earlier retry delay. Thus catch-up plus overlap-skip can admit
fewer than `cap` slots. These policies are independent, not additive guarantees
that every selected occurrence will execute.

Only `Outcome::Failed` permits a configured retry. The executor must return this
only after execution actually stops. An acknowledgement timeout, cancellation
without confirmed termination, lost response, or panic is `Unknown`; none causes
an automatic retry. Opting into retries can repeat external side effects even
after a confirmed failure. Consumers that need idempotency must implement it.

## Calendar semantics

The shared parser is pinned to Croner 4.0.1, independently of either driver's
internal parser. Inputs require five numeric fields and an explicit IANA timezone.
Supported syntax is `*`, lists, ranges, and steps. Named months/weekdays,
`@` aliases, seconds, years, and extensions such as `L`, `W`, `#`, and `?` are
rejected. Sunday is 0 or 7; restricted day-of-month and day-of-week use OR.
Invalid, impossible, or out-of-search-range calendar evaluation returns an error.
The runner stops on that error rather than silently consuming more work.

Croner's pinned DST rules distinguish fixed-time and wildcard/interval jobs.
Fixed-time jobs in a spring gap move to the first valid instant after the gap;
in a fall fold they run at the first instance only. Wildcard/interval jobs skip
the gap and can run in both fold instances, identified by distinct UTC due times.
Tests cover New York's 2026 spring gap and fall fold. These are explicit comparison
choices, not claims about every cron implementation or historical timezone rule.
See [Croner's source and DST tests](https://docs.rs/croner/4.0.1/src/croner/lib.rs.html).

## Persistence and execution boundary

`Core::open(directory, limits)` opens a dedicated sled 0.34.7 database and the
reserved `scheduler_state` tree. `Core::from_config` accepts `sled::Config`, following
[SessionLog's constructor](../server/src/engine/session_log.rs); named trees and JSON
records follow the existing [label store](../server/src/engine/fs_labels.rs).
Other trees in the database are left alone. This core is standalone; it does not
route writes through the repository's Raft cluster path or attach to a live server.

The complete bounded snapshot is one `state` key. A sled compare-and-swap atomically
replaces schedule cursors, occurrence receipts, IDs, and outcomes, rejecting an
unexpected writer. Every successful transition then explicitly calls `Db::flush`
before returning. Atomic visibility alone is not a durability acknowledgement.
The default constructor disables periodic flushing because every transition already
flushes; custom configurations may enable it without weakening the explicit boundary.
See [sled's flush contract](https://docs.rs/sled/0.34.7/sled/struct.Db.html#method.flush).

Sled owns the database's process lock. Keep exactly one core per database and do
not mutate its reserved tree externally. Use private local storage; network
filesystems, replicas, takeover, and malicious local writers are outside the contract.
After the last handle drops, sled background I/O may briefly retain its file lock;
a same-process reopen can return a lock error and should be retried after release.
Tests bound that release wait separately from their explicit scheduler clock.
Old SQLite prototype stores are rejected rather than silently replaced with empty
state; this revision does not implement a migration or delete old files.

The transition sequence is:

1. `observe` commits occurrence receipts and advances schedule cursors together.
2. `claim` commits `Running` before returning an `Invocation` to the executor.
3. `finish` commits a confirmed outcome, retry wait, or unknown outcome.

A pending receipt can be claimed after restart because it was never handed out.
A running receipt becomes unknown on reopen. A crash between claim and submission
can therefore lose an execution; a crash after submission can lose knowledge of
its result. Neither case resubmits the attempt. There is no atomic transaction
with external execution and no exactly-once external-effects guarantee. The
caller of the low-level `claim` API must not reuse returned invocations.

Unknown receipts hold both overlap and global capacity slots. After separately
establishing that execution stopped, `acknowledge_stopped` makes a receipt
`Abandoned`, releases its slot, and never retries it. Its effect outcome remains
unresolved. Failure to persist any transition poisons the instance, preventing
further claims until it is reopened. The fault tests cover a failed write and an
atomic write whose flush fails but whose bytes subsequently reach disk. These fault
injections exercise conservative recovery; they do not simulate every device failure.

Disabling or deleting stops future admission atomically relative to `observe`.
Already-admitted work, including queued work and configured retries, may continue.
Deletion hides the definition from normal lists but retains its tombstone and
receipts. Definitions cannot be edited or resumed in this slice. There is no
creation-request deduplication or public identity/authorization API.

## Bounds and lifecycle

Default limits are 64 definitions (including deleted tombstones), 4,096 retained
receipts, 128 outstanding occurrences, and 16 running/unknown occurrences.
Payloads are at most 64 KiB and names 128 bytes. Outstanding includes queued,
retry-waiting, running, and unknown work, so failures cannot overflow the queue
when they become retries. Limits are persisted and must match when reopening.

When admission is full, selected slots are discarded and the schedule's `rejected`
counter increases. This counter includes overlap/capacity rejections, not the total
number of historical missed slots. Oldest terminal receipts may be evicted;
unresolved work is never evicted. Tombstones count toward the definition limit
and there is no purge API yet. History is bounded inspection, not an audit archive.
Admission visits schedules in ID order; fairness under saturation is not promised.

`run_until` owns one runner lease per core. Each native driver produces a one-second
observation pulse into a capacity-one channel. Delayed pulses coalesce. The runner
uses current wall time, then dispatches claimed invocations through `Executor`.
The core decides admission; neither native driver owns executable payloads or
persistent occurrence records. Consequently this compares timer integration and
dependency cost, not Apalis's durable queue against tokio-cron's native stores.

An executor future must cover the actual execution lifetime, not just submission.
Graceful stop prevents new admission and drains executing futures, without a forced
timeout. Aborting the runner cancels its local futures and marks consumed work
unknown; this does not prove remote effects stopped. Panics also become unknown.
A future server adapter must preserve the existing Engine's authorization, hooks,
resource bounds, and actual completion observation. No such adapter is enabled here.

`Core::from_checkpoint` creates a pure-memory planner without opening a temporary
sled database or flushing provisional state; its caller must durably commit the
resulting checkpoint before exposing a returned invocation. `Core::open` and
`Core::from_config` retain the local sled durability boundary described above.

This is a small-workload prototype: state transitions clone and serialize bounded
snapshots, and synchronous sled writes and flushes can block a runtime worker. It has
no
throughput benchmark, distributed guarantees, encryption, schema migration, result
payload archive, delivery/replay system, or production authority integration.

## Relationship to the design model

[The selected model](../specs/TaskScheduleCron.md) allows overlapping occurrences,
drops missed slots, and never retries executions. Our defaults intentionally use
non-overlap, following the later comparison request. `Skip` missed runs plus
`Concurrent` overlap plus zero retries is the closest configuration to that model,
but this implementation adds cron parsing, grace, persistence, capacity, and failure
boundaries that the model abstracts away. Run-once, catch-up, queueing, non-overlap,
and retries are application policies; the existing TLA+ results do not prove them.
The design branch and its model are unchanged.

## Run and compare

From either branch, with Rust 1.99.0:

```sh
cargo test --manifest-path scheduler-core/Cargo.toml --locked
cargo fmt --manifest-path scheduler-core/Cargo.toml --check
cargo clippy --manifest-path scheduler-core/Cargo.toml --locked --all-targets -- -D warnings
cargo run --manifest-path scheduler-core/Cargo.toml --locked --example inspect
```

The example inspects opaque payloads; it does not execute JavaScript. The common
deterministic tests cover all 18 policy combinations, repeated/backward/forward
observations, queue ordering, retry delay/caps, capacity and retention, disable/delete
ordering, timezone behavior, and store reopen. A subprocess exits without destructors
at pending, claimed, submitted, and completed boundaries. These are process-restart
tests, not power-loss or disk-corruption certification. One ignored test is a helper
invoked by the subprocess test, not omitted coverage.

The runner tests check the executor interface, graceful stop, cancellation, panic,
and exclusive runner ownership. The real native pulse test uses wall time with a
five-second timeout; policy decisions use explicit timestamps, not timer sleeps.
Store-reopen tests may wait briefly for sled to release its file lock.
The same CI workflow runs on both comparison branches and stacked PRs.
