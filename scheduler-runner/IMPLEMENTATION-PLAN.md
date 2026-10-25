# Complete the distributed scheduler

## Required behavior

Cancellation targets one occurrence independently of schedule enablement.
Disabling prevents future scheduled occurrences and leaves accepted executions
alone. Operators may explicitly request both operations. Cancellation must still
work after a schedule is disabled.

Persist cancellation intent, distinguish requested from confirmed stopped, and
retain active capacity until stopped execution is established. Admission, launch,
completion, takeover, duplicate requests and unavailable workers must not lose
that intent or falsely report successful cancellation. Cancellation cannot undo
external effects. Define treatment of already pending occurrences separately
from prevention of new scheduled occurrences.

## Work and acceptance gates

- Implement independent occurrence cancellation across runner, admission store,
  worker and Engine; add a terminal cancelled state without automatic retries.
- Add NixOS tests with actual runners and JavaScript execution. Initial tests in
  `tests/nixos/scheduler/` cover disable-only, cancel-only, and disable then cancel.
  They are authored, not VM-validated. Cancellation tests require implementation.
- Extend cancellation tests to admission/launch/completion races, ownership
  takeover, worker loss, uncertain capacity retention and restart recovery.
- Implement Events delivery with durable delivery identity, retries and consumer
  deduplication semantics. Test disconnected consumers and replay after restart.
- Bind authenticated MCP principals to schedule ownership and execution grants.
  Test cross-principal create/read/change/cancel denial and revoked authority.
- Implement explicit versioned checkpoint migration. Test old fixtures, interrupted
  migration, unsupported versions and preservation of unresolved/admitted work.
- Compact retained checkpoints, claims and admission records while preserving
  stale-admission fencing and duplicate-launch rejection. Test delayed deliveries
  and process restarts after compaction.
- Update operator documentation, bounded models/replays, regression suites and CI.
  Record actual passing results and remaining limits before marking work complete.

Do not merge or deploy as part of these tests.
