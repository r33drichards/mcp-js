# Simple cron-style task scheduling

Selected design: dispatch each new scheduled occurrence independently.

- Overlap is allowed. A running, failed, or unknown prior execution does not
  block or pause the schedule.
- Each `(schedule, due time)` is dispatched at most once. There are no automatic
  retries of failed or uncertain occurrences.
- Periodic schedules retain their original cadence. Missed periodic slots are
  dropped, not queued for catch-up.
- Disabling a schedule prevents new dispatches; it does not cancel active work.

This is a focused revision to PR #281, based on
[`a6b49e98f65282a4ebb5d57d0ea462c2caed1409`](https://github.com/r33drichards/mcp-js/commit/a6b49e98f65282a4ebb5d57d0ea462c2caed1409).
The existing `TaskSchedule` files are retained for comparison. Their previous
non-overlap/skip-active rule is deliberately absent from this selected design.

## Model boundary

`Fire` atomically records a logical dispatch and consumes its occurrence.
`running` is an observational count, never an admission guard. `Finish` can
represent success or failure, and never rearms an occurrence. Unknown or
nonterminating executions need not finish.

The model verifies scheduler decisions, not distributed delivery or exactly-once
external effects. Durable storage, dispatch acknowledgement loss, production
implementation, retries, fencing, and failure detectors are outside this slice.
No physical non-overlap guarantee is intended.

Like the original model, overdue one-time behavior remains undecided: clock jumps
cannot cross an enabled, pending one-time deadline. This scope restriction is not
a proposed runtime rule. Clock observations are abstract, not a timing SLA.

The fixture has one one-time schedule at tick 2 and one periodic schedule at
ticks 1, 3, and 5, with time bounded at 6. The 12-dispatch audit bound exceeds all
four eligible occurrences. Scheduler admission/disabled-skip/clock actions have
weak fairness; execution completion has **no fairness assumption**.

## Run

From the repository root, with Java 21, Bash, curl and sha256sum:

```sh
curl -fL -o specs/tla2tools.jar \
  https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar
specs/run-task-schedule-cron.sh
```

The runner verifies SHA-256
`0f56d6a813368c48008dfbecb23a73ab43a4a5e1bf09f026782a674aa9d4b3c9`
and checks exact expected outcomes. The jar is gitignored. Individual command:

```sh
cd specs
java -Duser.timezone=UTC -XX:+UseParallelGC -Xmx2g -cp tla2tools.jar tlc2.TLC \
  -workers 1 -seed 1 -fp 0 -deadlock -noGenerateSpecTE \
  -config TaskScheduleCron_safe.cfg TaskScheduleCron.tla
```

`-deadlock` permits finite-horizon end states; invariants and temporal properties
are still checked. `KeepMissed` and `ConsumeOccurrence` are fault-injection
constants, not proposed user options.

## Actual TLC results

Run 2026-10-15 UTC; jar reports `TLC2 Version 2026.10.13.164326 (rev: 210df3a)`.
Raw output is in [`cron-results/`](cron-results/).

| Config | Result |
|---|---|
| `safe` | Pass: 905 distinct states, 2,579 generated, empty queue |
| `overlap` | Expected diagnostic failure: tick 1 and tick 3 both active |
| `catchup` | Expected `NoCatchUp` failure with missed-slot dropping disabled |
| `retry` | Expected `NoAutomaticRetry` failure without occurrence consumption |

The safe run checks well-formed state, no early dispatch, no catch-up, disabled
admission protection, at-most-once dispatch per occurrence, one-time at-most-once,
fixed cadence, and eventual processing of observed due slots. These are bounded
design checks, not an implementation proof.

The overlap trace is intentional: create periodic schedule → observe tick 1 →
dispatch occurrence 1 → observe tick 3 → dispatch occurrence 3 while occurrence 1
is still active. These are **different occurrences**, not a retry. The diagnostic
`NoOverlapDiagnostic` is excluded from the selected design's invariant set.
