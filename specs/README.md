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

## TaskSchedule

Design model for a **proposed scheduling layer over existing MCP Tasks**.
A one-time schedule creates one task; a periodic schedule creates a task for
each observed, eligible occurrence. Tool names and arguments are immutable
payloads and are abstracted away. No scheduler server code is implemented here.

This replaces the earlier durable-worker proposal in this PR. Durable execution,
worker leases/epochs, retries, heap or filesystem snapshots, session forks,
transactional outboxes, and external-effect deduplication are outside this slice.
MCP Tasks supplies the execution lifecycle; this model only decides task admission.

### Scheduling behavior

- `Create` registers a future (or currently due) one-time or periodic schedule.
  An interval of zero means one-time. Positive intervals repeat on the original
  cadence: `FirstDue + n * Interval`, not completion time plus interval.
- `Fire` creates one logical task and consumes its occurrence in the same action.
  This is an atomic local scheduler decision, not a distributed exactly-once
  guarantee. Task transport, tool execution, and external effects are not modelled.
- `Skip` consumes an occurrence if its schedule is disabled or already has an
  active execution. It does not queue that occurrence for later.
- `Disable` prevents future fires without cancelling an existing execution.
- `Finish` releases the schedule's active slot when execution has ended. A
  cooperative MCP cancellation acknowledgement alone is not proof of termination
  and must not release this slot while the execution is still running.
- `Advance` moves abstract scheduler observation time forward. Periodic slots
  strictly before the new observation are skipped; an occurrence exactly at the
  observation remains eligible. The next timestamp stays on the original cadence.

For example, with hourly times at 1, 2, 3, and 4 PM, a gap from 12:30 to 2:30 PM
skips 1 and 2 PM and leaves 3 PM next. If the 3 PM task is still active at 4 PM,
the 4 PM occurrence is skipped; completing it later does not replay that slot.

`now` is an abstract observation timestamp, not a wall-clock punctuality promise.
The model processes due decisions before moving observation time again. Clock
jumps describe omitted periodic slots; they do **not** model durable schedule
storage, process restart, or recovery. Real timer jitter and task-start latency
are abstracted away.

**Overdue one-time behavior remains undecided.** To avoid silently selecting a
policy, clock jumps cannot cross an enabled, pending one-time deadline in this
model. This is a model scope restriction, not a runtime requirement. One-time
schedules are checked when their deadline is observed.

### Properties and bounds

The committed safe config checks:

| Property | Guarantee within the model |
|---|---|
| `TypeOK` | State remains well-formed |
| `NoEarlyFire` | Admission is never before the scheduled occurrence |
| `NoCatchUp` | Fires belong to the current observed slot, never a missed backlog |
| `NoOverlap` | Each schedule has at most one active execution |
| `NoDisabledFire` | Every fire was admitted while its schedule was enabled |
| `NoDuplicateFire` | Each `(schedule, due time)` creates at most one task |
| `OneTimeAtMostOnce` | A one-time schedule creates at most one task |
| `CadencePreserved` | Periodic timestamps remain aligned with their original cadence |
| `DueEventuallyProcessed` | Each observed due slot is eventually fired or skipped |
| `TasksEventuallyFinish` | Active executions eventually release their slots |

The last two properties use weak fairness of `Fire`, `Skip`, and `Finish`.
Execution termination is an assumption of this abstraction, not a guarantee of
MCP Tasks or arbitrary JavaScript. No schedule is required to fire if disabled,
missed, or blocked by an active execution; no crash-recovery liveness is claimed.

The fixture has two schedules: one one-time at tick 2, and one periodic starting
at tick 1 with interval 2. Time is bounded at tick 6; the six-task audit bound
exceeds the four possible eligible occurrences, so it does not limit safe runs.
The clock may advance one tick or jump over multiple periodic slots, and task
completion and disabling may interleave with admission.

Three constants inject broken implementations for negative checks. They are
**not proposed user-configurable policies**:

- `AllowOverlap = TRUE`: removes the active-execution admission guard.
- `KeepMissed = TRUE`: leaves past periodic slots pending after clock jumps.
- `ConsumeOccurrence = FALSE`: creates a task without advancing its due slot.

### Running and results

Use the gitignored `tla2tools.jar` downloaded above. Run from `specs/`:

```sh
# All eight safety invariants and both conditional progress properties pass.
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config TaskSchedule_safe.cfg TaskSchedule.tla

# Each faulty design must produce its named invariant violation.
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config TaskSchedule_overlap.cfg TaskSchedule.tla
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config TaskSchedule_catchup.cfg TaskSchedule.tla
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config TaskSchedule_duplicate.cfg TaskSchedule.tla
```

`-deadlock` disables deadlock reporting because finite observation time permits
terminal states; `Spec` permits stuttering. The safe run still checks its explicit
temporal properties. TLC was run using the jar from the v1.8.0 release URL above.

| Config | Observed TLC result |
|---|---|
| `TaskSchedule_safe.cfg` | No error; 803 distinct states |
| `TaskSchedule_overlap.cfg` | `NoOverlap` violated: periodic fires at ticks 1 and 3 without finishing |
| `TaskSchedule_catchup.cfg` | `NoCatchUp` violated: jump to tick 2, then fire missed tick 1 |
| `TaskSchedule_duplicate.cfg` | `NoDuplicateFire` violated: fire one-time tick 2, finish, fire tick 2 again |

These are bounded design checks, not verification of a scheduler implementation.
