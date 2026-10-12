# Formal specs

TLA+ models backing correctness-sensitive parts of the server.

## FsLabelAtomicWrite

Models a cluster-mode label move (`server/src/engine/fs_labels.rs`), which must
move the label head pointer **and** append a reflog entry recording that move.
`fs_reset` and the reflog-rooted GC both depend on the reflog covering every
head the label has ever pointed at, so a head that moves without a reflog entry
is silent corruption.

The `Mode` constant selects the implementation:

- `split` — the pre-fix code: the head and the reflog are two separate
  replicated writes. The model proves this is unsafe.
- `combined` — the fix: the head write and the reflog write ride in a single
  replicated log entry and are applied as one atomic batch. The model proves
  this is safe.

### Invariant

```
StableReflogCoversHead == (phase = "idle") => ((head = 0) \/ (head \in reflog))
```

In any settled state, a non-initial head is recorded in the reflog. (The
stronger `AlwaysReflogCoversHead` drops the `phase = "idle"` guard and holds for
`combined` too, showing the head is never even transiently observable ahead of
the reflog.)

### Running

Grab the TLA+ tools jar once (gitignored):

```sh
curl -fsSL -o tla2tools.jar \
  https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar
```

Then model-check each config (`-deadlock` is passed only because the model
terminates after `MaxMoves` rather than stuttering):

```sh
# safe — "Model checking completed. No error has been found."
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config FsLabelAtomicWrite_combined.cfg FsLabelAtomicWrite.tla

# unsafe — "Invariant StableReflogCoversHead is violated" + counterexample
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config FsLabelAtomicWrite_split.cfg FsLabelAtomicWrite.tla
```

### Result

- `combined`: all reachable states satisfy the invariants — **no error**.
- `split`: violated. Minimal counterexample:

  | step | action | head | reflog | phase |
  |------|--------|------|--------|-------|
  | 1 | Init | 0 | {} | idle |
  | 2 | StartMove | 1 | {} | reflogPending |
  | 3 | FailReflog | 1 | {} | idle |

  After step 3 the label head is committed at `1` but the reflog is empty, so
  `fs_reset`/GC cannot see how the label reached its current head.

The fix in `cluster.rs` (`cas_with` / `put_with` carrying `extra` writes applied
via a single `apply_batch`) and `fs_labels.rs` (using them for `create`/`cas`/
`force` in cluster mode) implements the `combined` design.

## DurableSched

Design model for a **proposed** durable job scheduler (`schedule_js`): jobs
stored in the Raft-replicated metadata store, claimed by workers under a lease,
and committed by compare-and-swap on the session head. Nothing here is
implemented yet; the model exists to choose the retry and conflict policies
before any code is written.

A heap is modelled as the sequence of events that produced it, so equal
sequences stand for equal content hashes. Raft is abstracted to one atomic
linearizable state machine. There are no clocks: a lease may expire at any
moment, so every safety result holds for arbitrary clock skew and pauses, and a
worker whose lease expired keeps running as a zombie until it is fenced.

Two constants select the policy:

- `RetryBase` — which heap a retry starts from.
  - `pinned` — the head captured at the job's first claim.
  - `head` — the session head at the time of each claim.
  - `checkpoint` — the job's last durable checkpoint, else as `pinned`.
- `OnConflict` — what `Complete` does when the session head is no longer the
  heap the attempt started from.
  - `reject` — fail the attempt, keep the base.
  - `rebase` — fail the attempt, drop base and checkpoint, so the next attempt
    starts from the current head.
  - `clobber` — commit anyway (last writer wins).

### Running

```sh
# safe — "Model checking completed. No error has been found." (~610k states)
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config DurableSched_pinned_rebase.cfg DurableSched.tla

# trap — "Invariant NoDead is violated"
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config DurableSched_pinned_reject.cfg DurableSched.tla

# lost writes — "Action property HeadOnlyGrows is violated"
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config DurableSched_clobber.cfg DurableSched.tla

# at-least-once side effects — "Invariant FxAtMostOnce is violated"
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config DurableSched_side_effects.cfg DurableSched.tla
```

### Result

All nine `RetryBase` x `OnConflict` combinations were checked with two jobs,
two workers, two steps per job, three attempts and one interactive `run_js` on
the same session. The verdicts are the same for all three retry bases except
where a row names them.

| Property | reject | rebase | clobber |
|---|---|---|---|
| `TypeOK`, `OneOwner`, `HeapAtMostOnce`, `NoPartialJob`, `FxBounded` | holds | holds | holds |
| `Terminates` (checked with two attempts) | holds | holds | holds |
| `HeadOnlyGrows` — no committed write is discarded | holds | holds | **violated** |
| `DoneIsDurable` — a done job's effects are in the head | holds | holds | **violated** |
| `NoDead` with `Faults = FALSE` | **violated** for `pinned` and `checkpoint`; holds for `head` | holds | holds |
| `FxAtMostOnce` — external side effects | violated | violated | violated |

- **`pinned` or `checkpoint` with `reject` is a trap.** Two jobs claim on the
  same head, one commits, and the other can never commit: it retries from its
  stale base until its attempts run out. The shortest counterexample is 17
  states with no crash, no lease expiry and no interactive run.
- **`clobber` loses data.** It discards interactive writes and can erase
  another job that was already marked done.
- **`head` needs more attempts than concurrent writers.** With three attempts
  and two other writers `NoDead` holds; with two attempts it is violated.
- **Side effects are at-least-once under every policy.** A zombie keeps calling
  out after its lease expires; fencing protects the heap, not the outside
  world. Scripts need an idempotency key (job id, fire time, attempt).

Design consequence: make both settings per-job, default to `head`, and refuse
`pinned` + `reject` and `checkpoint` + `reject` at schedule time unless the job
owns its session exclusively. Jobs that want deterministic retries use
`pinned` + `rebase`.

### Not modelled

- Raft leader changes and log replication (one atomic state machine).
- The load/execute/commit race inside an interactive run (`UserRun` is atomic).
- Recurring schedules: a job fires once; a cron job is a series of such fires.
- A "fork to a new session" conflict policy, and whether checkpointing reduces
  repeated side effects.
