# Scheduler NixOS acceptance tests

These tests extend PR #284. Each boots a NixOS VM and starts three real Raft
runner processes plus the scheduled-only server and its actual JavaScript Engine.
The HTTP probe is an execution observer, not a mock worker or admission service.
All network traffic stays inside the guest on loopback; credentials are disposable
test constants. Four GiB RAM and four virtual CPUs are allocated to each VM.

Run separately to avoid starting all three VMs at once:

```sh
nix build -L .#checks.x86_64-linux.scheduler-disable-test
nix build -L .#checks.x86_64-linux.scheduler-cancellation-test
nix build -L .#checks.x86_64-linux.scheduler-disabled-cancellation-test
```

`.github/workflows/scheduler-nixos.yml` runs all three checks as independent
GitHub Actions jobs on relevant pull requests (including the design-branch base
used by #284), pushes to `impl/scheduler-apalis` or `main`, and manual dispatch.
Each job verifies KVM, retains failed Nix builds and uploads build/VM diagnostics.
Failures are not ignored, and one failing scenario does not cancel the others.

The guest has time synchronization disabled. The harness advances wall time to
the next actual minute boundary, retaining monotonic deadlines, instead of waiting
a minute per occurrence or replacing the production cron clock. It waits for two
JavaScript heartbeat requests before treating an execution as running.

## Assertions

- Disable only: accepted JavaScript can finish successfully; no next occurrence
  starts after the next cron boundary.
- Cancel only: a running occurrence reaches confirmed `Cancelled`, its heartbeat
  stops, and its normal-completion marker is absent. The schedule stays enabled.
  Duplicate cancellation is idempotent. The next occurrence actually starts and
  succeeds, proving cancellation did not disable scheduling or retain its slot.
- Disable then cancel: the existing execution can still be cancelled after the
  schedule is disabled, and future occurrences remain disabled.

The cancellation command contract exercised here is
`{"action":"cancel","occurrence_id":ID}` on the acquired runner's operator
stdin interface. Its acknowledgement is not treated as proof of stopped execution;
the test waits for a durable terminal `Cancelled` occurrence and observes the
original execution's heartbeat separately. No external-effect rollback is implied.

## Current implementation status

PR head `8677390325b1769a715bd889bcaa22bd1167cf9f` has neither this command nor
the terminal `Cancelled` state. The two cancellation checks are acceptance tests
for the requested implementation and are expected to fail on that head. They
are not skipped or marked expected-failure. The disable-only check exercises
existing behavior. None of these VM checks has been executed in the authoring
environment, which has no Nix installation or KVM device.

Guest configurations, worker output and runner stderr/JSON responses are copied
from `/tmp/scheduler-integration` into the test-driver output, including on failure.

This suite does not yet test cancellation across takeover, cancellation racing
with admission or completion, unreachable workers, retention of uncertain capacity,
MCP principal isolation, Events delivery, migration or compaction. Those remain
separate acceptance gates in the implementation plan.
